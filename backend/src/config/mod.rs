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
    /// 前端构建产物目录（`frontend/dist`）。
    ///
    /// 配好后本服务会在 `/api` 之外直接托管前端，SPA 路由回落到 `index.html`，
    /// 因此生产环境不再需要 nginx。省略或留空表示纯 API 模式（配合
    /// `pnpm dev` 使用）。环境变量：`APP_SERVER__STATIC_DIR`。
    #[serde(default)]
    pub static_dir: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct DogConfig {
    /// Directory where files uploaded by devices are stored. `None` (or an
    /// empty string) means "use the `App` default".
    #[serde(default)]
    pub upload_dir: Option<String>,
    /// 业务平台（`business/server`）地址；中台设备页「同步地图」通过它向设备下发指令。
    /// 为空时该按钮返回未配置错误。
    #[serde(default)]
    pub business_api_url: Option<String>,
    /// 调用业务平台内部接口的服务令牌，必须与业务平台 `ROBODOG_SERVICE_TOKEN` 一致。
    /// 建议通过 `APP_DOG__BUSINESS_API_TOKEN` 环境变量注入，不写入配置文件。
    #[serde(default)]
    pub business_api_token: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    /// Redis 配置。`token_store = "memory"` 时可以整段省掉。
    #[serde(default)]
    pub redis: Option<RedisConfig>,
    /// 登录会话（access / refresh 令牌）存在哪里。
    ///
    /// 默认 Redis；设 `APP_TOKEN_STORE=memory` 后改用进程内存，不再需要
    /// redis-server（代价：重启后所有人重新登录，也无法多实例共享会话）。
    #[serde(default)]
    pub token_store: TokenStoreKind,
    /// Injected from `APP_JWT_SECRET`; never present in `config.toml`.
    pub jwt_secret: String,
    pub database_url: String,
    /// Optional `[dog]` section; device uploads default to `data/device_uploads`.
    #[serde(default)]
    pub dog: DogConfig,
}

/// 会话令牌的存储后端。
#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TokenStoreKind {
    /// 默认：Redis。进程重启不掉线，支持多实例共享会话。
    #[default]
    Redis,
    /// 进程内内存。不需要 Redis，适合单实例部署；重启即全体重新登录。
    Memory,
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

    /// Move `APP_REDIS_PASSWORD` into place when the environment provides it
    /// but the file does not.
    fn normalise(&mut self) {
        // `redis.password` 是嵌套键，环境变量源只会从 `APP_REDIS__PASSWORD`
        // 读取。这里额外尊重文档里的扁平写法 `APP_REDIS_PASSWORD`，否则会
        // 静默跳过 AUTH，导致每次登录都报 `NOAUTH`。
        if let Some(redis) = self.redis.as_mut()
            && redis.password.is_empty()
            && let Ok(password) = std::env::var(REDIS_PASSWORD_ENV)
            && !password.trim().is_empty()
        {
            redis.password = password;
        }

        // `static_dir = ""`（或 `APP_SERVER__STATIC_DIR=`）等价于「不托管前端」，
        // 免得把当前目录当成站点根目录整盘暴露出去。
        self.server.static_dir = self
            .server
            .static_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(str::to_string);
    }

    /// 前端静态目录。为空表示不托管前端（纯 API 模式）。
    pub fn static_dir(&self) -> Option<PathBuf> {
        self.server.static_dir.as_deref().map(PathBuf::from)
    }

    /// Redis 连接串。只有 `token_store = "redis"` 时才会用到；
    /// 缺 `[redis]` 段时回落到 `localhost:6379`（`validate` 已提前报错）。
    pub fn redis_url(&self) -> String {
        self.redis
            .as_ref()
            .map(RedisConfig::url)
            .unwrap_or_else(|| "redis://localhost:6379".to_string())
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

        // `token_store = "memory"` 只能用单实例；Redis 模式必须真的配了 Redis，
        // 否则服务能启动但每次登录都失败，错误信息会很难定位。
        if self.token_store == TokenStoreKind::Redis && self.redis.is_none() {
            return Err(ConfigError::Message(format!(
                "`[redis]` 段缺失：`token_store = \"redis\"`（默认值）需要 Redis 连接信息。\
                 \n  · 单机部署没有 Redis？设 `APP_TOKEN_STORE=memory` 改用进程内存存储；\
                 \n  · 或者补上 `[redis]` 的 host / port（密码用 {REDIS_PASSWORD_ENV} 注入）。"
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_toml() -> &'static str {
        r#"
jwt_secret = "0123456789abcdef0123456789abcdef"
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

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn static_dir_defaults_to_none_and_blank_means_disabled() {
        // 没写 static_dir：纯 API 模式，不托管任何静态文件。
        let path = write_temp_config(minimal_toml());
        let config = AppConfig::load(&path).expect("config must parse");
        assert!(config.server.static_dir.is_none());
        assert!(config.static_dir().is_none());
        std::fs::remove_file(&path).ok();

        // 空字符串（含 `APP_SERVER__STATIC_DIR=` 的写法）同样视为未配置，
        // 否则会把当前工作目录当成站点根目录。
        let toml = minimal_toml().replace("[server]", "[server]\nstatic_dir = \"  \"");
        let path = write_temp_config(&toml);
        let config = AppConfig::load(&path).expect("config must parse");
        assert!(
            config.server.static_dir.is_none(),
            "空字符串必须被规整成 None"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn static_dir_is_read_from_the_server_section() {
        let toml = minimal_toml().replace("[server]", "[server]\nstatic_dir = \"web\"");
        let path = write_temp_config(&toml);
        let config = AppConfig::load(&path).expect("config must parse");

        assert_eq!(config.server.static_dir.as_deref(), Some("web"));
        assert_eq!(config.static_dir(), Some(PathBuf::from("web")));

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
        let toml =
            minimal_toml().replace("jwt_secret = \"0123456789abcdef0123456789abcdef\"\n", "");
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

        assert_eq!(
            config.redis.as_ref().map(|redis| redis.password.as_str()),
            Some("from-env")
        );
        assert_eq!(config.redis_url(), "redis://:from-env@localhost:6379");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn memory_token_store_needs_no_redis_section() {
        // 单机部署：不写 [redis]，只要把存储换到内存就能启动。
        // `token_store` 是根级键，必须放在 `[server]` 之前。
        let toml = format!(
            "token_store = \"memory\"\n{}",
            minimal_toml().replace("[redis]\nhost = \"localhost\"\nport = 6379\n", "")
        );
        let path = write_temp_config(&toml);
        let config = AppConfig::load(&path).expect("memory 模式不要求 [redis]");

        assert_eq!(config.token_store, TokenStoreKind::Memory);
        assert!(config.redis.is_none());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn token_store_defaults_to_redis_and_requires_a_redis_section() {
        // 默认仍是 Redis；漏配 [redis] 时必须报错讲清楚怎么处理，
        // 而不是让服务启动后每次登录都失败。
        let toml = minimal_toml().replace("[redis]\nhost = \"localhost\"\nport = 6379\n", "");
        let path = write_temp_config(&toml);

        let error = AppConfig::load(&path).expect_err("Redis 模式缺少 [redis] 必须报错");
        let message = error.to_string();
        assert!(message.contains("APP_TOKEN_STORE=memory"), "got: {message}");

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
