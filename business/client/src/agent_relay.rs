//! 板端 Agent 隧道（设备端）
//!
//! 正式环境的业务平台在公网、背包 Agent（`127.0.0.1:8000`）只在现场内网，
//! 操作员浏览器也未必与背包同网段（换 WiFi / 出差远程 / 公网入口）。本进程就跑在
//! 背包那块板子上，于是把 Agent 的能力借给平台：
//!
//! ```text
//! 平台 ──agent_request──▶ 本进程 ──HTTP──▶ 127.0.0.1:8000
//! 平台 ◀──agent_result─── 本进程 ◀──HTTP──┘
//! 平台 ◀──agent_event──── 本进程 ◀──SSE─── /api/events
//! ```
//!
//! 与 [`crate::ai_patrol`] 的分工：那边是**巡检到点**的自动识别（决策在服务端，
//! 窗口结束就收），这边是**操作员在实时视频页手动开的识别**（模型目录、运行态、
//! 启停、检出框），两者都调同一个 Agent，Agent 本身单路单任务。
//!
//! 三条硬规则：
//!
//! - **白名单**：只转发 4 个背包页只读接口 + 电话运行态 + 3 个 AI 动作，
//!   别把它变成通用出口；
//! - **连不上就不打招呼**：`agent_hello` 只在 `/api/cameras` 真的通时才发，
//!   平台侧自然显示「不可达」，不会把请求堆到黑洞里；
//! - **事件只转检测类**：遥测 / 按键 / 语音暂时没人消费，原样转发只白烧上行带宽。

use crate::wire::{WsMessage, send};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// 允许转发的 Agent 只读接口（与服务端 `agent_tunnel.rs` 的白名单一一对应）
const GET_PATHS: [&str; 5] = [
    "/api/cameras",
    "/api/relay/status",
    "/api/detect/tasks",
    "/api/detect/status",
    // 电话（对讲）运行态：页面要显示「通话中 / 对端」与板端网段
    "/api/phone/status",
];

/// 允许转发的 AI 动作
const ACTIONS: [&str; 3] = ["start", "stop", "config"];

/// 值得往平台转的事件：页面只消费这四种
const FORWARDED_EVENTS: [&str; 4] = ["detect", "detect_clear", "detect_state", "detect_error"];

/// 探测超时：Agent 与客户端同机，3s 足够
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// 启停识别要等模型加载（板端实测 1~3s），与巡检窗口同一档次
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);
/// 事件流断开后的重连间隔
const SSE_RETRY: Duration = Duration::from_secs(5);
/// SSE 单条缓冲上限：超过说明流里没有 `\n\n`，丢弃重来
const SSE_BUF_LIMIT: usize = 64 * 1024;

/// 板端 Agent 隧道
pub struct AgentRelay {
    /// Agent 基地址（默认 `http://127.0.0.1:8000`）
    agent: String,
    /// 总开关：与航点 AI 共用 `[ai] enabled`，关掉就既不发 hello 也不转发事件
    enabled: bool,
    probe: reqwest::Client,
    action: reqwest::Client,
    stream: reqwest::Client,
    /// 已经向平台登记过（`agent_hello` 成功）：板端重启后 Agent 可能比客户端晚起来，
    /// 没登记就一直由重试任务补发，页面那边才不会一直显示「不可达」
    registered: AtomicBool,
}

impl AgentRelay {
    pub fn new(agent: impl Into<String>, enabled: bool) -> Self {
        let agent = agent.into().trim().trim_end_matches('/').to_string();
        Self {
            agent,
            enabled,
            probe: reqwest::Client::builder()
                .timeout(PROBE_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            action: reqwest::Client::builder()
                .timeout(ACTION_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            // SSE 是长连接：只限建连时间，不能设总超时
            stream: reqwest::Client::builder()
                .connect_timeout(PROBE_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            registered: AtomicBool::new(false),
        }
    }

    /// 是否已经向平台登记过隧道
    pub fn registered(&self) -> bool {
        self.registered.load(Ordering::Relaxed)
    }

    /// 探测 + 登记：Agent 通才发 `agent_hello`（幂等，成功过就不再发）
    pub async fn announce(&self, out_tx: &mpsc::Sender<WsMessage>) -> bool {
        if self.registered() {
            return true;
        }
        let Some(payload) = self.hello().await else {
            return false;
        };
        send(out_tx, "agent_hello", payload).await;
        self.registered.store(true, Ordering::Relaxed);
        info!(
            "板端 Agent 隧道已登记：{}（背包页的模型目录 / 运行态 / AI 启停可跨网段代调）",
            self.agent
        );
        true
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// 上线打招呼：Agent 真的连得上才发（平台据此判断隧道是否可用）
    pub async fn hello(&self) -> Option<Value> {
        if !self.enabled {
            return None;
        }
        match self.get_json("/api/cameras").await {
            Ok(_) => Some(json!({ "base": self.agent })),
            Err(error) => {
                debug!("板端 Agent 未就绪，暂不登记隧道（{}）: {error}", self.agent);
                None
            }
        }
    }

    /// 平台的 `agent_request`：做完回一条 `agent_result`（页面在等这个结果）
    pub async fn on_request(&self, out_tx: &mpsc::Sender<WsMessage>, payload: &Value) {
        let request_id = payload
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if request_id.is_empty() {
            warn!("agent_request 缺少 requestId，忽略: {payload}");
            return;
        }
        if !self.enabled {
            send(
                out_tx,
                "agent_result",
                json!({
                    "requestId": request_id,
                    "ok": false,
                    "error": "板端客户端未启用 Agent 桥接（[ai] enabled = false）",
                }),
            )
            .await;
            return;
        }

        let action = payload
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = match action {
            "get" => self.handle_get(payload).await,
            other if ACTIONS.contains(&other) => {
                let body = payload.get("body").cloned().unwrap_or_else(|| json!({}));
                self.post_json(&format!("/api/detect/{other}"), &body).await
            }
            other => Err(format!("隧道不支持该动作：{other}")),
        };

        let reply = match result {
            Ok(payload) => json!({ "requestId": request_id, "ok": true, "payload": payload }),
            Err(error) => json!({ "requestId": request_id, "ok": false, "error": error }),
        };
        send(out_tx, "agent_result", reply).await;
    }

    async fn handle_get(&self, payload: &Value) -> Result<Value, String> {
        let path = payload
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !GET_PATHS.contains(&path) {
            return Err(format!("隧道不支持该接口：{path}"));
        }
        self.get_json(path).await
    }

    /// 常驻任务：订阅本机 Agent 事件流并转发给平台（调用方 abort 掉即可收工）
    pub async fn forward_events(&self, out_tx: mpsc::Sender<WsMessage>) {
        if !self.enabled {
            return;
        }
        loop {
            match self
                .stream
                .get(format!("{}/api/events", self.agent))
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    info!(
                        "板端 Agent 事件流已连接（{}/api/events），检测事件转发给平台",
                        self.agent
                    );
                    if let Err(error) = self.pump_events(&out_tx, response).await {
                        warn!("板端 Agent 事件流中断: {error}");
                    }
                }
                Ok(response) => warn!("板端 Agent 拒绝事件流：HTTP {}", response.status()),
                Err(error) => debug!("板端 Agent 事件流未连上（{error}），稍后重试"),
            }
            tokio::time::sleep(SSE_RETRY).await;
        }
    }

    async fn pump_events(
        &self,
        out_tx: &mpsc::Sender<WsMessage>,
        mut response: reqwest::Response,
    ) -> Result<(), String> {
        let mut buffer = String::new();
        loop {
            match response.chunk().await {
                Ok(Some(bytes)) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    if buffer.len() > SSE_BUF_LIMIT {
                        warn!("板端 Agent 事件流缓冲超过上限，丢弃重来");
                        buffer.clear();
                        continue;
                    }
                    for payload in crate::ai_patrol::take_sse_events(&mut buffer) {
                        let Ok(event) = serde_json::from_str::<Value>(&payload) else {
                            continue;
                        };
                        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
                        if !FORWARDED_EVENTS.contains(&kind) {
                            continue;
                        }
                        // 事件原样转发：中文标签、坐标、计数都由板端算好，服务端不改写
                        send(out_tx, "agent_event", event).await;
                    }
                }
                Ok(None) => return Ok(()),
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    async fn get_json(&self, path: &str) -> Result<Value, String> {
        let response = self
            .probe
            .get(format!("{}{path}", self.agent))
            .send()
            .await
            .map_err(|error| format!("请求失败: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "上游返回 {status}: {}",
                text.chars().take(160).collect::<String>()
            ));
        }
        serde_json::from_str(&text).map_err(|error| format!("响应不是合法 JSON: {error}"))
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value, String> {
        let response = self
            .action
            .post(format!("{}{path}", self.agent))
            .json(body)
            .send()
            .await
            .map_err(|error| format!("请求失败: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if payload.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(payload);
        }
        let reason = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(&text)
            .to_string();
        Err(if reason.trim().is_empty() {
            format!("上游返回 {status}")
        } else {
            reason
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitelists_match_the_server_side_tunnel() {
        // 两端白名单必须一致：服务端放行、客户端拒绝会出现「请求发出去了却回不来」
        assert_eq!(GET_PATHS.len(), 5);
        assert!(GET_PATHS.contains(&"/api/detect/tasks"));
        assert!(GET_PATHS.contains(&"/api/phone/status"));
        assert!(!GET_PATHS.contains(&"/api/control/led"));
        assert_eq!(ACTIONS, ["start", "stop", "config"]);
        assert!(FORWARDED_EVENTS.contains(&"detect"));
        assert!(!FORWARDED_EVENTS.contains(&"telemetry"));
    }

    #[tokio::test]
    async fn disabled_relay_does_not_register() {
        let relay = AgentRelay::new("http://127.0.0.1:1", false);
        assert!(relay.hello().await.is_none());
    }

    #[tokio::test]
    async fn unreachable_agent_is_not_announced() {
        // 127.0.0.1:1 一定连不上：平台上就该显示「不可达」，而不是登记后全超时
        let relay = AgentRelay::new("http://127.0.0.1:1", true);
        assert!(relay.hello().await.is_none());
    }
}

#[cfg(test)]
mod flow {
    //! 起一个假板端 Agent（HTTP + SSE），把「平台 → 客户端 → Agent」跑一遍：
    //! 打招呼、只读接口、AI 启停、检出事件转发。

    use super::*;
    use axum::Router;
    use axum::routing::{get, post};
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    const STATUS: &str =
        r#"{"running":true,"state":"running","task":"combined","default_cam":"dogcam"}"#;

    /// 假 Agent 收到的 AI 动作请求体（断言平台参数被原样带过来）
    type Calls = Arc<Mutex<Vec<(String, Value)>>>;

    async fn spawn_agent(calls: Calls) -> String {
        let app = Router::new()
            .route("/api/cameras", get(|| async { r#"{"cameras":{}}"# }))
            .route("/api/detect/status", get(|| async { STATUS }))
            .route(
                "/api/detect/start",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>,
                     axum::Json(body): axum::Json<Value>| async move {
                        calls.lock().unwrap().push(("start".to_string(), body));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/detect/stop",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>,
                     axum::Json(body): axum::Json<Value>| async move {
                        calls.lock().unwrap().push(("stop".to_string(), body));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/events",
                get(|| async {
                    // 一帧检出、一帧遥测：只有检出该转发给平台
                    concat!(
                        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"boxes\":[],\"counts\":{}}\n\n",
                        "data: {\"type\":\"telemetry\",\"battery\":80}\n\n",
                    )
                }),
            )
            .with_state(calls);
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("假 Agent 运行失败");
        });
        base
    }

    /// 等一条指定类型的事件（跳过心跳注释行等噪声）
    async fn expect_kind(rx: &mut mpsc::Receiver<WsMessage>, kind: &str) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let msg = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("等 {kind} 超时"))
                .expect("通道已关闭");
            if msg.msg_type == kind {
                return msg.payload;
            }
        }
    }

    #[tokio::test]
    async fn hello_requests_and_events_round_trip() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let base = spawn_agent(calls.clone()).await;
        let relay = AgentRelay::new(base.clone(), true);
        let (tx, mut rx) = mpsc::channel::<WsMessage>(16);

        // ① 打招呼：Agent 通才发，平台据此判断隧道可用
        let hello = relay.hello().await.expect("Agent 通时必须打招呼");
        assert_eq!(hello["base"], json!(base));

        // ② 只读接口
        relay
            .on_request(
                &tx,
                &json!({"requestId": "r1", "action": "get", "path": "/api/detect/status"}),
            )
            .await;
        let reply = expect_kind(&mut rx, "agent_result").await;
        assert_eq!(reply["requestId"], json!("r1"));
        assert_eq!(reply["ok"], json!(true));
        assert_eq!(reply["payload"]["running"], json!(true));

        // ③ 白名单外的接口：直接拒绝，别把它变成通用代理
        relay
            .on_request(
                &tx,
                &json!({"requestId": "r2", "action": "get", "path": "/api/control/led"}),
            )
            .await;
        let reply = expect_kind(&mut rx, "agent_result").await;
        assert_eq!(reply["ok"], json!(false));
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or_default()
                .contains("不支持"),
            "错误要说清是白名单拒绝：{reply}"
        );

        // ④ AI 动作：body 原样带到 Agent
        relay
            .on_request(
                &tx,
                &json!({
                    "requestId": "r3",
                    "action": "start",
                    "body": {"task": "combined", "labels": ["fire"], "conf": 0.4, "cams": ["cam1"], "fps": 5},
                }),
            )
            .await;
        let reply = expect_kind(&mut rx, "agent_result").await;
        assert_eq!(reply["ok"], json!(true));
        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "start");
        assert_eq!(calls[0].1["cams"], json!(["cam1"]));
        assert_eq!(calls[0].1["conf"], json!(0.4));

        // ⑤ 事件转发：detect 上去，telemetry 丢掉
        let forward = tokio::spawn({
            let relay = Arc::new(relay);
            let tx = tx.clone();
            async move { relay.forward_events(tx).await }
        });
        let event = expect_kind(&mut rx, "agent_event").await;
        assert_eq!(event["type"], json!("detect"));
        assert_eq!(event["cam"], json!("dogcam"));
        forward.abort();
    }

    #[tokio::test]
    async fn disabled_relay_refuses_requests() {
        let relay = AgentRelay::new("http://127.0.0.1:1", false);
        let (tx, mut rx) = mpsc::channel::<WsMessage>(4);
        relay
            .on_request(
                &tx,
                &json!({"requestId": "r9", "action": "get", "path": "/api/cameras"}),
            )
            .await;
        let reply = expect_kind(&mut rx, "agent_result").await;
        assert_eq!(reply["ok"], json!(false));
        assert!(
            reply["error"]
                .as_str()
                .unwrap_or_default()
                .contains("enabled"),
            "要写清是开关关掉了：{reply}"
        );
    }
}
