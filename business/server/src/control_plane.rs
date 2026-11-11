//! 反向控制信道客户端：业务平台主动拨入中台。
//!
//! 内网部署的业务平台没有入站端口，中台无法 `POST` 过来；但业务平台能出站。
//! 因此由本进程主动连中台的 `/api/dogDevice/control-plane/ws`，在那条连接上
//! 接收「指令 / 任务 / 运行态查询」并回执：
//!
//! ```text
//!   本进程 ──dial(出站)──▶ 中台 /api/dogDevice/control-plane/ws
//!        ◀── request ─────┘
//!        ──── result ─────▶
//! ```
//!
//! 执行逻辑与 HTTP 链路共用 `handlers::service::{apply_command, apply_task,
//! runtime_snapshot}`，所以两条链路的白名单、幂等、事件留痕完全一致。
//! 设备 WebSocket 仍然只连本进程，中台不碰设备帧。

use crate::app::AppState;
use crate::db::now;
use crate::error::AppError;
use crate::handlers::service::{self, CommandRequest, ControlMeta, TaskDispatchRequest};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tracing::{info, warn};

/// 握手时的服务标识，方便中台侧日志辨认。
const SERVICE_NAME: &str = "robodog-ops-server";
const HEADER: &str = "x-service-token";
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Frame {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    ts: String,
    #[serde(default)]
    payload: Value,
}

impl Frame {
    fn new(kind: &str, payload: Value) -> Self {
        Self {
            kind: kind.to_string(),
            ts: now(),
            payload,
        }
    }
}

/// 启动反向控制信道。未配置 `ROBODOG_ZENTAI_WS_URL` 时静默跳过（同网部署
/// 继续用中台直连 HTTP 的方式）。
pub fn spawn(state: AppState) {
    let Some(base) = state.config.zentai_ws_url.clone() else {
        info!("未配置 ROBODOG_ZENTAI_WS_URL，跳过反向控制信道");
        return;
    };
    let Some(token) = state.config.service_token.clone() else {
        warn!("未配置 ROBODOG_SERVICE_TOKEN，反向控制信道不会建立");
        return;
    };

    tokio::spawn(async move {
        let url = format!(
            "{}/api/dogDevice/control-plane/ws",
            base.trim_end_matches('/')
        );
        let mut backoff = MIN_BACKOFF;
        loop {
            match connect_once(&state, &url, &token, &mut backoff).await {
                Ok(()) => {
                    info!("控制信道已断开，{}s 后重连", backoff.as_secs());
                    backoff = MIN_BACKOFF;
                }
                Err(error) => {
                    warn!(
                        "控制信道连接失败（{}），{}s 后重试: {error}",
                        url,
                        backoff.as_secs()
                    );
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    });
}

/// 建立一次连接并跑到断开为止；返回 `Ok(())` 表示连上过（用于重置退避）。
async fn connect_once(
    state: &AppState,
    url: &str,
    token: &str,
    backoff: &mut Duration,
) -> Result<(), String> {
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    request.headers_mut().insert(
        HEADER,
        HeaderValue::from_str(token).map_err(|error| format!("服务令牌非法: {error}"))?,
    );

    let (socket, _response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| e.to_string())?;
    let (mut sink, mut stream) = socket.split();

    let hello = Frame::new(
        "hello",
        json!({"service": SERVICE_NAME, "version": env!("CARGO_PKG_VERSION")}),
    );
    let text = serde_json::to_string(&hello).unwrap_or_default();
    if sink.send(Message::Text(text)).await.is_err() {
        return Ok(());
    }
    *backoff = MIN_BACKOFF;
    info!("控制信道已连上中台: {url}");

    while let Some(message) = stream.next().await {
        let text = match message {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(error) => {
                warn!("控制信道读取失败: {error}");
                break;
            }
        };

        let frame: Frame = match serde_json::from_str(&text) {
            Ok(frame) => frame,
            Err(error) => {
                warn!("控制信道收到无法解析的消息: {error}");
                continue;
            }
        };

        match frame.kind.as_str() {
            "request" => {
                let payload = frame.payload.clone();
                let request_id = payload["requestId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let reply = match handle_request(state, &payload).await {
                    Ok(data) => {
                        json!({"requestId": request_id, "ok": true, "data": data})
                    }
                    Err(error) => {
                        json!({"requestId": request_id, "ok": false, "error": error.to_string()})
                    }
                };
                let text = serde_json::to_string(&Frame::new("result", reply)).unwrap_or_default();
                if sink.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            "ping" => {
                let text =
                    serde_json::to_string(&Frame::new("pong", json!({}))).unwrap_or_default();
                if sink.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            other => warn!("控制信道收到未处理的消息类型: {other}"),
        }
    }

    Ok(())
}

/// 处理一条控制请求，返回给中台的回执数据。
async fn handle_request(state: &AppState, payload: &Value) -> Result<Value, AppError> {
    let kind = payload["kind"].as_str().unwrap_or("command");
    let device_code = payload["deviceCode"].as_str().unwrap_or_default();
    if device_code.trim().is_empty() {
        return Err(AppError::BadRequest("控制请求缺少 deviceCode".to_string()));
    }
    let meta = ControlMeta {
        request_id: payload["requestId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        operator: payload["operatorId"].as_i64(),
        operator_admin: payload["operatorAdmin"].as_bool().unwrap_or(false),
    };

    match kind {
        "runtime" => service::runtime_snapshot(state, device_code).await,
        "sync-maps" => {
            let mode = payload["body"]["mode"].as_str().unwrap_or("manual");
            service::apply_sync_maps(state, device_code, mode).await
        }
        "task" => {
            let body: TaskDispatchRequest = serde_json::from_value(payload["body"].clone())
                .map_err(|error| AppError::BadRequest(format!("任务载荷无效: {error}")))?;
            service::apply_task(state, &meta, device_code, &body).await
        }
        "command" => {
            let body: CommandRequest = serde_json::from_value(payload["body"].clone())
                .map_err(|error| AppError::BadRequest(format!("指令载荷无效: {error}")))?;
            service::apply_command(state, &meta, device_code, &body).await
        }
        other => Err(AppError::BadRequest(format!("不支持的控制类型: {other}"))),
    }
}
