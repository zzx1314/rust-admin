//! 智航 Pro 本体 WebSocket 客户端。
//!
//! 一个逻辑客户端由两条连接组成：
//!
//! ```text
//! Logical Client (client_id = N)
//! ├── control   -> ws://host:10010/control    握手 / 控制请求 / 小数据
//! └── bulkdata  -> ws://host:10010/bulkdata   1401/1403/1405/1407 高频推送
//! ```
//!
//! 建链约束（v1.0.3 §1.1.3）：
//! 1. 先建 control，收到 1201 拿到 `session_id` / `bulkdata_bind_token`；
//! 2. 再建 bulkdata，首帧必须是 1202；
//! 3. 收到 1203 后 bulkdata 才可用；未收到不得发送业务数据。

use super::protocol::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// 请求等待响应的默认超时
const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// bulkdata 绑定应答超时（网关默认 bind_timeout_ms = 30000，这里取一半复核）
const BIND_TIMEOUT: Duration = Duration::from_secs(10);
/// 网关心跳间隔（1300/1301）
///
/// 实测：`192.168.234.234` 的网关 control 会话 **10s** 内收不到 `1300` 就主动断开
/// （关闭帧 reason = `heartbeat timeout`，约 +10.5s 触发）。协议文档写 20s，但固件实际
/// 更严，拉到 20s 会在 11s 左右被踢，这取 5s 留足抖动余量。
const GATEWAY_HEARTBEAT: Duration = Duration::from_secs(5);

/// 待应答请求：`msg_id → 回复发送端`
type ReplySender = oneshot::Sender<Envelope>;
/// 待应答请求：`预期响应类型 → 回复发送端`
///
/// 网关控制类响应（`1303` / `1311`）不带 `msg_id`（文档示例 head 只有
/// `type` / `time` / `src`），只能按「响应类型 = 请求类型 + 1」关联。
type TypeReplySender = oneshot::Sender<Envelope>;

/// 算法侧推送（非请求-响应）投递通道
pub type PushReceiver = broadcast::Receiver<Envelope>;

/// 会话信息（1201 应答）
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // control_path / bind_timeout_ms 供排障与日志使用
pub struct SessionInfo {
    pub client_id: u32,
    pub session_id: String,
    pub bulkdata_bind_token: String,
    pub control_path: String,
    pub bulkdata_path: String,
    pub bind_timeout_ms: u64,
    pub device_type: String,
}

impl SessionInfo {
    fn from_envelope(env: &Envelope) -> Self {
        let data = &env.data;
        Self {
            client_id: data
                .get("client_id")
                .and_then(Value::as_u64)
                .unwrap_or_default() as u32,
            session_id: str_field(data, "session_id"),
            bulkdata_bind_token: str_field(data, "bulkdata_bind_token"),
            control_path: str_field(data, "control_path"),
            bulkdata_path: str_field(data, "bulkdata_path"),
            bind_timeout_ms: data
                .get("bind_timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(30_000),
            device_type: str_field(data, "type"),
        }
    }
}

/// 智航本体客户端：一条 control + 一条 bulkdata
///
/// `bulkdata_tx` / `bulk_bound` 只在 `is_bulk_bound` / `send_bulk` 中使用；
/// 当前协议里外部端不向 bulkdata 写业务数据，保留它们以便后续扩展。
pub struct ZhProClient {
    control_tx: mpsc::Sender<Message>,
    bulkdata_tx: mpsc::Sender<Message>,
    pending: Arc<Mutex<HashMap<u32, ReplySender>>>,
    /// 按「响应类型」等待的请求（控制类 1302 / 1310 用）
    pending_types: Arc<Mutex<HashMap<u32, TypeReplySender>>>,
    msg_seq: AtomicU32,
    pushes: broadcast::Sender<Envelope>,
    session: Arc<Mutex<SessionInfo>>,
    /// bulkdata 是否已完成 1202/1203 绑定
    bulk_bound: Arc<Mutex<bool>>,
    /// RoamerX 控制权是否持有（4000~5999 白名单外的消息需要）
    control_owned: Arc<Mutex<bool>>,
    request_timeout: Duration,
}

impl ZhProClient {
    /// 建立完整会话：control 握手 → bulkdata 绑定。
    ///
    /// `host` 形如 `192.168.168.100`，也可以传完整 `ws://…/control` 前缀地址。
    pub async fn connect(host: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let (control_url, bulkdata_url) = endpoint_urls(host);
        info!("连接智航 control 通道: {control_url}");
        let (control_ws, _) = connect_async(&control_url).await?;
        let (mut control_write, mut control_read) = control_ws.split();

        // 1) 等待网关主动下发的 1201
        let session = Arc::new(Mutex::new(SessionInfo::default()));
        let hello = tokio::time::timeout(Duration::from_secs(10), control_read.next())
            .await
            .map_err(|_| "等待 1201 握手应答超时")?
            .ok_or("control 连接在建链期间被关闭")??;
        let hello_text = hello.to_text().map_err(|_| "1201 不是文本帧")?;
        let hello_env: Envelope = serde_json::from_str(hello_text)?;
        if hello_env.head.msg_type != T_HANDSHAKE_ACK {
            return Err(format!("首帧不是 1201，而是 {}", hello_env.head.msg_type).into());
        }
        let info = SessionInfo::from_envelope(&hello_env);
        info!(
            "握手成功: client_id={}, session_id={}, device_type={}",
            info.client_id,
            info.session_id,
            if info.device_type.is_empty() {
                "unknown"
            } else {
                &info.device_type
            }
        );
        *session.lock().await = info.clone();

        let pending: Arc<Mutex<HashMap<u32, ReplySender>>> = Arc::new(Mutex::new(HashMap::new()));
        let pending_types: Arc<Mutex<HashMap<u32, TypeReplySender>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pushes = broadcast::channel::<Envelope>(256).0;
        let (control_tx, mut control_rx) = mpsc::channel::<Message>(64);

        // control 写循环
        tokio::spawn(async move {
            while let Some(msg) = control_rx.recv().await {
                if control_write.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // control 读循环：响应派发 + 推送广播
        {
            let pending = pending.clone();
            let pending_types = pending_types.clone();
            let pushes = pushes.clone();
            let session = session.clone();
            tokio::spawn(async move {
                while let Some(frame) = control_read.next().await {
                    let Ok(frame) = frame else { break };
                    let Some(text) = frame.to_text().ok() else {
                        continue;
                    };
                    let Ok(env) = serde_json::from_str::<Envelope>(text) else {
                        warn!("control 收到无法解析的消息: {text}");
                        continue;
                    };
                    handle_incoming(&env, &pending, &pending_types, &pushes, &session).await;
                }
                warn!("control 通道已断开");
            });
        }

        // 2) 建 bulkdata 并绑定
        let bulkdata_url_final = if info.bulkdata_path.is_empty() {
            bulkdata_url
        } else {
            format!("{}{}", host_base(host), normalize_path(&info.bulkdata_path))
        };
        info!("连接智航 bulkdata 通道: {bulkdata_url_final}");
        let (bulk_ws, _) = connect_async(&bulkdata_url_final).await?;
        let (mut bulk_write, mut bulk_read) = bulk_ws.split();
        let (bulkdata_tx, mut bulk_rx) = mpsc::channel::<Message>(64);
        let bulk_bound = Arc::new(Mutex::new(false));

        // 首帧 1202
        let bind = Envelope::request(
            T_BIND_BULKDATA,
            0,
            json!({ "session_id": info.session_id, "token": info.bulkdata_bind_token }),
        );
        bulk_write.send(Message::Text(bind.to_text())).await?;

        {
            tokio::spawn(async move {
                while let Some(msg) = bulk_rx.recv().await {
                    if bulk_write.send(msg).await.is_err() {
                        break;
                    }
                }
            });
        }

        // 等 1203
        let ack = tokio::time::timeout(BIND_TIMEOUT, bulk_read.next())
            .await
            .map_err(|_| "等待 1203 bulkdata 绑定应答超时")?
            .ok_or("bulkdata 连接在绑定期间被关闭")??;
        let ack_text = ack.to_text().map_err(|_| "1203 不是文本帧")?;
        let ack_env: Envelope = serde_json::from_str(ack_text)?;
        if ack_env.head.msg_type != T_BIND_BULKDATA_ACK {
            return Err(
                format!("bulkdata 首帧应答不是 1203，而是 {}", ack_env.head.msg_type).into(),
            );
        }
        info!(
            "bulkdata 绑定成功: channel={}",
            str_field(&ack_env.data, "channel")
        );
        *bulk_bound.lock().await = true;

        // bulkdata 读循环：1401/1403/1405/1407 全部作为推送广播
        {
            let pushes = pushes.clone();
            let pending = pending.clone();
            let pending_types = pending_types.clone();
            let session = session.clone();
            tokio::spawn(async move {
                while let Some(frame) = bulk_read.next().await {
                    let Ok(frame) = frame else { break };
                    let Some(text) = frame.to_text().ok() else {
                        continue;
                    };
                    let Ok(env) = serde_json::from_str::<Envelope>(text) else {
                        continue;
                    };
                    handle_incoming(&env, &pending, &pending_types, &pushes, &session).await;
                }
                warn!("bulkdata 通道已断开");
            });
        }

        let client = Self {
            control_tx,
            bulkdata_tx,
            pending,
            pending_types,
            msg_seq: AtomicU32::new(1),
            pushes,
            session,
            bulk_bound,
            control_owned: Arc::new(Mutex::new(false)),
            request_timeout: REQUEST_TIMEOUT,
        };

        client.spawn_gateway_heartbeat();
        client.spawn_status_watch();
        Ok(client)
    }

    /// 网关 1300 心跳，保持 control 会话保活
    fn spawn_gateway_heartbeat(&self) {
        let tx = self.control_tx.clone();
        let seq = self.next_msg_id();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(GATEWAY_HEARTBEAT);
            let mut msg_id = seq;
            loop {
                ticker.tick().await;
                msg_id = msg_id.wrapping_add(1);
                let msg = Envelope::request(T_HEARTBEAT, msg_id, empty());
                if tx.send(Message::Text(msg.to_text())).await.is_err() {
                    break;
                }
            }
        });
    }

    /// 订阅 1201/1207/1299 等会话类通知（只做日志与状态维护）
    fn spawn_status_watch(&self) {
        let mut rx = self.pushes.subscribe();
        let control_owned = self.control_owned.clone();
        let session = self.session.clone();
        tokio::spawn(async move {
            while let Ok(env) = rx.recv().await {
                match env.head.msg_type {
                    T_MODULE_NOTIFY => {
                        if let Some(modules) = env.data.get("modules").and_then(Value::as_array) {
                            let desc: Vec<String> = modules
                                .iter()
                                .map(|m| {
                                    format!(
                                        "{}={}",
                                        str_field(m, "name"),
                                        m.get("available")
                                            .and_then(Value::as_bool)
                                            .unwrap_or(false)
                                    )
                                })
                                .collect();
                            debug!("模块可用性: {}", desc.join(", "));
                        }
                    }
                    T_TAKE_CONTROL_NOTIFY => {
                        let status = env
                            .data
                            .get("status_code")
                            .and_then(Value::as_u64)
                            .unwrap_or(1);
                        if status == 0 {
                            *control_owned.lock().await = true;
                            info!(
                                "已取得 RoamerX 控制权: client_id={}",
                                env.data
                                    .get("client_id")
                                    .and_then(serde_json::Value::as_u64)
                                    .unwrap_or(0)
                            );
                        } else {
                            warn!(
                                "获取 RoamerX 控制权失败: {}",
                                str_field(&env.data, "message")
                            );
                        }
                    }
                    T_GATEWAY_ERROR => {
                        error!("网关传输层错误: {}", str_field(&env.data, "message"));
                    }
                    T_HANDSHAKE_ACK => {
                        let info = SessionInfo::from_envelope(&env);
                        *session.lock().await = info;
                    }
                    _ => {}
                }
            }
        });
    }

    fn next_msg_id(&self) -> u32 {
        self.msg_seq.fetch_add(1, Ordering::Relaxed)
    }

    pub fn subscribe(&self) -> PushReceiver {
        self.pushes.subscribe()
    }

    pub async fn session(&self) -> SessionInfo {
        self.session.lock().await.clone()
    }

    #[allow(dead_code)]
    pub async fn is_bulk_bound(&self) -> bool {
        *self.bulk_bound.lock().await
    }

    /// 发送请求并等待同 msg_id 的响应
    pub async fn request(&self, msg_type: u32, data: Value) -> Result<Envelope, String> {
        self.request_with_timeout(msg_type, data, self.request_timeout)
            .await
    }

    /// 同 [`Self::request`]，但可指定等待应答的时长
    pub async fn request_with_timeout(
        &self,
        msg_type: u32,
        data: Value,
        wait: Duration,
    ) -> Result<Envelope, String> {
        let msg_id = self.next_msg_id();
        let env = Envelope::request(msg_type, msg_id, data);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(msg_id, tx);

        if self
            .control_tx
            .send(Message::Text(env.to_text()))
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&msg_id);
            return Err("control 通道已关闭".to_string());
        }

        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => Err("响应通道被丢弃".to_string()),
            Err(_) => {
                self.pending.lock().await.remove(&msg_id);
                Err(format!("等待 {} 响应超时", msg_type + 1))
            }
        }
    }

    /// 发送请求，应答「可有可无」：等不到应答返回 `Ok(None)`，本体明确拒绝才返回 `Err`
    ///
    /// 用于 `4030` 这类**长耗时操作**：现场固件的 `4031` 常常比 `REQUEST_TIMEOUT` 慢，
    /// 或者干脆只靠 `5009 loc_state` 反馈进度。把它当普通请求等 8s 会把「本体正在载图」
    /// 误判成「定位未就绪」，所以这里只取「有没有被明确拒绝」这一个信息。
    pub async fn call_ack_optional(
        &self,
        msg_type: u32,
        data: Value,
        ack_wait: Duration,
    ) -> Result<Option<Value>, String> {
        match self.request_with_timeout(msg_type, data, ack_wait).await {
            Ok(reply) if reply.is_ok() => Ok(Some(reply.data)),
            Ok(reply) => Err(format!(
                "{msg_type} 失败: code={} {}",
                reply.code(),
                reply.message()
            )),
            // 超时 = 本体没回执，不代表被拒：交给 5009 状态流判断
            Err(e) if e.contains("响应超时") => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// 发送请求并要求 `code` / `status_code` 为 0，返回 data 本体
    pub async fn call(&self, msg_type: u32, data: Value) -> Result<Value, String> {
        let reply = self.request(msg_type, data).await?;
        if reply.is_ok() {
            Ok(reply.data)
        } else {
            Err(format!(
                "{} 失败: code={} {}",
                msg_type,
                reply.code(),
                reply.message()
            ))
        }
    }

    /// 发送单向消息（不等响应）
    ///
    /// 用于文档里没有响应类型的控制帧，典型是 `1304` 摇杆：没有 `1305`，
    /// 用 [`Self::call`] 发只能等满 8s 超时，还会在 `pending` 里留一个永远
    /// 等不到应答的 oneshot。
    pub async fn send(&self, msg_type: u32, data: Value) -> Result<(), String> {
        let env = Envelope::notify(msg_type, data);
        self.control_tx
            .send(Message::Text(env.to_text()))
            .await
            .map_err(|_| "control 通道已关闭".to_string())
    }

    /// 发送控制类请求（`1302` / `1310`），按「响应类型 = 请求类型 + 1」等回复
    ///
    /// 网关的 `1303` / `1311` 不回 `msg_id`，只能用类型关联；成功与否看
    /// `data.status_code`（见 [`Envelope::code`]）。
    ///
    /// 注：现场固件的 `1302` 实测**不回 `1303`**（连非法 cmd 都不回），遥控与
    /// 本体动作走 [`Self::send`]；这里留着给 `1310`（充电桩，有 `1311` 应答）这类
    /// 真正会回包的控制请求用。
    #[allow(dead_code)]
    pub async fn call_control(&self, msg_type: u32, data: Value) -> Result<Envelope, String> {
        let reply_type = msg_type + 1;
        let (tx, rx) = oneshot::channel();
        self.pending_types.lock().await.insert(reply_type, tx);

        let msg_id = self.next_msg_id();
        let env = Envelope::request(msg_type, msg_id, data);
        if self
            .control_tx
            .send(Message::Text(env.to_text()))
            .await
            .is_err()
        {
            self.pending_types.lock().await.remove(&reply_type);
            return Err("control 通道已关闭".to_string());
        }

        match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(reply)) => {
                if reply.is_ok() {
                    Ok(reply)
                } else {
                    Err(format!(
                        "{} 失败: status_code={} {}",
                        msg_type,
                        reply.code(),
                        reply.message()
                    ))
                }
            }
            Ok(Err(_)) => Err("响应通道被丢弃".to_string()),
            Err(_) => {
                self.pending_types.lock().await.remove(&reply_type);
                Err(format!("等待 {} 响应超时", reply_type))
            }
        }
    }

    /// 向 bulkdata 通道发送（当前协议中外部端不向 bulkdata 写业务数据，保留备用）
    #[allow(dead_code)]
    pub async fn send_bulk(&self, env: &Envelope) -> Result<(), String> {
        if !self.is_bulk_bound().await {
            return Err("bulkdata 尚未绑定".to_string());
        }
        self.bulkdata_tx
            .send(Message::Text(env.to_text()))
            .await
            .map_err(|_| "bulkdata 通道已关闭".to_string())
    }

    /// 确保持有 RoamerX 控制权（`1204`），已持有时直接返回
    ///
    /// 协议 §「4000 ~ 5999 自由数据透传区间」：gateway 的默认 owner 为 UINT32_MAX 时
    /// **不转发任何非白名单 4000~5999 消息**（白名单只有 4012/4022/5010）；owner 存在时
    /// 仅 owner 的消息会被处理。所以 `4030`（载图定位）、`4044`、`5000`（建任务）这些
    /// 都必须先拿到控制权，否则会被网关静默丢弃（表现为等应答超时）。
    pub async fn ensure_control(&self) -> Result<(), String> {
        self.take_control().await
    }

    /// 获取 RoamerX 控制权（1204），发送 4000~5999 前调用
    pub async fn take_control(&self) -> Result<(), String> {
        if *self.control_owned.lock().await {
            return Ok(());
        }
        let msg_id = self.next_msg_id();
        let env = Envelope::request(T_TAKE_CONTROL, msg_id, empty());
        self.control_tx
            .send(Message::Text(env.to_text()))
            .await
            .map_err(|_| "control 通道已关闭".to_string())?;
        // 1205 为通知，不走 msg_id 关联，等待状态标志由 status_watch 更新
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            if *self.control_owned.lock().await {
                return Ok(());
            }
        }
        Err("获取 RoamerX 控制权超时".to_string())
    }
}

async fn handle_incoming(
    env: &Envelope,
    pending: &Arc<Mutex<HashMap<u32, ReplySender>>>,
    pending_types: &Arc<Mutex<HashMap<u32, TypeReplySender>>>,
    pushes: &broadcast::Sender<Envelope>,
    session: &Arc<Mutex<SessionInfo>>,
) {
    // 响应优先按 msg_id 派发；通知/推送走广播
    if let Some(msg_id) = env.head.msg_id
        && let Some(tx) = pending.lock().await.remove(&msg_id)
    {
        let _ = tx.send(env.clone());
        return;
    }
    // 控制类响应（1303/1311）不带 msg_id，按响应类型派发
    // （`pending_types` 里只有登记过的控制类响应类型，不会抢走推送）
    if let Some(tx) = pending_types.lock().await.remove(&env.head.msg_type) {
        let _ = tx.send(env.clone());
        return;
    }
    if env.head.msg_type == T_HANDSHAKE_ACK {
        *session.lock().await = SessionInfo::from_envelope(env);
    }
    let _ = pushes.send(env.clone());
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 规范化路径，保证以 `/` 开头
fn normalize_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// 从主机配置推导 base url（`192.168.168.100` / `ws://ip:10010` / `ws://ip:10010/control` 均可）
pub fn host_base(host: &str) -> String {
    let trimmed = host.trim().trim_end_matches('/');
    if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        match trimmed.find("://") {
            Some(idx) => {
                let (scheme, rest) = trimmed.split_at(idx + 3);
                match rest.find('/') {
                    Some(slash) => format!("{scheme}{}", &rest[..slash]),
                    None => trimmed.to_string(),
                }
            }
            None => trimmed.to_string(),
        }
    } else {
        format!("ws://{trimmed}:10010")
    }
}

/// control / bulkdata 两个连接地址
pub fn endpoint_urls(host: &str) -> (String, String) {
    let base = host_base(host);
    if base.contains("/control") || base.contains("/bulkdata") {
        return (
            base.replace("/bulkdata", "/control"),
            base.replace("/control", "/bulkdata"),
        );
    }
    (format!("{base}/control"), format!("{base}/bulkdata"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_endpoints_from_bare_ip() {
        let (c, b) = endpoint_urls("192.168.168.100");
        assert_eq!(c, "ws://192.168.168.100:10010/control");
        assert_eq!(b, "ws://192.168.168.100:10010/bulkdata");
    }

    #[test]
    fn builds_endpoints_from_full_url() {
        let (c, b) = endpoint_urls("ws://10.0.0.5:10010");
        assert_eq!(c, "ws://10.0.0.5:10010/control");
        assert_eq!(b, "ws://10.0.0.5:10010/bulkdata");
    }

    #[test]
    fn handles_control_suffix() {
        let (c, b) = endpoint_urls("ws://10.0.0.5:10010/control");
        assert_eq!(c, "ws://10.0.0.5:10010/control");
        assert_eq!(b, "ws://10.0.0.5:10010/bulkdata");
    }
}
