//! 控制信道 WebSocket 端点：`GET /api/dogDevice/control-plane/ws`。
//!
//! 由业务平台主动拨入（内网出站），用同行共享服务令牌鉴权。中台在这条连接上
//! 推送控制请求并等待回执，不转发任何设备帧。

use crate::api::AppState;
use crate::business::dog::device::control_channel::{self, Frame};
use crate::common::error::AppError;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::time::Duration;
use tracing::{info, warn};

/// 心跳间隔：长时间只有下行请求的连接也要能发现半开状态。
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// `GET /api/dogDevice/control-plane/ws` — 业务平台拨入的反向控制信道。
pub async fn control_plane_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let expected = state
        .business_api_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(
                "未配置业务平台服务令牌（APP_DOG__BUSINESS_API_TOKEN），无法建立控制信道"
                    .to_string(),
            )
        })?;
    let provided = headers
        .get(control_channel::SERVICE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if provided.trim() != expected {
        return Err(AppError::Unauthorized("服务令牌无效".to_string()));
    }

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state)))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let channel = state.control_channel.clone();
    let (id, mut outgoing) = channel.register().await;
    let mut ticker = tokio::time::interval(PING_INTERVAL);
    ticker.tick().await;

    info!("业务平台控制信道已建立（connection {id}）");

    loop {
        tokio::select! {
            frame = outgoing.recv() => {
                let Some(frame) = frame else { break };
                let text = serde_json::to_string(&frame).unwrap_or_default();
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => handle_text(&channel, &text).await,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        warn!("控制信道读取失败（connection {id}）: {error}");
                        break;
                    }
                }
            }
            _ = ticker.tick() => {
                if !channel.send(id, Frame::new("ping", json!({}))).await {
                    break;
                }
            }
        }
    }

    channel.unregister(id).await;
    info!("业务平台控制信道已断开（connection {id}）");
}

async fn handle_text(channel: &control_channel::ControlChannel, text: &str) {
    let frame: Frame = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(error) => {
            warn!("控制信道收到无法解析的消息: {error}");
            return;
        }
    };

    match frame.kind.as_str() {
        "hello" => {
            info!(
                service = frame.payload["service"].as_str().unwrap_or("unknown"),
                version = frame.payload["version"].as_str().unwrap_or("unknown"),
                "业务平台控制信道握手完成"
            );
        }
        "result" => {
            let request_id = frame.payload["requestId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if request_id.is_empty() {
                warn!("控制信道收到缺少 requestId 的回执");
                return;
            }
            let ok = frame.payload["ok"].as_bool().unwrap_or(false);
            let result = if ok {
                Ok(frame.payload["data"].clone())
            } else {
                Err(frame.payload["error"]
                    .as_str()
                    .unwrap_or("业务平台执行失败")
                    .to_string())
            };
            channel.complete(&request_id, result).await;
        }
        "pong" | "ping" => {}
        other => {
            // 只记录，不打断连接：未来协议加消息时旧版本中台仍然可用
            warn!(kind = other, "控制信道收到未处理的消息类型");
        }
    }
}
