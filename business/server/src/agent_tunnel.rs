//! 板端 Agent 隧道：平台 ⇄ 设备客户端 ⇄ 本机 Agent
//!
//! 官方部署有两个硬约束（见 `docs/backpack-video-integration.md` §3.5）：
//!
//! 1. 业务平台在公网，背包 Agent（`192.168.x.x:8000`）只在现场内网 —— 服务端直连超时；
//! 2. 操作员电脑也未必与背包同网段（换 WiFi、出差远程、走公网入口），Chrome 的
//!    Local Network Access 还会额外拦掉「公网页面 → 内网地址」。
//!
//! 但设备客户端本来就跑在背包那块板子上（`127.0.0.1:8000` 一定通），而且有一条到
//! 平台的常驻设备 WS。于是把 `/api/backpack/*` 的全套能力挂到这条已有信道上：
//!
//! ```text
//! 浏览器 ──GET /api/backpack/status────────▶ 平台 ──agent_request──▶ 客户端 ──HTTP──▶ Agent
//! 浏览器 ◀──SSE /api/backpack/events──────── 平台 ◀──agent_event──── 客户端 ◀──SSE── Agent
//! 浏览器 ──POST /api/backpack/detect/start──▶ 平台 ──agent_request──▶ 客户端 ──HTTP──▶ Agent
//! ```
//!
//! 设计取舍：
//!
//! - **白名单**：只放行背包页真正用到的 4 个只读接口 + 3 个 AI 动作。板端 Agent 没有
//!   鉴权，做成通用代理等于把现场内网的任意 HTTP 能力暴露给控制台账号；
//! - **不缓存**：页面本来就 8s 轮询一次，服务端按需问一次客户端（4s 超时）比维护一份
//!   迟早过期的快照更省心，也天然把「客户端掉线 / Agent 挂了」表现成 `reachable=false`；
//! - **事件只转 detect 类**：`telemetry` / `lingsi` / `button` 页面暂不用，原样转发
//!   只会白烧板端上行带宽。

use crate::gateway::{self, DeviceConnections};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};
use tracing::{debug, info, warn};

/// 允许经隧道转发的 Agent 只读接口
///
/// 只放行背包页真正要用的：板端 Agent 没有鉴权，多放一条就是多一条内网 HTTP 能力。
/// 新增前先问：这个接口只读吗？页面真的消费吗？
pub const GET_PATHS: [&str; 4] = [
    "/api/cameras",
    "/api/relay/status",
    "/api/detect/tasks",
    "/api/detect/status",
];

/// 允许经隧道转发的 AI 启停动作（对应 `/api/detect/{action}`）
pub const ACTIONS: [&str; 3] = ["start", "stop", "config"];

/// 事件流里值得往平台转的类型：页面只消费这四种（框、清框、状态变化、异常）
const FORWARDED_EVENTS: [&str; 4] = ["detect", "detect_clear", "detect_state", "detect_error"];

/// 隧道事件：带上来源设备，SSE 才能只推「正在看的那一台」
///
/// 多背包时两台板子的 Agent 事件会混在同一条广播里：不过滤的话，
/// 看 RD-002 的页面会画上 RD-001 的框（`detect` 里只有 `cam`，没有设备）。
#[derive(Debug, Clone)]
pub struct TunnelEvent {
    /// 来源设备编号（`agent_hello` 登记的那台）；`None` = 不知道来源
    pub device: Option<String>,
    pub text: String,
}

/// 板端 Agent 隧道的服务端侧状态
///
/// 一个进程一份：设备上线时打招呼登记（[`AgentTunnel::hello`]），页面请求进来时
/// 由这里挑一台设备发 `agent_request` 并等回执，检出事件由 [`AgentTunnel::publish`]
/// 广播给 `/api/backpack/events` 的订阅者。
pub struct AgentTunnel {
    /// 报过到的设备：(设备码, Agent 基址)，最近上报的排在末尾
    agents: RwLock<Vec<(String, String)>>,
    /// 请求表：requestId → 回执通道（与指令回执同一套「登记 → 唤醒」模型）
    waiters: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    /// 检出事件广播（SSE 订阅者）
    events: broadcast::Sender<TunnelEvent>,
    /// requestId 自增段：同一进程内唯一即可，不需要全局唯一
    seq: AtomicU64,
}

impl Default for AgentTunnel {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentTunnel {
    pub fn new() -> Self {
        // 通道不必大：一条 detect 事件几十字节，订阅者通常只有 1~2 个页面
        let (events, _) = broadcast::channel(128);
        Self {
            agents: RwLock::new(Vec::new()),
            waiters: Mutex::new(HashMap::new()),
            events,
            seq: AtomicU64::new(1),
        }
    }

    /// 设备客户端上线 / Agent 地址变化：登记「这台设备能代调 Agent」
    pub async fn hello(&self, device: &str, base: &str) {
        let mut agents = self.agents.write().await;
        let before = agents.len();
        agents.retain(|(code, _)| code != device);
        agents.push((device.to_string(), base.trim_end_matches('/').to_string()));
        if before == agents.len() {
            info!("板端 Agent 隧道已登记：设备 {device} → {base}");
        }
    }

    /// 隧道当前指向的设备：配了 `[backpack] tunnel_device` 就只认它，
    /// 否则取「最近上报且仍然在线」的那台（单背包部署不用配任何东西）。
    pub async fn device(
        &self,
        prefer: Option<&str>,
        connections: &DeviceConnections,
    ) -> Option<String> {
        let agents = self.agents.read().await;
        if let Some(code) = prefer {
            return agents
                .iter()
                .any(|(device, _)| device == code)
                .then(|| code.to_string());
        }
        let online = connections.read().await;
        agents
            .iter()
            .rev()
            .find(|(device, _)| online.contains_key(device))
            .map(|(device, _)| device.clone())
    }

    /// 设备登记的 Agent 基址（展示用：`status.agent.base` 要显示实际地址）
    pub async fn base_of(&self, device: &str) -> Option<String> {
        self.agents
            .read()
            .await
            .iter()
            .find(|(code, _)| code == device)
            .map(|(_, base)| base.clone())
    }

    /// 隧道是否可用（不真的发请求，只判断有没有在线设备登记过 Agent）
    pub async fn available(&self, prefer: Option<&str>, connections: &DeviceConnections) -> bool {
        self.device(prefer, connections).await.is_some()
    }

    /// 经隧道取一个只读接口（白名单见 [`GET_PATHS`]）
    pub async fn get(
        &self,
        prefer: Option<&str>,
        connections: &DeviceConnections,
        path: &str,
        timeout: Duration,
    ) -> Result<Value, String> {
        if !GET_PATHS.contains(&path) {
            return Err(format!("隧道不支持该接口：{path}"));
        }
        self.call(
            prefer,
            connections,
            json!({"action": "get", "path": path}),
            timeout,
        )
        .await
    }

    /// 经隧道执行一次 AI 动作（`start` / `stop` / `config`）
    pub async fn act(
        &self,
        prefer: Option<&str>,
        connections: &DeviceConnections,
        action: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        if !ACTIONS.contains(&action) {
            return Err(format!("隧道不支持该动作：{action}"));
        }
        self.call(
            prefer,
            connections,
            json!({"action": action, "body": body}),
            timeout,
        )
        .await
    }

    /// 发一条 `agent_request` 并等客户端回执
    async fn call(
        &self,
        prefer: Option<&str>,
        connections: &DeviceConnections,
        extra: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let Some(device) = self.device(prefer, connections).await else {
            return Err(
                "没有可用的设备客户端：确认板端客户端在线，且它连得上本机 Agent（`[ai] enabled`）"
                    .to_string(),
            );
        };

        let request_id = format!(
            "agent-{}-{}",
            std::process::id(),
            self.seq.fetch_add(1, Ordering::Relaxed)
        );
        let (tx, rx) = oneshot::channel();
        self.waiters.lock().await.insert(request_id.clone(), tx);

        let mut payload = extra;
        if let Value::Object(map) = &mut payload {
            map.insert("requestId".to_string(), json!(request_id));
        }
        if !gateway::send_message_to_device(connections, &device, "agent_request", payload).await {
            self.waiters.lock().await.remove(&request_id);
            return Err(format!("设备 {device} 不在线，隧道无法转发"));
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => {
                if reply.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                    Ok(reply.get("payload").cloned().unwrap_or(Value::Null))
                } else {
                    Err(reply
                        .get("error")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                        .unwrap_or("板端 Agent 未返回原因")
                        .to_string())
                }
            }
            Ok(Err(_)) => Err(format!("设备 {device} 的隧道已断开")),
            Err(_) => {
                self.waiters.lock().await.remove(&request_id);
                Err(format!(
                    "设备 {device} {}s 内没有回执（板端客户端可能卡住）",
                    timeout.as_secs()
                ))
            }
        }
    }

    /// 客户端回执：唤醒对应的等待方（找不到说明已超时，丢弃即可）
    pub async fn complete(&self, request_id: &str, reply: Value) {
        let Some(tx) = self.waiters.lock().await.remove(request_id) else {
            debug!("隧道回执迟到（请求 {request_id} 已超时）");
            return;
        };
        let _ = tx.send(reply);
    }

    /// 广播一帧 Agent 事件；页面没开背包页时直接丢掉（不缓存、不补发）
    ///
    /// `device` 是发这条事件的设备客户端的设备码（`gateway` 里那一条连接的
    /// `device_id`）：SSE 订阅者按它过滤，多背包才不会串台。
    pub fn publish(&self, device: Option<&str>, event: &Value) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        if !FORWARDED_EVENTS.contains(&kind) {
            return;
        }
        if self.events.receiver_count() == 0 {
            return;
        }
        let Ok(text) = serde_json::to_string(event) else {
            warn!("板端事件序列化失败，丢弃：{event}");
            return;
        };
        // 没有订阅者时 send 会报错，属正常情况
        let _ = self.events.send(TunnelEvent {
            device: device.map(str::to_string),
            text,
        });
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TunnelEvent> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelists_are_narrow_on_purpose() {
        // Agent 无鉴权：多放一条就是多一条内网 HTTP 能力
        assert_eq!(GET_PATHS.len(), 4);
        assert!(GET_PATHS.contains(&"/api/detect/status"));
        assert!(!GET_PATHS.contains(&"/api/whep/dogcam"));
        assert_eq!(ACTIONS, ["start", "stop", "config"]);
    }

    #[tokio::test]
    async fn unknown_path_is_rejected_without_touching_the_device() {
        let tunnel = AgentTunnel::new();
        let connections = gateway::new_device_connections();
        let error = tunnel
            .get(
                None,
                &connections,
                "/api/control/led",
                Duration::from_millis(50),
            )
            .await
            .expect_err("白名单外的路径必须直接拒绝");
        assert!(error.contains("不支持"), "{error}");
        let error = tunnel
            .act(
                None,
                &connections,
                "plan/clear",
                json!({}),
                Duration::from_millis(50),
            )
            .await
            .expect_err("白名单外的动作必须直接拒绝");
        assert!(error.contains("不支持"), "{error}");
    }

    #[tokio::test]
    async fn no_device_means_actionable_error() {
        let tunnel = AgentTunnel::new();
        let connections = gateway::new_device_connections();
        let error = tunnel
            .get(
                None,
                &connections,
                "/api/cameras",
                Duration::from_millis(50),
            )
            .await
            .expect_err("没有设备时不能挂住请求");
        assert!(error.contains("设备客户端"), "{error}");
    }

    #[tokio::test]
    async fn only_detect_events_are_broadcast() {
        let tunnel = AgentTunnel::new();
        let mut rx = tunnel.subscribe();
        tunnel.publish(Some("RD-001"), &json!({"type": "telemetry", "battery": 80}));
        tunnel.publish(Some("RD-001"), &json!({"type": "detect", "boxes": []}));
        // `recv` 只能拿到被放行的那一条：遥测不进背包页的 SSE
        let event = rx.try_recv().expect("detect 事件应被转发");
        assert!(event.text.contains("\"detect\""));
        assert_eq!(event.device.as_deref(), Some("RD-001"));
        assert!(rx.try_recv().is_err());
    }

    /// 多背包：事件必须带来源设备，SSE 才敢按设备过滤。
    #[tokio::test]
    async fn events_carry_the_source_device() {
        let tunnel = AgentTunnel::new();
        let mut rx = tunnel.subscribe();
        tunnel.publish(Some("RD-001"), &json!({"type": "detect", "boxes": []}));
        tunnel.publish(None, &json!({"type": "detect_clear"}));
        assert_eq!(rx.try_recv().unwrap().device.as_deref(), Some("RD-001"));
        assert_eq!(
            rx.try_recv().unwrap().device,
            None,
            "不知道来源时不能编一个设备"
        );
    }
}
