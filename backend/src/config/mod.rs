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
pub struct AppConfig {
    pub server: ServerConfig,
    pub redis: RedisConfig,
    pub jwt_secret: String,
    pub database_url: String,
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
            .add_source(Environment::with_prefix("APP").separator("_"))
            .build()?;

        config.try_deserialize()
    }
}
