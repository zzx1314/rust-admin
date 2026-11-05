use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::api::AppState;
use crate::business::dog::gateway::domain::{
    CommandResultPayload, FileEndPayload, FileStartPayload, HeartbeatPayload, MAX_UPLOAD_BYTES,
    MSG_FILE_END, MSG_FILE_START, MSG_HEARTBEAT, MSG_PING, MSG_REPORT, MSG_SERVER_COMMAND,
    MSG_TOROS_REPORT, file_ack, heartbeat_ack,
};

/// In-progress binary upload of a device stream.
struct UploadState {
    stream_id: String,
    file_name: Option<String>,
    media_type: Option<String>,
    buffer: Vec<u8>,
}

/// Drive one device WebSocket connection until it closes.
pub async fn run(state: AppState, device_id: String, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    state
        .dog_gateway
        .registry()
        .register(&device_id, tx, None)
        .await;
    tracing::info!(device_id = %device_id, "device websocket connected");

    let writer = tokio::spawn(async move {
        while let Some(text) = rx.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut upload: Option<UploadState> = None;

    while let Some(frame) = stream.next().await {
        match frame {
            Ok(Message::Text(text)) => {
                handle_text(&state, &device_id, text.as_str(), &mut upload).await;
            }
            Ok(Message::Binary(bytes)) => {
                let Some(current) = upload.as_mut() else {
                    tracing::warn!(
                        device_id = %device_id,
                        size = bytes.len(),
                        "binary frame without file_start, ignored"
                    );
                    continue;
                };
                if current.buffer.len() + bytes.len() > MAX_UPLOAD_BYTES {
                    tracing::warn!(
                        device_id = %device_id,
                        stream_id = %current.stream_id,
                        "upload stream exceeded the size limit, dropped"
                    );
                    upload = None;
                    continue;
                }
                current.buffer.extend_from_slice(&bytes);
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Err(error) => {
                tracing::debug!(device_id = %device_id, %error, "device websocket error");
                break;
            }
        }
    }

    state.dog_gateway.registry().unregister(&device_id).await;
    state.dog_gateway.mark_offline(&device_id).await;
    writer.abort();
    tracing::info!(device_id = %device_id, "device websocket closed");
}

async fn handle_text(
    state: &AppState,
    device_id: &str,
    text: &str,
    upload: &mut Option<UploadState>,
) {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(device_id = %device_id, %error, payload = text, "invalid json frame");
            return;
        }
    };

    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    match kind.as_str() {
        MSG_HEARTBEAT => {
            let heartbeat: HeartbeatPayload =
                serde_json::from_value(value.clone()).unwrap_or_default();
            if let Err(error) = state
                .dog_gateway
                .handle_heartbeat(device_id, &heartbeat)
                .await
            {
                tracing::warn!(device_id = %device_id, %error, "failed to store heartbeat");
            }
            reply(state, device_id, heartbeat_ack(device_id)).await;
        }
        MSG_SERVER_COMMAND | MSG_REPORT | MSG_TOROS_REPORT => {
            let result: CommandResultPayload =
                serde_json::from_value(value.clone()).unwrap_or_default();
            if let Err(error) = state
                .dog_gateway
                .handle_command_result(device_id, &result, &value)
                .await
            {
                tracing::warn!(device_id = %device_id, %error, "failed to store command result");
            }
        }
        MSG_FILE_START => {
            let start: FileStartPayload = serde_json::from_value(value.clone()).unwrap_or_default();
            if start.stream_id.trim().is_empty() {
                tracing::warn!(device_id = %device_id, "file_start without streamId");
                return;
            }
            *upload = Some(UploadState {
                stream_id: start.stream_id,
                file_name: start.file_name,
                media_type: start.media_type,
                buffer: Vec::new(),
            });
        }
        MSG_FILE_END => {
            let end: FileEndPayload = serde_json::from_value(value.clone()).unwrap_or_default();
            let Some(current) = upload.take() else {
                tracing::warn!(device_id = %device_id, stream_id = %end.stream_id, "file_end without file_start");
                reply(state, device_id, file_ack(&end.stream_id, false, None)).await;
                return;
            };
            if current.stream_id != end.stream_id {
                tracing::warn!(
                    device_id = %device_id,
                    expected = %current.stream_id,
                    received = %end.stream_id,
                    "file_end stream mismatch"
                );
                reply(state, device_id, file_ack(&end.stream_id, false, None)).await;
                return;
            }

            match state
                .dog_gateway
                .save_upload(
                    device_id,
                    &current.stream_id,
                    current.file_name.as_deref(),
                    current.media_type.as_deref(),
                    &current.buffer,
                )
                .await
            {
                Ok(media) => {
                    let path = media.as_ref().and_then(|media| media.media_path.clone());
                    reply(
                        state,
                        device_id,
                        file_ack(&current.stream_id, media.is_some(), path.as_deref()),
                    )
                    .await;
                }
                Err(error) => {
                    tracing::warn!(
                        device_id = %device_id,
                        stream_id = %current.stream_id,
                        %error,
                        "failed to store upload"
                    );
                    reply(state, device_id, file_ack(&current.stream_id, false, None)).await;
                }
            }
        }
        MSG_PING => {
            reply(state, device_id, json!({ "type": "pong" })).await;
        }
        other => {
            tracing::debug!(device_id = %device_id, kind = other, "unknown frame type");
        }
    }
}

async fn reply(state: &AppState, device_id: &str, payload: Value) {
    let text = payload.to_string();
    if !state.dog_gateway.registry().send(device_id, &text).await {
        tracing::debug!(device_id = %device_id, "no active connection to reply to");
    }
}
