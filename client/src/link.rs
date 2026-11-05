use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::error::ClientError;
use crate::protocol::{
    CommandResult, FileAck, FileEndMessage, FileStartMessage, Heartbeat, MSG_FILE_END,
    MSG_FILE_START, MSG_SERVER_COMMAND, ServerCommand,
};

/// Binary frames are chunked to keep each WebSocket message small
/// (the original client used a 10 KiB payload buffer).
const UPLOAD_CHUNK_SIZE: usize = 8 * 1024;

/// An established device WebSocket link.
pub struct DeviceLink {
    stream: WebSocketStream<MaybeTlsStream<TcpStream>>,
}

impl DeviceLink {
    pub(crate) fn new(stream: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        Self { stream }
    }

    pub async fn send_text(&mut self, text: &str) -> Result<(), ClientError> {
        self.stream.send(Message::Text(text.into())).await?;
        Ok(())
    }

    pub async fn send_json<T: Serialize>(&mut self, value: &T) -> Result<(), ClientError> {
        let text = serde_json::to_string(value)?;
        self.send_text(&text).await
    }

    /// Receive the next JSON frame, or `None` when `timeout` elapsed.
    pub async fn recv_json(&mut self, timeout: Duration) -> Result<Option<Value>, ClientError> {
        let start = Instant::now();
        loop {
            let remaining = match timeout.checked_sub(start.elapsed()) {
                Some(remaining) if !remaining.is_zero() => remaining,
                _ => return Ok(None),
            };

            match tokio::time::timeout(remaining, self.stream.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => {
                    return Err(ClientError::Protocol(
                        "server closed the connection".to_string(),
                    ));
                }
                Ok(Some(Err(error))) => return Err(error.into()),
                Ok(Some(Ok(Message::Text(text)))) => {
                    return Ok(Some(serde_json::from_str(text.as_str())?));
                }
                Ok(Some(Ok(Message::Binary(bytes)))) => {
                    return Ok(Some(serde_json::from_slice(&bytes)?));
                }
                Ok(Some(Ok(Message::Close(frame)))) => {
                    return Err(ClientError::Protocol(format!(
                        "server closed the connection: {frame:?}"
                    )));
                }
                Ok(Some(Ok(_))) => continue,
            }
        }
    }

    /// Send a heartbeat without waiting for the acknowledgement.
    pub async fn heartbeat(&mut self, heartbeat: &Heartbeat) -> Result<(), ClientError> {
        self.send_json(heartbeat).await
    }

    /// Send a heartbeat and wait for the server acknowledgement, which proves
    /// the link is bidirectional and the device is persisted as online.
    pub async fn heartbeat_with_ack(
        &mut self,
        heartbeat: &Heartbeat,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        self.heartbeat(heartbeat).await?;
        self.wait_for("heartbeat acknowledgement", timeout, |value| {
            value.get("type").and_then(Value::as_str) == Some("report")
        })
        .await
    }

    /// Wait for the next command (task) pushed by the server.
    pub async fn next_command(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<ServerCommand>, ClientError> {
        let start = Instant::now();
        loop {
            let remaining = match timeout.checked_sub(start.elapsed()) {
                Some(remaining) if !remaining.is_zero() => remaining,
                _ => return Ok(None),
            };

            let Some(value) = self.recv_json(remaining).await? else {
                return Ok(None);
            };

            if value.get("type").and_then(Value::as_str) == Some(MSG_SERVER_COMMAND) {
                return Ok(Some(serde_json::from_value(value)?));
            }
        }
    }

    /// Report a command result back to the server.
    pub async fn report(&mut self, result: &CommandResult) -> Result<(), ClientError> {
        self.send_json(result).await
    }

    /// Upload a file: `file_start` -> binary chunks -> `file_end`, then wait for
    /// the server acknowledgement.
    pub async fn upload_file(
        &mut self,
        path: impl AsRef<Path>,
        media_type: Option<&str>,
        target_ids: Option<&str>,
        timeout: Duration,
    ) -> Result<FileAck, ClientError> {
        let path = path.as_ref();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("upload.bin")
            .to_string();
        let data = tokio::fs::read(path).await?;
        let stream_id = format!("stream-{}", uuid::Uuid::new_v4().simple());

        self.send_json(&FileStartMessage {
            kind: MSG_FILE_START,
            stream_id: stream_id.clone(),
            file_name,
            media_type: media_type.map(str::to_string),
        })
        .await?;

        for chunk in data.chunks(UPLOAD_CHUNK_SIZE) {
            self.stream
                .send(Message::Binary(chunk.to_vec().into()))
                .await?;
        }

        self.send_json(&FileEndMessage {
            kind: MSG_FILE_END,
            stream_id: stream_id.clone(),
            target_ids: target_ids.map(str::to_string),
        })
        .await?;

        let ack = self
            .wait_for("file acknowledgement", timeout, |value| {
                value.get("type").and_then(Value::as_str) == Some("file_ack")
                    && value.get("streamId").and_then(Value::as_str) == Some(stream_id.as_str())
            })
            .await?;

        Ok(serde_json::from_value(ack)?)
    }

    /// Close the link with a normal (1000) close frame.
    pub async fn close(mut self) -> Result<(), ClientError> {
        self.stream.close(None).await?;
        Ok(())
    }

    async fn wait_for<F>(
        &mut self,
        what: &'static str,
        timeout: Duration,
        mut accept: F,
    ) -> Result<Value, ClientError>
    where
        F: FnMut(&Value) -> bool,
    {
        let start = Instant::now();
        loop {
            let remaining = match timeout.checked_sub(start.elapsed()) {
                Some(remaining) if !remaining.is_zero() => remaining,
                _ => return Err(ClientError::Timeout(what)),
            };

            let Some(value) = self.recv_json(remaining).await? else {
                return Err(ClientError::Timeout(what));
            };

            if accept(&value) {
                return Ok(value);
            }
        }
    }
}
