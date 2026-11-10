use std::path::PathBuf;

use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;

/// Environment variable that points at the config file explicitly. Highest
/// precedence so that a deployed binary can be relocated freely.
pub const CONFIG_PATH_ENV: &str = "APP_CONFIG_FILE";

/// Secrets are never read from `config.toml`; they must come from the
/// environment. See `config/config.toml.example`.
pub const JWT_SECRET_ENV: &str = "APP_JWT_SECRET";
pub const REDIS_PASSWORD_ENV: &str = "APP_REDIS_PASSWORD";
pub const DEVICE_AUTH_SECRET_ENV: &str = "APP_DEVICE_AUTH_SECRET";

/// Minimum accepted length for `jwt_secret` outside of tests.
const MIN_JWT_SECRET_LEN: usize = 32;

#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    /// Password injected from `APP_REDIS_PASSWORD`; empty means no auth.
    #[serde(default)]
    pub password: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct DogConfig {
    /// Directory where files uploaded by devices are stored. `None` (or an
    /// empty string) means "use the `App` default".
    #[serde(default)]
    pub upload_dir: Option<String>,
    /// Shared secret used to verify `GET /api/dogGateway/auth/{deviceId}`
    /// requests. Injected from `APP_DEVICE_AUTH_SECRET`.
    #[serde(default)]
    pub device_auth_secret: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub redis: RedisConfig,
    /// Injected from `APP_JWT_SECRET`; never present in `config.toml`.
    pub jwt_secret: String,
    pub database_url: String,
    /// Optional `[dog]` section; device uploads default to `data/device_uploads`.
    #[serde(default)]
    pub dog: DogConfig,
    /// Shared secret for device gateway authentication, injected from
    /// `APP_DEVICE_AUTH_SECRET`. Kept as a top-level field so that deployments
    /// that only use `config.toml` for non-secret values can still supply it.
    #[serde(default)]
    pub device_auth_secret: String,
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
        Self::load(Self::locate_config_file()?)
    }

    /// Load configuration from an explicit path.
    ///
    /// Precedence (low to high): the TOML file, then `APP_*` environment
    /// variables. Secrets live only in the environment, so a checked-in
    /// `config.toml` never contains credentials.
    pub fn load(config_path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let config_path = config_path.into();

        let builder = Config::builder()
            .add_source(
                File::from(config_path.clone())
                    // A missing file is tolerated so that a deployment can run
                    // purely from environment variables.
                    .required(false),
            )
            .add_source(
                Environment::with_prefix("APP")
                    .prefix_separator("_")
                    .separator("__")
                    .try_parsing(true),
            );

        let mut config: Self = builder.build()?.try_deserialize().map_err(|error| {
            // Secrets are intentionally absent from `config.toml`; point the
            // operator at the environment variable instead of a bare
            // "missing field" error.
            let message = error.to_string();
            if message.contains("jwt_secret") {
                ConfigError::Message(format!(
                    "`jwt_secret` is missing. Export `{JWT_SECRET_ENV}` \
                     (for example `export {JWT_SECRET_ENV}=$(openssl rand -hex 32)`)."
                ))
            } else if message.contains("device_auth_secret") {
                ConfigError::Message(format!(
                    "`device_auth_secret` is missing. Export `{DEVICE_AUTH_SECRET_ENV}` \
                     (for example `export {DEVICE_AUTH_SECRET_ENV}=$(openssl rand -hex 32)`)."
                ))
            } else {
                ConfigError::Message(message)
            }
        })?;
        config.normalise();
        config.validate()?;
        Ok(config)
    }

    /// Resolve the config file location.
    ///
    /// Order: `APP_CONFIG_FILE`, then `<cwd>/config/config.toml`, then
    /// `<exe dir>/config/config.toml`. `CARGO_MANIFEST_DIR` is deliberately
    /// not used: it would bake the build machine's absolute path into the
    /// binary and break relocation.
    pub fn locate_config_file() -> Result<PathBuf, ConfigError> {
        if let Ok(explicit) = std::env::var(CONFIG_PATH_ENV)
            && !explicit.trim().is_empty()
        {
            return Ok(PathBuf::from(explicit));
        }

        let mut candidates: Vec<PathBuf> = vec![PathBuf::from("config/config.toml")];

        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("config").join("config.toml"));
        }
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            candidates.push(dir.join("config").join("config.toml"));
        }

        // Fall back to the first candidate so that `load` can report a clear
        // error instead of `locate_config_file` guessing wrong.
        Ok(candidates
            .into_iter()
            .find(|candidate| candidate.exists())
            .unwrap_or_else(|| PathBuf::from("config/config.toml")))
    }

    /// Move `APP_DEVICE_AUTH_SECRET` / `APP_REDIS_PASSWORD` into place when the
    /// environment provides them but the file does not (or the other way
    /// round for the `[dog] device_auth_secret` alias).
    fn normalise(&mut self) {
        if self.device_auth_secret.is_empty()
            && let Some(secret) = self.dog.device_auth_secret.take()
        {
            self.device_auth_secret = secret;
        }

        // `redis.password` is a nested key, so the environment source only
        // maps it from `APP_REDIS__PASSWORD`. Honour the documented flat
        // `APP_REDIS_PASSWORD` name as well, otherwise Redis AUTH is silently
        // skipped and every login fails with `NOAUTH`.
        if self.redis.password.is_empty()
            && let Ok(password) = std::env::var(REDIS_PASSWORD_ENV)
            && !password.trim().is_empty()
        {
            self.redis.password = password;
        }
    }

    fn validate(&mut self) -> Result<(), ConfigError> {
        self.jwt_secret = self.jwt_secret.trim().to_string();
        if self.jwt_secret.is_empty() {
            return Err(ConfigError::Message(format!(
                "`jwt_secret` is empty. Set the `{JWT_SECRET_ENV}` environment variable."
            )));
        }
        if self.jwt_secret == "your-secret-key" {
            return Err(ConfigError::Message(format!(
                "`jwt_secret` is still the example value. Set a real secret via `{JWT_SECRET_ENV}`."
            )));
        }
        if self.jwt_secret.len() < MIN_JWT_SECRET_LEN {
            return Err(ConfigError::Message(format!(
                "`jwt_secret` is shorter than {MIN_JWT_SECRET_LEN} bytes. Generate one with \
                 `openssl rand -hex 32` and set `{JWT_SECRET_ENV}`."
            )));
        }

        self.device_auth_secret = self.device_auth_secret.trim().to_string();
        if self.device_auth_secret.is_empty() {
            return Err(ConfigError::Message(format!(
                "`device_auth_secret` is empty. Set the `{DEVICE_AUTH_SECRET_ENV}` environment \
                 variable; it signs `GET /api/dogGateway/auth/{{deviceId}}` requests."
            )));
        }

        Ok(())
    }

    /// Shared secret used to authenticate device token requests.
    pub fn device_auth_secret(&self) -> &str {
        &self.device_auth_secret
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml() -> &'static str {
        r#"
jwt_secret = "0123456789abcdef0123456789abcdef"
device_auth_secret = "device-secret"
database_url = "sqlite:data/test.db?mode=rwc"

[server]
host = "127.0.0.1"
port = 3000

[redis]
host = "localhost"
port = 6379

[dog]
upload_dir = "/tmp/rust_admin_config_test"
"#
    }

    fn write_temp_config(contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rust_admin_config_{}.toml",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&path, contents).expect("write temp config");
        path
    }

    #[test]
    fn loads_a_config_file_with_a_dog_section() {
        let path = write_temp_config(minimal_toml());
        let config = AppConfig::load(&path).expect("config must parse");

        assert_eq!(config.server.port, 3000);
        assert_eq!(
            config.dog.upload_dir.as_deref(),
            Some("/tmp/rust_admin_config_test")
        );
        assert_eq!(config.device_auth_secret(), "device-secret");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_the_example_jwt_secret() {
        let toml = minimal_toml().replace("0123456789abcdef0123456789abcdef", "your-secret-key");
        let path = write_temp_config(&toml);

        let error = AppConfig::load(&path).expect_err("example secret must be rejected");
        assert!(error.to_string().contains(JWT_SECRET_ENV), "got: {error}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_a_short_jwt_secret() {
        let toml = minimal_toml().replace("0123456789abcdef0123456789abcdef", "short");
        let path = write_temp_config(&toml);

        let error = AppConfig::load(&path).expect_err("short secret must be rejected");
        assert!(error.to_string().contains("shorter than"), "got: {error}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_a_missing_device_auth_secret() {
        let toml = minimal_toml().replace("device_auth_secret = \"device-secret\"", "");
        let path = write_temp_config(&toml);

        let error = AppConfig::load(&path).expect_err("missing device secret must be rejected");
        assert!(
            error.to_string().contains(DEVICE_AUTH_SECRET_ENV),
            "got: {error}"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_tolerated_but_validation_still_applies() {
        let path = std::env::temp_dir().join("rust_admin_definitely_missing_config.toml");
        std::fs::remove_file(&path).ok();

        // Nothing on disk and no env vars: the mandatory sections are absent,
        // so loading fails with a deserialisation error rather than a panic.
        let error = AppConfig::load(&path).expect_err("empty config must not load");
        assert!(error.to_string().contains("missing field"), "got: {error}");
    }

    #[test]
    fn missing_secrets_surface_as_validation_errors() {
        // A file that only carries non-secret values: validation must point at
        // the environment variable instead of silently accepting an empty key.
        let toml = minimal_toml()
            .replace("jwt_secret = \"0123456789abcdef0123456789abcdef\"\n", "")
            .replace("device_auth_secret = \"device-secret\"\n", "");
        let path = write_temp_config(&toml);

        let error = AppConfig::load(&path).expect_err("secrets are mandatory");
        assert!(error.to_string().contains("jwt_secret"), "got: {error}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn adopts_the_flat_redis_password_env_name() {
        let path = write_temp_config(minimal_toml());

        // SAFETY: no other test reads this variable.
        unsafe { std::env::set_var(REDIS_PASSWORD_ENV, "from-env") };
        let config = AppConfig::load(&path).expect("config must parse");
        unsafe { std::env::remove_var(REDIS_PASSWORD_ENV) };

        assert_eq!(config.redis.password, "from-env");
        assert_eq!(config.redis.url(), "redis://:from-env@localhost:6379");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn locate_config_file_honours_the_override_env() {
        let path = write_temp_config(minimal_toml());

        // SAFETY: assertions in this test only observe the value we just set.
        unsafe { std::env::set_var(CONFIG_PATH_ENV, &path) };
        assert_eq!(AppConfig::locate_config_file().unwrap(), path);
        unsafe { std::env::remove_var(CONFIG_PATH_ENV) };

        std::fs::remove_file(&path).ok();
    }
}
