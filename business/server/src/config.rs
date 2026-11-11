//! 业务平台配置：**环境变量 > `server.toml` > 内置默认值**。
//!
//! 取向与设备端 `business/client` 一致（那边还多一层 CLI），只是服务端没有
//! 命令行参数：
//!
//! - 配置文件路径：`ROBODOG_SERVER_CONFIG`；未设置时尝试当前目录的 `server.toml`。
//!   默认文件不存在视为「只用环境变量」，静默跳过；**显式指定的文件不存在直接报错**，
//!   避免部署时把地址写错却被悄悄忽略。
//! - 环境变量永远优先，方便 systemd / 容器只覆盖个别项。
//! - 文件里的相对路径按**配置文件所在目录**解析，部署时不依赖当前工作目录。
//! - 未知字段报错：拼错的键静默失效比启动失败更难排查。
//! - 密钥（`jwt_secret` / `zentai.password` / `service_token`）仍建议只走环境变量，
//!   文件里放地址类配置。

use serde::Deserialize;
use std::env;
use std::path::{Path, PathBuf};

/// 默认配置文件（相对当前工作目录）。
pub const DEFAULT_CONFIG_FILE: &str = "server.toml";
/// 指定配置文件路径的环境变量。
pub const CONFIG_FILE_ENV: &str = "ROBODOG_SERVER_CONFIG";

#[derive(Clone, Debug)]
pub struct Config {
    pub port: u16,
    pub database_path: PathBuf,
    pub jwt_secret: String,
    pub jwt_ttl_hours: i64,
    pub web_dist: PathBuf,
    /// 根目录 backend 的 SQLite 文件，用于导入 dog_map 中已上传的 ROS 地图。
    pub backend_db_path: PathBuf,
    /// 中台（根 backend）服务地址，用于把机器人建图自动推入 dog_map。
    /// 为空时禁用自动回传，`/api/map-versions/ingest` 会返回 503。
    pub zentai_base_url: Option<String>,
    /// 中台登录账号（需具备地图上传权限）
    pub zentai_username: Option<String>,
    pub zentai_password: Option<String>,
    /// 自动上传的地图包本地暂存目录
    pub ingest_dir: PathBuf,
    /// 中台调用业务平台内部接口的服务令牌；为空时内部触发接口拒绝请求。
    pub service_token: Option<String>,
    /// 反向控制信道的中台地址（`ws://` / `wss://`）。
    ///
    /// 业务平台部署在内网、只能出站访问互联网时必须配置：中台无法主动连入，
    /// 改由本进程主动拨入中台的长连接接收指令。为空时保持「中台直连 HTTP」模式。
    pub zentai_ws_url: Option<String>,
    /// 设备心跳超时秒数：超过这个时长没心跳就把设备置为「离线」（看门狗）。
    ///
    /// 客户端默认 10s 一次心跳，取 9 倍留出网络重连窗口；WS 静默断开（拉闸、掉线）
    /// 收不到 Close 帧，没有看门狗会一直显示「在线」。
    pub device_timeout_secs: i64,
}

/// 配置文件读取 / 解析失败。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("配置文件 {path} 不存在（由 {env} 指定）")]
    Missing { path: String, env: String },
    #[error("配置文件 {path} 读取失败: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("配置文件 {path} 解析失败: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

/// `server.toml` 的结构。全部可选：缺省项回落到内置默认值。
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ServerFile {
    port: Option<u16>,
    database: Option<String>,
    jwt_secret: Option<String>,
    jwt_ttl_hours: Option<i64>,
    web_dist: Option<String>,
    backend_db: Option<String>,
    ingest_dir: Option<String>,
    device_timeout_secs: Option<i64>,
    zentai: ZentaiFile,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ZentaiFile {
    /// 反向控制信道地址（内网部署必需）
    ws_url: Option<String>,
    /// 中台 HTTP 地址（建图回传、账号登录、设备同步用）
    base_url: Option<String>,
    username: Option<String>,
    password: Option<String>,
    service_token: Option<String>,
}

impl ServerFile {
    fn read(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })
    }
}

impl Config {
    /// 正式入口：按 环境变量 > `server.toml` > 默认值 组装配置。
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_file_path(env::var(CONFIG_FILE_ENV).ok())?;
        let (file, base_dir) = match &path {
            Some(path) => (Some(ServerFile::read(path)?), base_dir_of(path)),
            None => (None, None),
        };
        Ok(Self::merge(file, base_dir, |name| env::var(name).ok()))
    }

    /// 只用环境变量的入口（忽略配置文件），兼容旧部署与脚本。
    pub fn from_env() -> Self {
        Self::merge(None, None, |name| env::var(name).ok())
    }

    fn merge<F>(file: Option<ServerFile>, base_dir: Option<PathBuf>, env: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let file = file.unwrap_or_default();
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        let text = |name: &str, from_file: Option<String>, default: String| -> String {
            env_text(&env, name)
                .or_else(|| from_file.and_then(clean))
                .unwrap_or(default)
        };
        let path = |name: &str, from_file: Option<String>, default: PathBuf| -> PathBuf {
            match env_text(&env, name) {
                Some(value) => PathBuf::from(value),
                None => file_path(from_file, base_dir.as_deref(), default),
            }
        };
        let url = |name: &str, from_file: Option<String>| -> Option<String> {
            env_text(&env, name)
                .or_else(|| from_file.and_then(clean))
                .map(|value| value.trim_end_matches('/').to_string())
        };

        Self {
            port: env_text(&env, "ROBODOG_PORT")
                .and_then(|value| value.parse().ok())
                .or(file.port)
                .unwrap_or(3100),
            database_path: path(
                "ROBODOG_DB",
                file.database,
                manifest_dir.join("data/robodog.db"),
            ),
            jwt_secret: text(
                "ROBODOG_JWT_SECRET",
                file.jwt_secret,
                "robodog-ops-development-secret-change-me".to_string(),
            ),
            jwt_ttl_hours: env_text(&env, "ROBODOG_JWT_TTL_HOURS")
                .and_then(|value| value.parse().ok())
                .or(file.jwt_ttl_hours)
                .unwrap_or(24),
            device_timeout_secs: env_text(&env, "ROBODOG_DEVICE_TIMEOUT_SECS")
                .and_then(|value| value.parse().ok())
                .or(file.device_timeout_secs)
                .unwrap_or(90)
                .max(20),
            web_dist: path(
                "ROBODOG_WEB_DIST",
                file.web_dist,
                manifest_dir.join("../web/dist"),
            ),
            backend_db_path: path(
                "ROBODOG_BACKEND_DB",
                file.backend_db,
                manifest_dir.join("../../backend/data/users.db"),
            ),
            zentai_base_url: url("ROBODOG_ZENTAI_URL", file.zentai.base_url),
            zentai_username: env_text(&env, "ROBODOG_ZENTAI_USER")
                .or_else(|| file.zentai.username.and_then(clean)),
            zentai_password: env_text(&env, "ROBODOG_ZENTAI_PASSWORD")
                .or_else(|| file.zentai.password.and_then(clean)),
            ingest_dir: path(
                "ROBODOG_INGEST_DIR",
                file.ingest_dir,
                manifest_dir.join("data/map_ingest"),
            ),
            service_token: env_text(&env, "ROBODOG_SERVICE_TOKEN")
                .or_else(|| file.zentai.service_token.and_then(clean)),
            zentai_ws_url: url("ROBODOG_ZENTAI_WS_URL", file.zentai.ws_url),
        }
    }

    pub fn database_url(&self) -> String {
        format!("sqlite://{}", self.database_path.display())
    }

    /// 与中台通信是否就绪：地址 + （共享服务令牌 或 服务账号）。
    ///
    /// 只要有令牌就走内部接口 `/api/dogInternal/*`，不需要中台账号。
    pub fn zentai_ready(&self) -> bool {
        self.zentai_base_url.is_some()
            && (self.has_service_token()
                || (self.zentai_username.is_some() && self.zentai_password.is_some()))
    }

    /// 是否配置了共享服务令牌（决定走内部接口还是账号登录）。
    pub fn has_service_token(&self) -> bool {
        self.service_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    }

    /// 中台账号登录是否可用（业务平台代理登录中台时需要真实账号）。
    ///
    /// 只配服务令牌时该值为 `false`：设备台账 / 地图回传仍然工作，
    /// 但「用中台账号登录业务平台」这条路径关闭。
    pub fn zentai_account_ready(&self) -> bool {
        self.zentai_base_url.is_some()
            && self.zentai_username.is_some()
            && self.zentai_password.is_some()
    }

    /// 与中台通信的鉴权方式，用于健康检查与日志。
    pub fn zentai_auth_mode(&self) -> &'static str {
        if self.zentai_base_url.is_none() {
            "none"
        } else if self.has_service_token() {
            "service-token"
        } else if self.zentai_username.is_some() && self.zentai_password.is_some() {
            "account"
        } else {
            "incomplete"
        }
    }

    /// 反向控制信道是否可用（地址 + 共享令牌齐全）
    pub fn control_channel_ready(&self) -> bool {
        self.zentai_ws_url.is_some() && self.service_token.is_some()
    }
}

/// 定位配置文件：显式指定的必须存在，默认路径缺失视为「不读文件」。
fn config_file_path(explicit: Option<String>) -> Result<Option<PathBuf>, ConfigError> {
    match explicit.and_then(clean) {
        Some(value) => {
            let path = PathBuf::from(value);
            if !path.is_file() {
                return Err(ConfigError::Missing {
                    path: path.display().to_string(),
                    env: CONFIG_FILE_ENV.to_string(),
                });
            }
            Ok(Some(path))
        }
        None => {
            let default = PathBuf::from(DEFAULT_CONFIG_FILE);
            Ok(default.is_file().then_some(default))
        }
    }
}

/// 配置文件所在目录（相对路径的解析基准）。
fn base_dir_of(path: &Path) -> Option<PathBuf> {
    path.parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

fn env_text<F>(env: &F, name: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    env(name).and_then(clean)
}

fn clean(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 文件里的相对路径按配置文件目录解析；绝对路径原样使用。
fn file_path(from_file: Option<String>, base_dir: Option<&Path>, default: PathBuf) -> PathBuf {
    match from_file.and_then(clean) {
        Some(value) => {
            let path = PathBuf::from(value);
            match base_dir {
                Some(dir) if path.is_relative() => dir.join(path),
                _ => path,
            }
        }
        None => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ServerFile {
        toml::from_str(text).expect("toml must parse")
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_apply_without_file_or_env() {
        let config = Config::merge(None, None, no_env);
        assert_eq!(config.port, 3100);
        assert_eq!(config.jwt_ttl_hours, 24);
        assert!(config.zentai_ws_url.is_none());
        assert!(config.service_token.is_none());
        assert!(!config.control_channel_ready());
    }

    #[test]
    fn file_values_are_used() {
        let config = Config::merge(
            Some(parse(
                r#"
                port = 4100
                jwt_ttl_hours = 6
                [zentai]
                ws_url = "ws://10.0.0.8:3000/"
                base_url = "http://10.0.0.8:3000/"
                username = "sysadmin"
                service_token = "from-file"
                "#,
            )),
            None,
            no_env,
        );
        assert_eq!(config.port, 4100);
        assert_eq!(config.jwt_ttl_hours, 6);
        assert_eq!(config.zentai_ws_url.as_deref(), Some("ws://10.0.0.8:3000"));
        assert_eq!(
            config.zentai_base_url.as_deref(),
            Some("http://10.0.0.8:3000")
        );
        assert_eq!(config.service_token.as_deref(), Some("from-file"));
        assert!(config.control_channel_ready());
    }

    #[test]
    fn env_wins_over_file() {
        let env = |name: &str| match name {
            "ROBODOG_PORT" => Some("5000".to_string()),
            "ROBODOG_ZENTAI_WS_URL" => Some("ws://env-host:3000".to_string()),
            _ => None,
        };
        let config = Config::merge(
            Some(parse(
                r#"
                port = 4100
                [zentai]
                ws_url = "ws://file-host:3000"
                service_token = "from-file"
                "#,
            )),
            None,
            env,
        );
        assert_eq!(config.port, 5000);
        assert_eq!(config.zentai_ws_url.as_deref(), Some("ws://env-host:3000"));
        // 环境变量没有覆盖的项仍来自文件
        assert_eq!(config.service_token.as_deref(), Some("from-file"));
    }

    #[test]
    fn blank_env_value_falls_back_to_file() {
        let env = |name: &str| match name {
            "ROBODOG_ZENTAI_WS_URL" => Some("   ".to_string()),
            _ => None,
        };
        let config = Config::merge(
            Some(parse("[zentai]\nws_url = \"ws://file-host:3000\"")),
            None,
            env,
        );
        assert_eq!(config.zentai_ws_url.as_deref(), Some("ws://file-host:3000"));
    }

    #[test]
    fn relative_paths_resolve_against_config_dir() {
        let config = Config::merge(
            Some(parse(
                r#"
                database = "data/x.db"
                web_dist = "dist"
                ingest_dir = "/var/lib/robodog/maps"
                "#,
            )),
            Some(PathBuf::from("/etc/robodog")),
            no_env,
        );
        assert_eq!(
            config.database_path,
            PathBuf::from("/etc/robodog/data/x.db")
        );
        assert_eq!(config.web_dist, PathBuf::from("/etc/robodog/dist"));
        assert_eq!(config.ingest_dir, PathBuf::from("/var/lib/robodog/maps"));
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(toml::from_str::<ServerFile>("prot = 3100").is_err());
        assert!(toml::from_str::<ServerFile>("[zentai]\nwss_url = \"x\"").is_err());
    }

    #[test]
    fn service_token_alone_is_enough_to_talk_to_zentai() {
        let env = |name: &str| match name {
            "ROBODOG_ZENTAI_URL" => Some("http://10.0.0.8:3000".to_string()),
            "ROBODOG_SERVICE_TOKEN" => Some("token-only".to_string()),
            _ => None,
        };
        let config = Config::merge(None, None, env);
        assert!(config.zentai_ready(), "令牌足以完成台账同步与地图回传");
        assert_eq!(config.zentai_auth_mode(), "service-token");
        // 中台账号登录路径关闭（没有账号可代理登录）
        assert!(!config.zentai_account_ready());
    }

    #[test]
    fn account_mode_still_works_without_service_token() {
        let env = |name: &str| match name {
            "ROBODOG_ZENTAI_URL" => Some("http://10.0.0.8:3000".to_string()),
            "ROBODOG_ZENTAI_USER" => Some("sysadmin".to_string()),
            "ROBODOG_ZENTAI_PASSWORD" => Some("secret".to_string()),
            _ => None,
        };
        let config = Config::merge(None, None, env);
        assert!(config.zentai_ready());
        assert!(!config.has_service_token());
        assert!(config.zentai_account_ready());
        assert_eq!(config.zentai_auth_mode(), "account");
    }

    #[test]
    fn address_only_reports_incomplete() {
        let env = |name: &str| match name {
            "ROBODOG_ZENTAI_URL" => Some("http://10.0.0.8:3000".to_string()),
            _ => None,
        };
        let config = Config::merge(None, None, env);
        assert!(!config.zentai_ready(), "没有令牌也没有账号，视为未就绪");
        assert_eq!(config.zentai_auth_mode(), "incomplete");
    }

    #[test]
    fn bundled_example_file_parses() {
        // 仓里的 server.toml.example 必须始终能解析，否则部署教程就是错的
        let file: ServerFile =
            toml::from_str(include_str!("../server.toml.example")).expect("示例配置必须可解析");
        assert_eq!(
            file.zentai.ws_url.as_deref(),
            Some("ws://192.168.41.10:3000")
        );
        assert_eq!(file.port, Some(3100));
    }

    #[test]
    fn explicit_missing_config_file_is_an_error() {
        let error = config_file_path(Some("/nonexistent/server.toml".to_string()))
            .expect_err("显式指定的文件不存在必须报错");
        assert!(matches!(error, ConfigError::Missing { .. }));
    }

    #[test]
    fn default_missing_config_file_is_skipped() {
        // 默认路径在测试工作目录下通常不存在；存在也不影响断言
        let path = config_file_path(None).expect("默认路径缺失不算错误");
        assert!(path.is_none() || path.is_some_and(|path| path.is_file()));
    }
}
