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
pub struct DogConfig {
    /// Directory where files uploaded by devices are stored.
    #[serde(default = "default_dog_upload_dir")]
    pub upload_dir: String,
}

fn default_dog_upload_dir() -> String {
    "data/device_uploads".to_string()
}

impl Default for DogConfig {
    fn default() -> Self {
        Self {
            upload_dir: default_dog_upload_dir(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub redis: RedisConfig,
    pub jwt_secret: String,
    pub database_url: String,
    /// Optional `[dog]` section; device uploads default to `data/device_uploads`.
    #[serde(default)]
    pub dog: DogConfig,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_config_file_with_dog_section() {
        let config = AppConfig::new().expect("config/config.toml must stay parseable");
        assert!(!config.database_url.is_empty());
        assert_eq!(config.dog.upload_dir, "data/device_uploads");
    }
}
