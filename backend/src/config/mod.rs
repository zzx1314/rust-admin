use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub password: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct HarborConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    #[serde(default = "default_staging_project")]
    pub staging_project: String,
    #[serde(default = "default_production_project")]
    pub production_project: String,
    pub registry_endpoint_id: Option<i64>,
    pub registry_insecure: Option<bool>,
    pub webhook_secret: Option<String>,
    #[serde(default = "default_replication_timeout")]
    pub replication_timeout_secs: u64,
    #[serde(default = "default_replication_poll_interval")]
    pub replication_poll_interval_secs: u64,
}

fn default_staging_project() -> String {
    "staging-project".to_string()
}

fn default_production_project() -> String {
    "production-project".to_string()
}

fn default_replication_timeout() -> u64 {
    30
}

fn default_replication_poll_interval() -> u64 {
    1
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub redis: RedisConfig,
    pub jwt_secret: String,
    pub database_url: String,
    pub harbor: Option<HarborConfig>,
}

impl RedisConfig {
    pub fn url(&self) -> String {
        if self.password.is_empty() {
            format!("redis://{}:{}", self.host, self.port)
        } else {
            format!("redis://:{}@{}:{}", self.password, self.host, self.port)
        }
    }
}

impl AppConfig {
    pub fn new() -> Result<Self, ConfigError> {
        let config_path = format!("{}/config/config.toml", env!("CARGO_MANIFEST_DIR"));

        let config = Config::builder()
            .add_source(File::with_name(&config_path).required(true))
            .add_source(
                Environment::with_prefix("APP")
                    .prefix_separator("_")
                    .separator("__"),
            )
            .build()?;

        config.try_deserialize()
    }
}
