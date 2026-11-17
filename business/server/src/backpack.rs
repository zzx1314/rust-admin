//! 数据采集背包（板端 Agent）同源代理
//!
//! 背包 Demo 的 Agent 是 FastAPI（`http://<板IP>:8000`），没有 CORS 中间件；
//! 平台前端在 `:5273`/`:3100` 直接 `fetch` 会被浏览器拦掉。所以 **REST 与 SSE 全部
//! 由业务平台转一层**，页面只跟同源 `/api/backpack/*` 打交道：
//!
//! ```text
//! 浏览器 ──/api/backpack/*──▶ 业务平台（本模块） ──HTTP/SSE──▶ 背包 Agent
//! 浏览器 ──WHEP(信令+媒体)──────────────────────────────────▶ mediamtx :8889
//! ```
//!
//! 视频不走平台中继：现场本来就是「浏览器与背包在同一局域网」，多一层
//! 只能增加延时。浏览器侧 WHEP 基址由 `GET /api/backpack/status` 返回
//! （`webrtc.base`），跨网段部署时再按 README 的 P1 方案加中继。
//!
//! 三条上游通道，按可用性依次退让（三者对页面都是同一个 `mode` 语义）：
//!
//! | 通道 | 什么时候用 | 谁连 Agent |
//! | --- | --- | --- |
//! | `direct` | `ROBODOG_BACKPACK_BROWSER_URL` 已配（现场入口，浏览器在园区内网） | 浏览器 |
//! | `proxy` | `ROBODOG_BACKPACK_URL` 已配且服务端连得通（平台与背包同网段） | 业务服务 |
//! | `tunnel` | 以上都不通，但板端设备客户端在线（`agent_tunnel.rs`） | 设备客户端代调 |
//!
//! 页面在 `direct` 模式下会自己探测 Agent；探测失败且后端报 `tunnel.available`
//! 时，前端带 `?transport=tunnel` 再拉一次，走本文件的隧道分支——这样「换了 WiFi /
//! 出差远程 / 公网入口」都不用再回现场入口。
//!
//! 这里**只读不改**：不做任何业务落库，AI 事件写库留给告警中心那一期。

use crate::app::AppState;
use crate::auth::Claims;
use crate::config::Config;
use crate::error::{ApiResponse, AppError};
use axum::Json;
use axum::body::Body;
use axum::extract::{Extension, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use tracing::debug;

/// 聚合查询的单次上游超时。Agent 的 `/api/detect/tasks` 会扫目录，3s 足够；
/// 超过就按「背包不可达」上报，页面立刻给结论，不转圈。
const API_TIMEOUT: Duration = Duration::from_secs(4);

/// 会话地址前缀：Agent 的 `/api/whep/*` 会把 mediamtx 的会话地址改写成
/// 自己的同源路径，平台不再包一层，所以这里只需要知道 path 形态即可。
const AI_KEYS: [&str; 5] = ["dogcam", "cam0", "cam1", "cam2", "cam3"];

/// 隧道上做一次 AI 动作要等多久：板端要加载模型（实测 1~3s，冷启动更久），
/// 再加上「平台 → 客户端 → Agent」两跳，按直连动作的同一档次留足。
const TUNNEL_ACTION_TIMEOUT: Duration = Duration::from_secs(70);

/// `GET /api/backpack/status` 的查询参数
#[derive(Debug, Default, Deserialize)]
pub struct StatusQuery {
    /// `tunnel` = 强制走设备客户端隧道。
    ///
    /// 直连模式下浏览器自己探测 Agent，只有它能判断「我能不能连上板子」；
    /// 探不通时页面带这个参数重试，后端改走隧道返回同一份聚合结果。
    transport: Option<String>,
}

pub(crate) fn api_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(API_TIMEOUT)
        .build()
        .map_err(|error| AppError::Internal(format!("构建 HTTP 客户端失败: {error}")))
}

/// AI 启停用更长超时：Agent 加载模型最多等 30s，板端画框启动还有 8s 确认窗口，
/// 4s 的聚合超时会把「正在启动」直接判成失败。
fn action_client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| AppError::Internal(format!("构建 HTTP 客户端失败: {error}")))
}

pub(crate) fn stream_client() -> Result<reqwest::Client, AppError> {
    // SSE 是长连接：**不能设总超时**，只限制建连时间，否则 4s 后事件流被切断，
    // 页面看起来像「识别框不刷新」。
    reqwest::Client::builder()
        .connect_timeout(API_TIMEOUT)
        .build()
        .map_err(|error| AppError::Internal(format!("构建 SSE 客户端失败: {error}")))
}

/// 未配置时的统一提示：说清配哪里，而不是抛 500。
pub(crate) fn agent_base(config: &Config) -> Result<&str, AppError> {
    config.backpack_base_url.as_deref().ok_or_else(|| {
        AppError::Upstream(
            "本部署未接入数据采集背包：请设置 ROBODOG_BACKPACK_URL（板端 Agent 地址，如 \
             http://192.168.234.200:8000）后重启业务服务"
                .to_string(),
        )
    })
}

pub(crate) async fn fetch_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("请求失败: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("读取响应失败: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "上游返回 {status}: {}",
            text.chars().take(180).collect::<String>()
        ));
    }
    serde_json::from_str(&text).map_err(|error| format!("响应不是合法 JSON: {error}"))
}

/// 6 路视频源的归一化清单
///
/// Agent 的 `/api/cameras` 里 `cameras` 是 map（cam0~cam3 + dogcam），红外单独一段。
/// 前端需要**固定顺序 + 固定字段**，所以在平台侧就摊平成数组，页面不再猜键名。
fn sources_of(cameras: &Value, relay: Option<&Value>) -> Vec<Value> {
    let relay_enabled = relay
        .and_then(|r| r.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let relay_running = relay
        .and_then(|r| r.get("running"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let title_of = |key: &str| -> String {
        cameras
            .get("cameras")
            .and_then(|c| c.get(key))
            .and_then(|c| c.get("title"))
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_string()
    };
    let stream_of = |key: &str| -> String {
        cameras
            .get("cameras")
            .and_then(|c| c.get(key))
            .and_then(|c| c.get("stream"))
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_string()
    };

    let mut list = Vec::new();
    for key in AI_KEYS {
        let is_relay = key == "dogcam";
        // 未配置源时 Agent 会把这个键留着但不推流，前端应置灰并写清恢复方式
        let available = !is_relay || relay_enabled;
        let hint = if !is_relay {
            String::new()
        } else if !relay_enabled {
            "板端未配置机器狗前置 RTSP 源（DOG_RTSP_URL 或 backend/dog_rtsp_url），配置并重启 Agent 后可用"
                .to_string()
        } else if !relay_running {
            "已配置源但当前没有收到机器狗推流：检查 3588 是否在推、网线是否通".to_string()
        } else {
            String::new()
        };
        list.push(json!({
            "key": key,
            "label": title_of(key),
            "stream": stream_of(key),
            "kind": if is_relay { "relay" } else { "usb" },
            "ai": true,
            "available": available,
            "hint": hint,
        }));
    }

    let infrared_stream = cameras
        .get("infrared")
        .and_then(|i| i.get("stream"))
        .and_then(Value::as_str)
        .unwrap_or("dog-ipcam");
    list.push(json!({
        "key": "infrared",
        "label": "红外",
        // 红外是现场已有的 mediamtx path，名字与摄像头的 stream 不共用一套
        "stream": infrared_stream,
        "kind": "infrared",
        "ai": false,
        "available": true,
        "hint": "红外通道不参与 AI 识别（热成像不具备可见光模型特征）",
    }));
    list
}

/// 四个上游响应的聚合结果（各自带原因，任何一个失败都不影响其余）
struct Aggregate {
    cameras: Result<Value, String>,
    relay: Result<Value, String>,
    tasks: Result<Value, String>,
    detect: Result<Value, String>,
}

impl Aggregate {
    /// 全通道都不通：把同一个原因铺到四个字段（页面只有四个 `*_error` 可显示）
    fn failed(reason: &str) -> Self {
        Self {
            cameras: Err(reason.to_string()),
            relay: Err(reason.to_string()),
            tasks: Err(reason.to_string()),
            detect: Err(reason.to_string()),
        }
    }
}

/// 服务端直连 Agent（4 个请求并发）
async fn aggregate_direct(base: &str) -> Result<Aggregate, AppError> {
    let client = api_client()?;
    let cameras_url = format!("{base}/api/cameras");
    let relay_url = format!("{base}/api/relay/status");
    let tasks_url = format!("{base}/api/detect/tasks");
    let detect_url = format!("{base}/api/detect/status");
    let (cameras, relay, tasks, detect) = tokio::join!(
        fetch_json(&client, &cameras_url),
        fetch_json(&client, &relay_url),
        fetch_json(&client, &tasks_url),
        fetch_json(&client, &detect_url),
    );
    Ok(Aggregate {
        cameras,
        relay,
        tasks,
        detect,
    })
}

/// 经设备客户端隧道取同样的 4 个接口
async fn aggregate_tunnel(state: &AppState) -> Aggregate {
    let prefer = state.config.backpack_tunnel_device.as_deref();
    let connections = &state.device_connections;
    let tunnel = &state.agent_tunnel;
    let (cameras, relay, tasks, detect) = tokio::join!(
        tunnel.get(prefer, connections, "/api/cameras", API_TIMEOUT),
        tunnel.get(prefer, connections, "/api/relay/status", API_TIMEOUT),
        tunnel.get(prefer, connections, "/api/detect/tasks", API_TIMEOUT),
        tunnel.get(prefer, connections, "/api/detect/status", API_TIMEOUT),
    );
    Aggregate {
        cameras,
        relay,
        tasks,
        detect,
    }
}

/// 把聚合结果摊成页面要的那一份 status：两条通道共用同一套归一化
fn status_body(
    state: &AppState,
    mode: &str,
    base: &str,
    aggregate: Aggregate,
    tunnel: &Value,
) -> Value {
    let reachable = aggregate.cameras.is_ok();
    let relay_value = aggregate.relay.as_ref().ok().cloned();
    let sources = aggregate
        .cameras
        .as_ref()
        .map(|cameras| sources_of(cameras, relay_value.as_ref()))
        .unwrap_or_default();

    let detect_status = aggregate.detect.as_ref().ok();
    let osd = detect_status
        .and_then(|s| s.get("osd"))
        .cloned()
        .unwrap_or(Value::Null);

    json!({
        "configured": true,
        "mode": mode,
        // 隧道是否在位：直连模式下页面据此决定「探不通时要不要回退到平台」
        "tunnel": tunnel,
        "agent": {
            "base": base,
            "reachable": reachable,
            "error": aggregate.cameras.as_ref().err().cloned().unwrap_or_default(),
        },
        // 浏览器直连的 WHEP 基址（页面用它拼 `<base>/<stream>/whep`）
        "webrtc": { "base": state.config.backpack_webrtc_base() },
        "sources": sources,
        "relay": relay_value.unwrap_or(Value::Null),
        "relay_error": aggregate.relay.as_ref().err().cloned().unwrap_or_default(),
        "detect": {
            "tasks": aggregate.tasks.as_ref().ok().cloned().unwrap_or(Value::Null),
            "tasks_error": aggregate.tasks.as_ref().err().cloned().unwrap_or_default(),
            "status": detect_status.cloned().unwrap_or(Value::Null),
            "status_error": aggregate.detect.as_ref().err().cloned().unwrap_or_default(),
        },
        "osd": {
            "enabled": osd.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            "ready": osd.get("ready").and_then(Value::as_bool).unwrap_or(false),
            "preview_stream": detect_status
                .and_then(|s| s.get("preview_stream"))
                .and_then(Value::as_str)
                .unwrap_or("preview"),
            "error": osd.get("error").and_then(Value::as_str).unwrap_or(""),
            "log_tail": osd.get("log_tail").cloned().unwrap_or(Value::Null),
        },
        // 与 Agent 单路约束保持一致，前端据此禁用多选
        "ai": {
            "single_cam": detect_status
                .and_then(|s| s.get("single_cam"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
            "allowed_cams": detect_status
                .and_then(|s| s.get("allowed_cams"))
                .cloned()
                .unwrap_or_else(|| json!(AI_KEYS)),
            "default_cam": detect_status
                .and_then(|s| s.get("default_cam"))
                .and_then(Value::as_str)
                .unwrap_or("dogcam"),
        },
        "label_format": detect_status
            .and_then(|s| s.get("label_format"))
            .cloned()
            .unwrap_or(Value::Null),
    })
}

/// 本部署完全没接背包：页面渲染「怎么配」的指引卡片，而不是一个红色报错
fn not_configured_body(tunnel: &Value, tunnel_error: &str) -> Value {
    json!({
        "configured": false,
        "mode": "none",
        "tunnel": tunnel,
        // 隧道在位只是「客户端登记过」，没有 Agent 地址仍然做不了事：两条提示都给
        "agent": {
            "base": "",
            "reachable": false,
            "error": format!(
                "未配置 ROBODOG_BACKPACK_URL（或让板端客户端用 `[ai] enabled` 代调）：{tunnel_error}"
            ),
        },
        "webrtc": { "base": Value::Null },
        "sources": [],
        "relay": Value::Null,
        "relay_error": "",
        "detect": {
            "tasks": Value::Null,
            "tasks_error": "",
            "status": Value::Null,
            "status_error": "",
        },
        "osd": {
            "enabled": false,
            "ready": false,
            "preview_stream": "preview",
            "error": "",
            "log_tail": Value::Null,
        },
        "ai": { "single_cam": true, "allowed_cams": json!(AI_KEYS), "default_cam": "dogcam" },
        "label_format": Value::Null,
    })
}

/// `GET /api/backpack/status` —— 页面首屏只发这一个请求
///
/// 聚合 Agent 的 `cameras` / `relay` / `detect/tasks` / `detect/status`：
/// 4 个请求并发，任何一个失败都**不影响其余**（各自的 `*_error` 字段说明原因）。
///
/// 通道按「浏览器直连 → 服务端直连 → 设备客户端隧道」依次退让，
/// `?transport=tunnel` 可跳过前两条（页面在直连模式探测失败时用它）。
pub async fn status(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let force_tunnel = query.transport.as_deref().map(str::trim) == Some("tunnel");
    let prefer = state.config.backpack_tunnel_device.as_deref();
    let tunnel_device = state
        .agent_tunnel
        .device(prefer, &state.device_connections)
        .await;
    let tunnel = json!({
        "available": tunnel_device.is_some(),
        "device": tunnel_device.clone().unwrap_or_default(),
    });

    // ① 直连模式：正式环境里平台在公网、背包在现场内网，业务服务连不上 Agent，
    // 但操作员浏览器与背包同网段。此时后端**不下发聚合结果**（它拿不到），
    // 只告诉页面「往哪个地址去调」，REST/SSE 由浏览器直连 Agent（已开 CORS）。
    if !force_tunnel && let Some(browser) = state.config.backpack_browser_url.as_deref() {
        return Ok(ApiResponse::ok(json!({
            "configured": true,
            "mode": "direct",
            // 直连探测失败时页面据此回退到 `?transport=tunnel`
            "tunnel": tunnel,
            "direct": {
                "apiBase": browser,
                "webrtcBase": state.config.backpack_webrtc_base(),
            },
            // 可达性由浏览器自己探测，这里不臆断
            "agent": { "base": browser, "reachable": true, "error": "" },
            "webrtc": { "base": state.config.backpack_webrtc_base() },
            "sources": [],
            "relay": Value::Null,
            "relay_error": "",
            "detect": {
                "tasks": Value::Null,
                "tasks_error": "",
                "status": Value::Null,
                "status_error": "",
            },
            "osd": {
                "enabled": false,
                "ready": false,
                "preview_stream": "preview",
                "error": "",
                "log_tail": Value::Null,
            },
            "ai": { "single_cam": true, "allowed_cams": json!(AI_KEYS), "default_cam": "dogcam" },
            "label_format": Value::Null,
        })));
    }

    // ② 代理模式：服务端直连（配了 base_url 且真能通）
    let mut direct_error = String::new();
    if !force_tunnel && let Some(base) = state.config.backpack_base_url.as_deref() {
        let aggregate = aggregate_direct(base).await?;
        if aggregate.cameras.is_ok() {
            return Ok(ApiResponse::ok(status_body(
                &state, "proxy", base, aggregate, &tunnel,
            )));
        }
        direct_error = aggregate
            .cameras
            .as_ref()
            .err()
            .cloned()
            .unwrap_or_default();
    }

    // ③ 隧道：板端客户端代调（平台与背包/浏览器都不在同网段时）
    let mut tunnel_error = "没有板端设备客户端在线".to_string();
    if let Some(device) = tunnel_device.as_deref() {
        let aggregate = aggregate_tunnel(&state).await;
        if aggregate.cameras.is_ok() {
            let base = state.agent_tunnel.base_of(device).await.unwrap_or_default();
            return Ok(ApiResponse::ok(status_body(
                &state, "tunnel", &base, aggregate, &tunnel,
            )));
        }
        tunnel_error = aggregate
            .cameras
            .as_ref()
            .err()
            .cloned()
            .unwrap_or_default();
    }

    // ④ 都没通：配过 base_url 的按「不可达」上报（现场要能看出是链路问题），
    // 完全没配的按「未接入」上报（页面给配置指引）
    if let Some(base) = state.config.backpack_base_url.as_deref() {
        let reason = if direct_error.is_empty() {
            tunnel_error.clone()
        } else {
            format!("{direct_error}；隧道：{tunnel_error}")
        };
        return Ok(ApiResponse::ok(status_body(
            &state,
            "proxy",
            base,
            Aggregate::failed(&reason),
            &tunnel,
        )));
    }
    Ok(ApiResponse::ok(not_configured_body(&tunnel, &tunnel_error)))
}

/// `GET /api/backpack/ai-window` —— 巡检到点自动打开的识别窗口
///
/// 排查「航点配了 AI 识别但没出告警」时先看这里：窗口有没有开、配到了哪个场景、
/// 阈值多少、窗口内上报了哪几类目标。逻辑见 [`crate::ai_patrol`]。
pub async fn ai_window(
    State(_state): State<AppState>,
    Extension(_claims): Extension<Claims>,
) -> Json<ApiResponse<Value>> {
    ApiResponse::ok(crate::ai_patrol::global().status().await)
}

/// `GET /api/backpack/events` —— Agent SSE 透传
///
/// 事件类型（`detect` / `telemetry` / `button` / `detect_state`…）由 Agent 定义，
/// 平台**不改写内容**只换传输层；页面解析逻辑与 Demo 保持一致，后续接告警中心时
/// 再在服务端侧顺便落库（那时才需要解析）。
///
/// 两条通道：服务端直连 Agent（原样透传字节流）；隧道模式由板端客户端订阅本机
/// Agent 后把事件转发上来（见 `agent_tunnel.rs`）。
pub async fn events(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
) -> Result<Response, AppError> {
    let mut direct_error = String::new();
    if let Some(base) = state.config.backpack_base_url.as_deref() {
        match connect_sse(base).await {
            Ok(response) => return Ok(passthrough_sse(response)),
            Err(error) => direct_error = error,
        }
    }

    let prefer = state.config.backpack_tunnel_device.as_deref();
    if state
        .agent_tunnel
        .available(prefer, &state.device_connections)
        .await
    {
        return Ok(tunnel_sse(&state));
    }

    Err(AppError::Upstream(if direct_error.is_empty() {
        agent_base(&state.config)
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| {
                "本部署既没配 ROBODOG_BACKPACK_URL，也没有板端设备客户端在线：背包接口不可用"
                    .to_string()
            })
    } else {
        format!("{direct_error}（也没有可用的设备客户端隧道）")
    }))
}

/// 连上 Agent 的 SSE（长连接：只限建连时间，不设总超时）
async fn connect_sse(base: &str) -> Result<reqwest::Response, String> {
    let client = stream_client().map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{base}/api/events"))
        .header(header::ACCEPT, "text/event-stream")
        .send()
        .await
        .map_err(|error| {
            format!("连不上背包 Agent（{base}）：{error}；确认板端 Agent 已启动、与本机同网段")
        })?;
    if !response.status().is_success() {
        return Err(format!("背包 Agent 拒绝事件流：HTTP {}", response.status()));
    }
    Ok(response)
}

/// 直连模式的 SSE：逐块转发 Agent 的原始字节
fn passthrough_sse(response: reqwest::Response) -> Response {
    // 用 mpsc + poll_fn 转发，避免为 SSE 引入额外依赖（reqwest 未开 `stream` feature）。
    // 通道小、事件也小（每条几十字节），不做批量合并。
    // 用 `chunk()` 逐块读而不是 `bytes_stream()`：后者要开 reqwest 的 `stream`
    // feature，本项目为离线构建保持了最小特性集（见 Cargo.toml）。
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(32);
    tokio::spawn(async move {
        let mut upstream = response;
        loop {
            match upstream.chunk().await {
                Ok(Some(bytes)) => {
                    if tx.send(Ok(bytes.to_vec())).await.is_err() {
                        break; // 浏览器断开
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(std::io::Error::other(error.to_string()))).await;
                    break;
                }
            }
        }
    });

    let body = Body::from_stream(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)));
    sse_response(body)
}

/// 隧道模式的 SSE：板端客户端转发上来的 `detect` 类事件 + 20s 心跳注释行
fn tunnel_sse(state: &AppState) -> Response {
    let mut events = state.agent_tunnel.subscribe();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(32);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 第一个 tick 立即就绪，先吃掉，免得一上来就发心跳
        tick.tick().await;
        loop {
            let chunk: Vec<u8> = tokio::select! {
                event = events.recv() => match event {
                    Ok(text) => format!("data: {text}\n\n").into_bytes(),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        // 页面卡顿 / 断网重连期间漏掉几帧框，没必要补发
                        debug!("背包事件流订阅落后，跳过 {skipped} 条");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                // 注释行只防代理把空闲连接掐掉，`EventSource` 不会派发给页面
                _ = tick.tick() => b": ping\n\n".to_vec(),
            };
            if tx.send(Ok(chunk)).await.is_err() {
                break; // 浏览器断开
            }
        }
    });

    let body = Body::from_stream(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)));
    sse_response(body)
}

fn sse_response(body: Body) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            // nginx 关闭缓冲，否则事件被攒成一坨才下发
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        body,
    )
        .into_response()
}

/// AI 启停 / 热更新的统一转发：`action` = `start` / `stop` / `config`
///
/// 始终返回 200 + Agent 的原始载荷（其中 `ok=false` 时带 `error` 文案）。
/// 原因是这些错误几乎都是**现场可自解释的业务提示**（「识别已在运行」「未找到场景」），
/// 作为 HTTP 4xx 会让前端全局拦截器统一弹 toast，丢失页面内的上下文位置。
pub async fn detect_action(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    action: &str,
    body: Value,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    // ① 服务端直连（与 status 同一优先级：能直连就别多绕两跳）
    let mut direct_error = String::new();
    if let Some(base) = state.config.backpack_base_url.as_deref() {
        match post_direct(base, action, &body).await {
            Ok(payload) => return Ok(ApiResponse::ok(payload)),
            Err(error) => direct_error = error,
        }
    }

    // ② 隧道：板端客户端代调
    let prefer = state.config.backpack_tunnel_device.as_deref();
    let tunneled = state
        .agent_tunnel
        .act(
            prefer,
            &state.device_connections,
            action,
            body,
            TUNNEL_ACTION_TIMEOUT,
        )
        .await;
    match tunneled {
        Ok(payload) => Ok(ApiResponse::ok(payload)),
        Err(tunnel_error) => {
            let message = if !direct_error.is_empty() {
                format!("{direct_error}；隧道：{tunnel_error}")
            } else if state.config.backpack_base_url.is_none() {
                // 完全没配 base_url：先说清该配哪个变量，再补隧道的失败原因
                let hint = agent_base(&state.config)
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_default();
                format!("{hint}；隧道：{tunnel_error}")
            } else {
                tunnel_error
            };
            Err(AppError::Upstream(message))
        }
    }
}

/// 直连 Agent 做一次 AI 动作；`Err` 只表示**传输失败**（Agent 的业务错误仍走 Ok 载荷）
async fn post_direct(base: &str, action: &str, body: &Value) -> Result<Value, String> {
    let client = action_client().map_err(|error| error.to_string())?;
    let response = client
        .post(format!("{base}/api/detect/{action}"))
        .json(body)
        .send()
        .await
        .map_err(|error| {
            format!("连不上背包 Agent（{base}）：{error}；确认板端 Agent 已启动、与本机同网段")
        })?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    Ok(serde_json::from_str(&text).unwrap_or_else(|_| {
        json!({
            "ok": status.is_success(),
            "error": text.chars().take(200).collect::<String>(),
        })
    }))
}

pub async fn detect_start(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    detect_action(State(state), Extension(claims), "start", body).await
}

pub async fn detect_stop(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    detect_action(State(state), Extension(claims), "stop", json!({})).await
}

pub async fn detect_config(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    detect_action(State(state), Extension(claims), "config", body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn agent_cameras() -> Value {
        json!({
            "cameras": {
                "cam0": { "title": "左前", "stream": "cam0" },
                "cam1": { "title": "右前", "stream": "cam1" },
                "cam2": { "title": "右后", "stream": "cam2" },
                "cam3": { "title": "左后", "stream": "cam3" },
                "dogcam": { "title": "机器狗前置", "stream": "dogcam" }
            },
            "infrared": { "stream": "dog-ipcam" },
            "webrtc": { "host": "192.168.234.200", "port": 8889 }
        })
    }

    #[test]
    fn six_sources_are_ordered_and_only_infrared_has_no_ai() {
        let relay = json!({ "enabled": true, "running": true });
        let list = sources_of(&agent_cameras(), Some(&relay));
        let keys: Vec<&str> = list.iter().filter_map(|s| s["key"].as_str()).collect();
        // 顺序固定：狗前置 → 四路相机 → 红外（前端按这个顺序渲染 chip）
        assert_eq!(keys, ["dogcam", "cam0", "cam1", "cam2", "cam3", "infrared"]);
        assert_eq!(list[0]["label"], "机器狗前置");
        assert_eq!(list[1]["label"], "左前");
        assert_eq!(list[5]["label"], "红外");
        // 红外用 mediamtx 的 path 名，不是 camN
        assert_eq!(list[5]["stream"], "dog-ipcam");
        assert!(list[0]["ai"].as_bool().unwrap());
        let ai: Vec<bool> = list.iter().map(|s| s["ai"].as_bool().unwrap()).collect();
        assert_eq!(ai, [true, true, true, true, true, false]);
        let available: Vec<bool> = list
            .iter()
            .map(|s| s["available"].as_bool().unwrap())
            .collect();
        assert_eq!(available, [true, true, true, true, true, true]);
    }

    #[test]
    fn dogcam_is_unavailable_when_relay_source_is_not_configured() {
        let relay = json!({ "enabled": false, "running": false });
        let list = sources_of(&agent_cameras(), Some(&relay));
        assert_eq!(list[0]["available"], json!(false));
        assert!(
            list[0]["hint"].as_str().unwrap().contains("未配置"),
            "置灰时必须写清恢复方式，现场才知道配哪个环境变量"
        );
        // 其余五路不受影响
        assert!(list[1..].iter().all(|s| s["available"].as_bool().unwrap()));
    }

    #[test]
    fn dogcam_hint_explains_configured_but_not_streaming() {
        let relay = json!({ "enabled": true, "running": false });
        let list = sources_of(&agent_cameras(), Some(&relay));
        // 「配过源但没推流」仍可点（看门狗会自动重连），只是给出原因
        assert_eq!(list[0]["available"], json!(true));
        assert!(list[0]["hint"].as_str().unwrap().contains("没有收到"));
    }
}
