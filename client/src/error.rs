use thiserror::Error;

/// Errors returned by the device client.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("websocket error: {0}")]
    WebSocket(Box<tokio_tungstenite::tungstenite::Error>),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid url: {0}")]
    Url(String),

    #[error("server rejected the request: code={code} msg={msg}")]
    Server { code: u16, msg: String },

    #[error("device token missing, call authenticate() first")]
    MissingToken,

    #[error("unexpected server response: {0}")]
    Protocol(String),

    #[error("tls configuration error: {0}")]
    Tls(String),

    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
}

impl From<tokio_tungstenite::tungstenite::Error> for ClientError {
    fn from(error: tokio_tungstenite::tungstenite::Error) -> Self {
        // Boxed because `tungstenite::Error` is large (clippy::result_large_err).
        Self::WebSocket(Box::new(error))
    }
}
