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
//! **多设备**：所有接口都接受可选的 `?device=<设备编号>`（POST 动作同理）。
//! 编号与板端 relay 的 `RELAY_DEVICE_ID`、设备客户端的 `ROBODOG_DEVICE_ID` 一致；
//! 空 / 不传 = 默认设备（`[backpack]` 单机字段，行为与以前完全一致）。
//! 一台设备一个 [`Target`]（Agent 地址 / 隧道目标 / WHEP 基址），**不存在
//! 「编号没配就回退到默认设备」这种做法** —— 那样会把 AI 起在另一台狗上。
//!
//! 这里**只读不改**：不做任何业务落库，AI 事件写库留给告警中心那一期。

use crate::app::AppState;
use crate::auth::Claims;
use crate::config::Config;
use crate::error::{ApiResponse, AppError};
use crate::video;
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

/// `GET /api/backpack/status|events` 的查询参数
#[derive(Debug, Default, Deserialize)]
pub struct StatusQuery {
    /// `tunnel` = 强制走设备客户端隧道。
    ///
    /// 直连模式下浏览器自己探测 Agent，只有它能判断「我能不能连上板子」；
    /// 探不通时页面带这个参数重试，后端改走隧道返回同一份聚合结果。
    transport: Option<String>,
    /// 设备编号（多设备部署）：空 / 不传 = 默认设备（`[backpack]` 的单机字段）。
    ///
    /// 决定 Agent 地址、隧道目标与视频房间名（`bp:<device>:<stream>`），
    /// 字符集见 [`crate::video::normalize_device`]。
    device: Option<String>,
}

/// 只要设备编号的查询参数（POST 动作用，它们不关心 `transport`）
#[derive(Debug, Default, Deserialize)]
pub struct DeviceQuery {
    device: Option<String>,
}

/// 一次请求要看的背包：把「设备编号 → 各项地址」的解析收在一处
///
/// `id` 为空 = 默认设备（`[backpack]` 的单机字段、旧房间名 `bp:<stream>`）。
/// 编号给了但清单里没有时**不回退到默认设备**：视频仍走平台中继（板端自己推上来），
/// 但 Agent / AI 一律不可用 —— 宁可报「没配」，也不能把 AI 起在另一台狗上。
struct Target {
    id: String,
    label: Option<String>,
    agent_base: Option<String>,
    browser_base: Option<String>,
    webrtc_base: Option<String>,
    tunnel_device: Option<String>,
}

impl Target {
    fn resolve(config: &Config, device: Option<&str>) -> Result<Self, AppError> {
        let id = match device.map(str::trim).filter(|value| !value.is_empty()) {
            None => None,
            Some(raw) => Some(video::normalize_device(raw).ok_or_else(|| {
                AppError::BadRequest(format!("设备编号不合法：{raw}（只允许字母数字与 . _ -）"))
            })?),
        };
        let named = id.is_some();
        let entry = config.backpack_device(id.as_deref());
        let entry = entry.unwrap_or_default();
        // 隧道目标：显式配置 > 设备自己的编号（客户端就用它登记）>
        // 默认设备才轮到 `backpack_tunnel_device`。
        let tunnel_device = match id.as_deref() {
            // 清单里配了就用配的；没配清单也照样用设备自己的编号 ——
            // 设备客户端本来就是按这个编号登记 `agent_hello` 的。
            Some(id) => entry.tunnel_target().or_else(|| Some(id.to_string())),
            None => config.backpack_tunnel_device.clone(),
        };
        Ok(Self {
            id: id.unwrap_or_default(),
            label: entry.label.clone(),
            agent_base: entry.base_url.clone(),
            browser_base: entry.browser_url.clone(),
            // 「现场直连」的板端 mediamtx 地址：默认设备用部署级固定值（升级前的行为），
            // 命名设备优先由它自己的 Agent 地址推导（同主机 :8889），推不出来再退到
            // 部署级固定值。平台中继的 WHEP 是固定同源路径，不依赖这个值。
            webrtc_base: if named {
                entry
                    .webrtc_base()
                    .or_else(|| config.backpack_webrtc_base())
            } else {
                config.backpack_webrtc_base()
            },
            tunnel_device,
        })
    }

    /// 有没有 Agent 可调（直连或代理）
    fn agent_configured(&self) -> bool {
        self.agent_base.is_some() || self.browser_base.is_some()
    }
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
///
/// `prefer` 是**这台设备**的隧道目标（见 [`Target::tunnel_device`]）：
/// 多背包下不能再用全局的 `backpack_tunnel_device`，否则问的是另一台狗。
async fn aggregate_tunnel(state: &AppState, prefer: Option<&str>) -> Aggregate {
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
    target: &Target,
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
        // 设备编号 / 名称：页面据此确认「自己看到的是选中的那台」（空 = 默认设备）
        "device": target.id,
        "deviceLabel": target.label,
        "mode": mode,
        // 隧道是否在位：直连模式下页面据此决定「探不通时要不要回退到平台」
        "tunnel": tunnel,
        "agent": {
            "base": base,
            "reachable": reachable,
            "error": aggregate.cameras.as_ref().err().cloned().unwrap_or_default(),
        },
        // 浏览器直连的 WHEP 基址（页面用它拼 `<base>/<stream>/whep`）
        "webrtc": { "base": target.webrtc_base },
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
///
/// 带设备编号时不算「未接入」：板端 relay 自己把视频推到平台，
/// 没有 Agent 地址只意味着 AI 用不了（`configured: true` + `agent.reachable: false`）。
fn not_configured_body(target: &Target, tunnel: &Value, tunnel_error: &str) -> Value {
    let named = !target.id.is_empty();
    let error = if named {
        format!(
            "设备 {} 没有配 Agent 地址：在业务服务 [[backpack.devices]]（或 ROBODOG_BACKPACK_DEVICES）\
             里补 base_url / browser_url；视频仍可在「平台中继」下观看：{tunnel_error}",
            target.id
        )
    } else {
        format!(
            "未配置 ROBODOG_BACKPACK_URL（或让板端客户端用 `[ai] enabled` 代调）：{tunnel_error}"
        )
    };
    json!({
        "configured": named,
        "device": target.id,
        "deviceLabel": target.label,
        "mode": "none",
        "tunnel": tunnel,
        // 隧道在位只是「客户端登记过」，没有 Agent 地址仍然做不了事：两条提示都给
        "agent": {
            "base": "",
            "reachable": false,
            "error": error,
        },
        "webrtc": { "base": target.webrtc_base },
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

/// `GET /api/backpack/status[?device=<id>]` —— 页面首屏只发这一个请求
///
/// 聚合 Agent 的 `cameras` / `relay` / `detect/tasks` / `detect/status`：
/// 4 个请求并发，任何一个失败都**不影响其余**（各自的 `*_error` 字段说明原因）。
///
/// 通道按「浏览器直连 → 服务端直连 → 设备客户端隧道」依次退让，
/// `?transport=tunnel` 可跳过前两条（页面在直连模式探测失败时用它）。
/// `?device=` 切换设备：地址、隧道目标、WHEP 基址全部按这台设备解析。
pub async fn status(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let force_tunnel = query.transport.as_deref().map(str::trim) == Some("tunnel");
    let target = Target::resolve(&state.config, query.device.as_deref())?;
    let prefer = target.tunnel_device.as_deref();
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
    if !force_tunnel && let Some(browser) = target.browser_base.as_deref() {
        return Ok(ApiResponse::ok(json!({
            "configured": true,
            "device": target.id,
            "deviceLabel": target.label,
            "mode": "direct",
            // 直连探测失败时页面据此回退到 `?transport=tunnel`
            "tunnel": tunnel,
            "direct": {
                "apiBase": browser,
                "webrtcBase": target.webrtc_base,
            },
            // 可达性由浏览器自己探测，这里不臆断
            "agent": { "base": browser, "reachable": true, "error": "" },
            "webrtc": { "base": target.webrtc_base },
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
    if !force_tunnel && let Some(base) = target.agent_base.as_deref() {
        let aggregate = aggregate_direct(base).await?;
        if aggregate.cameras.is_ok() {
            return Ok(ApiResponse::ok(status_body(
                &target, "proxy", base, aggregate, &tunnel,
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
        let aggregate = aggregate_tunnel(&state, prefer).await;
        if aggregate.cameras.is_ok() {
            let base = state.agent_tunnel.base_of(device).await.unwrap_or_default();
            return Ok(ApiResponse::ok(status_body(
                &target, "tunnel", &base, aggregate, &tunnel,
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
    if let Some(base) = target.agent_base.as_deref() {
        let reason = if direct_error.is_empty() {
            tunnel_error.clone()
        } else {
            format!("{direct_error}；隧道：{tunnel_error}")
        };
        return Ok(ApiResponse::ok(status_body(
            &target,
            "proxy",
            base,
            Aggregate::failed(&reason),
            &tunnel,
        )));
    }
    Ok(ApiResponse::ok(not_configured_body(
        &target,
        &tunnel,
        &tunnel_error,
    )))
}

/// `GET /api/backpack/devices` —— 视频页的「设备编号」下拉
///
/// 两只来源合并：
/// 1. 配置里的多设备清单（**离线也列出来**，现场才知道有这台）；
/// 2. 平台视频房间里实际连上的设备（板端 relay 注册了 `bp:<device>:<stream>` 就会出现，
///    哪怕没配清单 —— 这样「先上万台狗、后补配置」也能先看到画面）。
///
/// `online` = 至少一路推流端在线；`streams` 只列在线的通道。
/// `id` 为空的条目是默认设备（旧单机配置，房间名 `bp:<stream>`）。
pub async fn devices(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
) -> Json<ApiResponse<Value>> {
    let channels = state.video_hub.channels().await;
    // 配置顺序优先（现场看到的顺序与填的一致），其次才是发现出来的
    let mut order: Vec<String> = Vec::new();
    for device in &state.config.backpack_devices {
        order.push(device.id.clone());
    }
    if state.config.backpack_mode().is_some() || channels.iter().any(|item| item.device.is_empty())
    {
        order.insert(0, String::new());
    }
    for item in &channels {
        if !order.iter().any(|id| id == &item.device) {
            order.push(item.device.clone());
        }
    }

    let list: Vec<Value> = order
        .iter()
        .map(|id| {
            let entry = state.config.backpack_device(Some(id.as_str()));
            let streams: Vec<&str> = channels
                .iter()
                .filter(|item| &item.device == id && item.publisher)
                .map(|item| item.stream.as_str())
                .collect();
            // 通道名去重（不同设备不可能重，但同一台可能重复注册）
            let mut streams = streams;
            streams.dedup();
            let transport = channels
                .iter()
                .find(|item| &item.device == id && item.transport.is_some())
                .and_then(|item| item.transport.clone());
            let label = entry
                .as_ref()
                .and_then(|device| device.label.clone())
                .unwrap_or_else(|| {
                    if id.is_empty() {
                        "默认设备".to_string()
                    } else {
                        id.clone()
                    }
                });
            // 清单里配了 Agent 地址（没配就只能看视频，AI 不可用）；
            // 默认设备的「配置」就是单机字段有没有配
            let configured = if id.is_empty() {
                state.config.backpack_mode().is_some()
            } else {
                entry.is_some()
            };
            json!({
                "id": id,
                "label": label,
                // 板端 relay 在线（至少一路在推）
                "online": !streams.is_empty(),
                "configured": configured,
                "streams": streams,
                // 这台板子上报的媒体通路：`rtsp` = 正常；null/`ws` = relay 没升级
                "transport": transport,
            })
        })
        .collect();

    ApiResponse::ok(json!({ "devices": list }))
}

/// `GET /api/backpack/ai-window` —— 巡检到点自动打开的识别窗口
///
/// 排查「航点配了 AI 识别但没出告警」时先看这里：窗口有没有开、配到了哪个场景、
/// 阈值多少、窗口内上报了哪几类目标。逻辑见 [`crate::ai_patrol`]。
pub async fn ai_window(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
) -> Json<ApiResponse<Value>> {
    ApiResponse::ok(state.ai_patrol.status().await)
}

/// `GET /api/backpack/events[?device=<id>]` —— Agent SSE 透传
///
/// 事件类型（`detect` / `telemetry` / `button` / `detect_state`…）由 Agent 定义，
/// 平台**不改写内容**只换传输层；页面解析逻辑与 Demo 保持一致，后续接告警中心时
/// 再在服务端侧顺便落库（那时才需要解析）。
///
/// 两条通道：服务端直连 Agent（原样透传字节流）；隧道模式由板端客户端订阅本机
/// Agent 后把事件转发上来（见 `agent_tunnel.rs`）。
///
/// **多设备**：按 `?device=` 选 Agent；隧道事件还要再按来源设备过滤一遍，
/// 否则另一台狗跑巡检时的 `detect` 会把框画到这台画面上。
pub async fn events(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    Query(query): Query<StatusQuery>,
) -> Result<Response, AppError> {
    let target = Target::resolve(&state.config, query.device.as_deref())?;
    let mut direct_error = String::new();
    if let Some(base) = target.agent_base.as_deref() {
        match connect_sse(base).await {
            Ok(response) => return Ok(passthrough_sse(response)),
            Err(error) => direct_error = error,
        }
    }

    let tunnel_device = state
        .agent_tunnel
        .device(target.tunnel_device.as_deref(), &state.device_connections)
        .await;
    if tunnel_device.is_some() {
        // 默认设备没指定隧道目标时（单机部署）不过滤，保持原有行为
        return Ok(tunnel_sse(&state, tunnel_device));
    }

    Err(AppError::Upstream(if direct_error.is_empty() {
        if target.id.is_empty() {
            agent_base(&state.config)
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| {
                    "本部署既没配 ROBODOG_BACKPACK_URL，也没有板端设备客户端在线：背包接口不可用"
                        .to_string()
                })
        } else {
            format!(
                "设备 {} 没有配 Agent 地址（[[backpack.devices]] / ROBODOG_BACKPACK_DEVICES），\
                 也没有可用的设备客户端隧道：背包接口不可用",
                target.id
            )
        }
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
///
/// `filter` 是多背包时的来源设备过滤条件（`None` = 不过滤，单机部署的老行为）。
fn tunnel_sse(state: &AppState, filter: Option<String>) -> Response {
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
                    Ok(event) => {
                        if let Some(filter) = filter.as_deref()
                            && event.device.as_deref() != Some(filter)
                        {
                            continue;
                        }
                        format!("data: {}\n\n", event.text).into_bytes()
                    }
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
///
/// `device` 决定动作发给哪台 Agent / 哪条隧道：多设备下把 AI 起在另一台狗上
/// 是最难发现的事故，所以这里**没有**「没配就回退到默认设备」的分支。
pub async fn detect_action(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    action: &str,
    body: Value,
    device: Option<&str>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let target = Target::resolve(&state.config, device)?;
    // ① 服务端直连（与 status 同一优先级：能直连就别多绕两跳）
    let mut direct_error = String::new();
    if let Some(base) = target.agent_base.as_deref() {
        match post_direct(base, action, &body).await {
            Ok(payload) => return Ok(ApiResponse::ok(payload)),
            Err(error) => direct_error = error,
        }
    }

    // ② 隧道：板端客户端代调（目标是**这台设备**，不是全局配置的那台）
    let tunneled = state
        .agent_tunnel
        .act(
            target.tunnel_device.as_deref(),
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
            } else if !target.agent_configured() {
                // 完全没配 Agent 地址：先说清该配哪里，再补隧道的失败原因
                let hint = if target.id.is_empty() {
                    agent_base(&state.config)
                        .err()
                        .map(|error| error.to_string())
                        .unwrap_or_default()
                } else {
                    format!(
                        "设备 {} 没有配 Agent 地址（[[backpack.devices]] / ROBODOG_BACKPACK_DEVICES）",
                        target.id
                    )
                };
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
    Query(query): Query<DeviceQuery>,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    detect_action(
        State(state),
        Extension(claims),
        "start",
        body,
        query.device.as_deref(),
    )
    .await
}

pub async fn detect_stop(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    detect_action(
        State(state),
        Extension(claims),
        "stop",
        json!({}),
        query.device.as_deref(),
    )
    .await
}

pub async fn detect_config(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<DeviceQuery>,
    Json(body): Json<Value>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    detect_action(
        State(state),
        Extension(claims),
        "config",
        body,
        query.device.as_deref(),
    )
    .await
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
