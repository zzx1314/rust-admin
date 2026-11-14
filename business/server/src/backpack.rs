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
//! 这里**只读不改**：不做任何业务落库，AI 事件写库留给告警中心那一期。

use crate::app::AppState;
use crate::auth::Claims;
use crate::config::Config;
use crate::error::{ApiResponse, AppError};
use axum::Json;
use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::time::Duration;

/// 聚合查询的单次上游超时。Agent 的 `/api/detect/tasks` 会扫目录，3s 足够；
/// 超过就按「背包不可达」上报，页面立刻给结论，不转圈。
const API_TIMEOUT: Duration = Duration::from_secs(4);

/// 会话地址前缀：Agent 的 `/api/whep/*` 会把 mediamtx 的会话地址改写成
/// 自己的同源路径，平台不再包一层，所以这里只需要知道 path 形态即可。
const AI_KEYS: [&str; 5] = ["dogcam", "cam0", "cam1", "cam2", "cam3"];

fn api_client() -> Result<reqwest::Client, AppError> {
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

fn stream_client() -> Result<reqwest::Client, AppError> {
    // SSE 是长连接：**不能设总超时**，只限制建连时间，否则 4s 后事件流被切断，
    // 页面看起来像「识别框不刷新」。
    reqwest::Client::builder()
        .connect_timeout(API_TIMEOUT)
        .build()
        .map_err(|error| AppError::Internal(format!("构建 SSE 客户端失败: {error}")))
}

/// 未配置时的统一提示：说清配哪里，而不是抛 500。
fn agent_base(config: &Config) -> Result<&str, AppError> {
    config.backpack_base_url.as_deref().ok_or_else(|| {
        AppError::Upstream(
            "本部署未接入数据采集背包：请设置 ROBODOG_BACKPACK_URL（板端 Agent 地址，如 \
             http://192.168.234.200:8000）后重启业务服务"
                .to_string(),
        )
    })
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
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

/// `GET /api/backpack/status` —— 页面首屏只发这一个请求
///
/// 聚合 Agent 的 `cameras` / `relay` / `detect/tasks` / `detect/status`：
/// 4 个请求并发，任何一个失败都**不影响其余**（各自的 `*_error` 字段说明原因）。
pub async fn status(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    // 直连模式：正式环境里平台在公网、背包在现场内网，业务服务连不上 Agent，
    // 但操作员浏览器与背包同网段。此时后端**不下发聚合结果**（它拿不到），
    // 只告诉页面「往哪个地址去调」，REST/SSE 由浏览器直连 Agent（已开 CORS）。
    if let Some(browser) = state.config.backpack_browser_url.as_deref() {
        return Ok(ApiResponse::ok(json!({
            "configured": true,
            "mode": "direct",
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

    let Some(base) = state.config.backpack_base_url.as_deref() else {
        // 未接入不是错误：页面要展示「怎么配」的指引卡片，而不是一个红色报错。
        // 其它接口（SSE / AI 启停）仍然返回 503 —— 那些操作确实做不了。
        return Ok(ApiResponse::ok(json!({
            "configured": false,
            "mode": "none",
            "agent": { "base": "", "reachable": false, "error": "未配置 ROBODOG_BACKPACK_URL" },
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
    };
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

    let reachable = cameras.is_ok();
    let relay_value = relay.as_ref().ok().cloned();
    let sources = cameras
        .as_ref()
        .map(|cameras| sources_of(cameras, relay_value.as_ref()))
        .unwrap_or_default();

    let detect_status = detect.as_ref().ok();
    let osd = detect_status
        .and_then(|s| s.get("osd"))
        .cloned()
        .unwrap_or(Value::Null);

    Ok(ApiResponse::ok(json!({
        "configured": true,
        "mode": "proxy",
        "agent": {
            "base": base,
            "reachable": reachable,
            "error": cameras.as_ref().err().cloned().unwrap_or_default(),
        },
        // 浏览器直连的 WHEP 基址（页面用它拼 `<base>/<stream>/whep`）
        "webrtc": { "base": state.config.backpack_webrtc_base() },
        "sources": sources,
        "relay": relay_value.unwrap_or(Value::Null),
        "relay_error": relay.as_ref().err().cloned().unwrap_or_default(),
        "detect": {
            "tasks": tasks.as_ref().ok().cloned().unwrap_or(Value::Null),
            "tasks_error": tasks.as_ref().err().cloned().unwrap_or_default(),
            "status": detect_status.cloned().unwrap_or(Value::Null),
            "status_error": detect.as_ref().err().cloned().unwrap_or_default(),
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
    })))
}

/// `GET /api/backpack/events` —— Agent SSE 原样透传
///
/// 事件类型（`detect` / `telemetry` / `button` / `detect_state`…）由 Agent 定义，
/// 平台**不改写内容**只换传输层；页面解析逻辑与 Demo 保持一致，后续接告警中心时
/// 再在服务端侧顺便落库（那时才需要解析）。
pub async fn events(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
) -> Result<Response, AppError> {
    let base = agent_base(&state.config)?.to_string();
    let client = stream_client()?;
    let response = client
        .get(format!("{base}/api/events"))
        .header(header::ACCEPT, "text/event-stream")
        .send()
        .await
        .map_err(|error| {
            AppError::Upstream(format!(
                "连不上背包 Agent（{base}）：{error}；确认板端 Agent 已启动、与本机同网段"
            ))
        })?;

    if !response.status().is_success() {
        return Err(AppError::Upstream(format!(
            "背包 Agent 拒绝事件流：HTTP {}",
            response.status()
        )));
    }

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
                    let _ = tx
                        .send(Err(std::io::Error::other(error.to_string())))
                        .await;
                    break;
                }
            }
        }
    });

    let body = Body::from_stream(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)));
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            // nginx 关闭缓冲，否则事件被攒成一坨才下发
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        body,
    )
        .into_response())
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
    let base = agent_base(&state.config)?;
    let client = action_client()?;
    let url = format!("{base}/api/detect/{action}");
    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            AppError::Upstream(format!(
                "连不上背包 Agent（{base}）：{error}；确认板端 Agent 已启动、与本机同网段"
            ))
        })?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let payload: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
        json!({
            "ok": status.is_success(),
            "error": text.chars().take(200).collect::<String>(),
        })
    });
    Ok(ApiResponse::ok(payload))
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
