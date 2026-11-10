use url::Url;

use crate::device_auth::DeviceSignature;
use crate::error::ClientError;
use crate::link::DeviceLink;
use crate::protocol::{ApiEnvelope, DeviceRegistration, DeviceSummary, DeviceTokenData};
use crate::tls::TlsOptions;

/// HTTP + WebSocket client for one device.
///
/// Flow (same as the original C client):
/// 1. `authenticate()` — `GET /api/dogGateway/auth/{deviceId}` returns a token;
/// 2. `register()` — `POST /api/dogGateway/register` with the token;
/// 3. `connect()` — `GET /api/notice/{deviceId}/{token}` upgraded to WebSocket.
///
/// Step 1 requires a shared secret configured with
/// [`DeviceClient::with_auth_secret`]; the client signs `deviceId`, a Unix
/// timestamp and a fresh nonce (see [`crate::device_auth`]).
pub struct DeviceClient {
    http: reqwest::Client,
    base_url: String,
    device_id: String,
    token: Option<String>,
    auth_secret: Option<String>,
}

impl DeviceClient {
    /// `base_url` is the server root, for example `http://127.0.0.1:3000`.
    pub fn new(base_url: impl Into<String>, device_id: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            device_id: device_id.into(),
            token: None,
            auth_secret: None,
        }
    }

    /// Set the shared secret used to sign [`DeviceClient::authenticate`].
    ///
    /// The server rejects authentication without a valid signature, so this
    /// must match `APP_DEVICE_AUTH_SECRET` on the backend.
    pub fn with_auth_secret(mut self, secret: impl Into<String>) -> Self {
        self.auth_secret = Some(secret.into());
        self
    }

    /// Use a custom HTTP client (for example one built by [`TlsOptions::http_client`]).
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Use a custom HTTP client built from TLS options in one step.
    pub fn with_tls(mut self, tls: &TlsOptions) -> Result<Self, ClientError> {
        self.http = tls.http_client()?;
        Ok(self)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Reuse an already issued token (for example one cached on disk) instead
    /// of calling [`DeviceClient::authenticate`] again.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Fetch a device access token.
    ///
    /// Fails with [`ClientError::MissingAuthSecret`] when no shared secret was
    /// configured, because the server refuses unsigned requests.
    pub async fn authenticate(&mut self) -> Result<String, ClientError> {
        let secret = self
            .auth_secret
            .as_deref()
            .ok_or(ClientError::MissingAuthSecret)?;
        let signature = DeviceSignature::sign_now(secret, &self.device_id);

        let url = self.endpoint(&["api", "dogGateway", "auth", &self.device_id])?;
        let mut request = self.http.get(url);
        for (name, value) in signature.headers() {
            request = request.header(name, value);
        }

        let response = request.send().await?;
        let envelope: ApiEnvelope<DeviceTokenData> = response.json().await?;
        let data = envelope.into_data()?;
        self.token = Some(data.token.clone());
        Ok(data.token)
    }

    /// Register (or refresh) the device record; requires an access token.
    pub async fn register(
        &self,
        registration: &DeviceRegistration,
    ) -> Result<DeviceSummary, ClientError> {
        let token = self.token.as_deref().ok_or(ClientError::MissingToken)?;
        let url = self.endpoint(&["api", "dogGateway", "register"])?;
        let response = self
            .http
            .post(url)
            .header("Authorization", token)
            .json(registration)
            .send()
            .await?;

        let envelope: ApiEnvelope<DeviceSummary> = response.json().await?;
        envelope.into_data()
    }

    /// Connect to the device WebSocket channel over plain `ws://`.
    pub async fn connect(&self) -> Result<DeviceLink, ClientError> {
        let url = self.websocket_url()?;
        let (stream, _) = tokio_tungstenite::connect_async(url).await?;
        Ok(DeviceLink::new(stream))
    }

    /// Connect over `wss://` using the provided TLS material.
    pub async fn connect_with_tls(&self, tls: &TlsOptions) -> Result<DeviceLink, ClientError> {
        let url = self.websocket_url()?;
        let connector = tls.connector()?;
        let (stream, _) =
            tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(connector))
                .await?;
        Ok(DeviceLink::new(stream))
    }

    /// Build an absolute url from path segments, percent-encoding each segment.
    fn endpoint(&self, segments: &[&str]) -> Result<Url, ClientError> {
        let mut url = Url::parse(&self.base_url)
            .map_err(|error| ClientError::Url(format!("{}: {error}", self.base_url)))?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| ClientError::Url("base url cannot be used as a base".to_string()))?;
            path.clear();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }

    /// Build `ws(s)://host/api/notice/{deviceId}/{token}` from the HTTP base url.
    pub fn websocket_url(&self) -> Result<String, ClientError> {
        let token = self.token.as_deref().ok_or(ClientError::MissingToken)?;

        let mut url = Url::parse(&self.base_url)
            .map_err(|error| ClientError::Url(format!("{}: {error}", self.base_url)))?;

        let scheme: &'static str = match url.scheme() {
            "http" | "ws" => "ws",
            "https" | "wss" => "wss",
            other => {
                return Err(ClientError::Url(format!(
                    "unsupported scheme `{other}`, expected http or https"
                )));
            }
        };
        url.set_scheme(scheme)
            .map_err(|_| ClientError::Url(format!("cannot apply scheme `{scheme}`")))?;

        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| ClientError::Url("base url cannot be used as a base".to_string()))?;
            segments.clear();
            segments.push("api");
            segments.push("notice");
            segments.push(&self.device_id);
            segments.push(token);
        }

        Ok(url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_url_is_derived_from_http_base() {
        let client = DeviceClient::new("http://127.0.0.1:3000/", "dog-1").with_token("abc123");
        assert_eq!(
            client.websocket_url().unwrap(),
            "ws://127.0.0.1:3000/api/notice/dog-1/abc123"
        );
    }

    #[test]
    fn websocket_url_upgrades_https_to_wss() {
        let client = DeviceClient::new("https://agent:4433", "dog-1").with_token("t");
        assert_eq!(
            client.websocket_url().unwrap(),
            "wss://agent:4433/api/notice/dog-1/t"
        );
    }

    #[test]
    fn websocket_url_encodes_device_id() {
        let client = DeviceClient::new("http://localhost:3000", "dog 1/2").with_token("t");
        let url = client.websocket_url().unwrap();
        assert!(url.contains("dog%201%2F2"), "unexpected url: {url}");
    }

    #[test]
    fn websocket_url_requires_a_token() {
        let client = DeviceClient::new("http://localhost:3000", "dog-1");
        assert!(client.websocket_url().is_err());
    }
}
