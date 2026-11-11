//! 反向控制信道：业务平台主动拨入中台的常连接。
//!
//! 业务平台可能部署在没有入站端口的内网（只能出站访问互联网）。这种网络下
//! 中台无法 `POST` 业务平台，但业务平台可以主动连出来。因此控制指令改走一条
//! 由业务平台发起的长连接：
//!
//! ```text
//!   业务平台 ──dial(出站)──▶ 中台 /api/dogDevice/control-plane/ws
//!        ▲                        │
//!        └──── request / result ──┘   (命令与回执都在这条连接上)
//! ```
//!
//! 边界：这条连接只跑**控制面消息**，不转发任何设备帧。设备 WebSocket 仍然
//! 只连业务平台，中台永远不直连设备。

use crate::common::util::format_datetime;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, mpsc, oneshot};

/// 服务令牌请求头（与业务平台 `ROBODOG_SERVICE_TOKEN` 一致）。
pub const SERVICE_TOKEN_HEADER: &str = "x-service-token";

/// 默认回执超时：长连接通常毫秒级返回，超时说明业务平台侧卡住或断线。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(8);

/// 连接上的消息信封。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub payload: Value,
}

impl Frame {
    pub fn new(kind: &str, payload: Value) -> Self {
        Self {
            kind: kind.to_string(),
            ts: format_datetime(Utc::now()),
            payload,
        }
    }
}

/// 一次控制请求的失败原因。
#[derive(Debug)]
pub enum ChannelError {
    /// 没有任何业务平台连接着（可能重启中）
    Offline,
    /// 业务平台在超时时间内没有回执
    Timeout,
    /// 业务平台明确拒绝（设备离线、未绑园区、指令非法…），消息可直接给运维看
    Rejected(String),
    /// 连接在等待回执期间断开
    Disconnected,
}

/// 中台侧的控制信道注册表。
#[derive(Clone, Default)]
pub struct ControlChannel {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    connections: HashMap<u64, mpsc::Sender<Frame>>,
    pending: HashMap<String, oneshot::Sender<Result<Value, String>>>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl ControlChannel {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否有业务平台连接着。
    pub async fn online(&self) -> bool {
        let mut inner = self.inner.lock().await;
        inner.connections.retain(|_, sender| !sender.is_closed());
        !inner.connections.is_empty()
    }

    /// 当前在线连接数（一个业务平台实例一条）。
    pub async fn connection_count(&self) -> usize {
        let mut inner = self.inner.lock().await;
        inner.connections.retain(|_, sender| !sender.is_closed());
        inner.connections.len()
    }

    /// 新连接注册，返回连接 id 与下行帧接收端。
    pub async fn register(&self) -> (u64, mpsc::Receiver<Frame>) {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(64);
        self.inner.lock().await.connections.insert(id, tx);
        (id, rx)
    }

    /// 连接断开时清理；未回执的请求会立刻收到失败，避免等到超时。
    pub async fn unregister(&self, id: u64) {
        let mut inner = self.inner.lock().await;
        inner.connections.remove(&id);
        let pending = std::mem::take(&mut inner.pending);
        for (_, sender) in pending {
            let _ = sender.send(Err("连接已断开".to_string()));
        }
    }

    /// 向连接发送一帧；连接不存在或已关闭时返回 false。
    pub async fn send(&self, id: u64, frame: Frame) -> bool {
        let sender = self.inner.lock().await.connections.get(&id).cloned();
        match sender {
            Some(sender) => sender.send(frame).await.is_ok(),
            None => false,
        }
    }

    /// 请求/应答：把命令推给业务平台，等待它在同一连接上回执。
    pub async fn request(
        &self,
        request_id: &str,
        kind: &str,
        payload: Value,
        timeout: Duration,
    ) -> Result<Value, ChannelError> {
        let (tx, rx) = oneshot::channel();
        let sender = {
            let mut inner = self.inner.lock().await;
            inner.connections.retain(|_, sender| !sender.is_closed());
            let Some(sender) = inner.connections.values().next().cloned() else {
                return Err(ChannelError::Offline);
            };
            inner.pending.insert(request_id.to_string(), tx);
            sender
        };

        if sender.send(Frame::new(kind, payload)).await.is_err() {
            self.inner.lock().await.pending.remove(request_id);
            return Err(ChannelError::Disconnected);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(ChannelError::Rejected(message)),
            Ok(Err(_)) => Err(ChannelError::Disconnected),
            Err(_) => {
                self.inner.lock().await.pending.remove(request_id);
                Err(ChannelError::Timeout)
            }
        }
    }

    /// 业务平台回执到达：唤醒等待中的请求。
    pub async fn complete(&self, request_id: &str, result: Result<Value, String>) {
        let sender = self.inner.lock().await.pending.remove(request_id);
        match sender {
            Some(sender) => {
                let _ = sender.send(result);
            }
            None => {
                // 超时之后才到的回执：中台侧早已放弃等待，审计会由业务平台侧记录
                tracing::debug!(request_id, "控制信道收到迟到的回执");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn request_without_connection_reports_offline() {
        let channel = ControlChannel::new();
        let error = channel
            .request("req-1", "request", json!({}), Duration::from_millis(50))
            .await
            .expect_err("no connection must be offline");
        assert!(matches!(error, ChannelError::Offline));
    }

    #[tokio::test]
    async fn request_is_answered_by_result_frame() {
        let channel = ControlChannel::new();
        let (id, mut rx) = channel.register().await;

        let waiting = {
            let channel = channel.clone();
            tokio::spawn(async move {
                channel
                    .request(
                        "req-2",
                        "request",
                        json!({"kind": "runtime"}),
                        Duration::from_secs(2),
                    )
                    .await
            })
        };

        // 业务平台侧收到下行帧，然后回执
        let frame = rx.recv().await.expect("frame must be delivered");
        assert_eq!(frame.kind, "request");
        assert_eq!(frame.payload["kind"], "runtime");
        channel.complete("req-2", Ok(json!({"online": true}))).await;

        let value = waiting.await.unwrap().expect("request must succeed");
        assert_eq!(value["online"], true);

        channel.unregister(id).await;
        assert!(!channel.online().await);
    }

    #[tokio::test]
    async fn rejection_message_is_surfaced() {
        let channel = ControlChannel::new();
        let (_id, mut rx) = channel.register().await;

        let waiting = {
            let channel = channel.clone();
            tokio::spawn(async move {
                channel
                    .request("req-3", "request", json!({}), Duration::from_secs(2))
                    .await
            })
        };

        let _ = rx.recv().await.expect("frame must be delivered");
        channel
            .complete("req-3", Err("设备 RD-100 当前未连接".to_string()))
            .await;

        let error = waiting.await.unwrap().expect_err("must be rejected");
        assert!(matches!(error, ChannelError::Rejected(msg) if msg.contains("未连接")));
    }

    #[tokio::test]
    async fn timeout_when_business_never_replies() {
        let channel = ControlChannel::new();
        let (_id, mut rx) = channel.register().await;

        let error = channel
            .request("req-4", "request", json!({}), Duration::from_millis(50))
            .await
            .expect_err("no reply must time out");
        assert!(matches!(error, ChannelError::Timeout));
        // 消费掉下行帧，避免 channel 报错
        let _ = rx.try_recv();
    }
}
