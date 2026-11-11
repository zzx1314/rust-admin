use std::env;
use std::path::PathBuf;

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
}

impl Config {
    pub fn from_env() -> Self {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let database_path = env::var("ROBODOG_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|_| manifest_dir.join("data/robodog.db"));
        let web_dist = env::var("ROBODOG_WEB_DIST")
            .map(PathBuf::from)
            .unwrap_or_else(|_| manifest_dir.join("../web/dist"));
        let backend_db_path = env::var("ROBODOG_BACKEND_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|_| manifest_dir.join("../../backend/data/users.db"));
        let ingest_dir = env::var("ROBODOG_INGEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| manifest_dir.join("data/map_ingest"));
        let blank_to_none = |value: String| {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        Self {
            port: env::var("ROBODOG_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(3100),
            database_path,
            jwt_secret: env::var("ROBODOG_JWT_SECRET")
                .unwrap_or_else(|_| "robodog-ops-development-secret-change-me".to_string()),
            jwt_ttl_hours: env::var("ROBODOG_JWT_TTL_HOURS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(24),
            web_dist,
            backend_db_path,
            zentai_base_url: env::var("ROBODOG_ZENTAI_URL")
                .ok()
                .and_then(blank_to_none)
                .map(|url| url.trim_end_matches('/').to_string()),
            zentai_username: env::var("ROBODOG_ZENTAI_USER").ok().and_then(blank_to_none),
            zentai_password: env::var("ROBODOG_ZENTAI_PASSWORD")
                .ok()
                .and_then(blank_to_none),
            ingest_dir,
            service_token: env::var("ROBODOG_SERVICE_TOKEN")
                .ok()
                .and_then(blank_to_none),
        }
    }

    pub fn database_url(&self) -> String {
        format!("sqlite://{}", self.database_path.display())
    }

    /// 中台回传是否就绪（地址 + 账号齐全）
    pub fn zentai_ready(&self) -> bool {
        self.zentai_base_url.is_some()
            && self.zentai_username.is_some()
            && self.zentai_password.is_some()
    }
}
