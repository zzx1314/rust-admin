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
        }
    }

    pub fn database_url(&self) -> String {
        format!("sqlite://{}", self.database_path.display())
    }
}
