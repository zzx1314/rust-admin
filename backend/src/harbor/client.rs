use crate::config::HarborConfig;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, CONTENT_TYPE, AUTHORIZATION};

#[derive(Clone)]
pub struct HarborClient {
    pub client: reqwest::Client,
    pub base_url: String,
    auth_header: String,
    pub enabled: bool,
}

impl HarborClient {
    pub fn new(config: &HarborConfig) -> Self {
        let enabled = !config.url.is_empty() && !config.username.is_empty();
        let auth_header = format!(
            "Basic {}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                format!("{}:{}", config.username, config.password).as_bytes()
            )
        );

        Self {
            client: reqwest::Client::new(),
            base_url: config.url.trim_end_matches('/').to_string(),
            auth_header,
            enabled,
        }
    }

    pub fn default_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&self.auth_header).unwrap(),
        );
        headers
    }

    pub fn api_url(&self, path: &str) -> String {
        let path = path.trim_start_matches('/');
        format!("{}/{}", self.base_url, path)
    }
}
