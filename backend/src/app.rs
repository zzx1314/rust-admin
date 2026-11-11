use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::api::{AppState, routes::create_router};
use crate::business::dog::config::repository::SeaOrmDogConfigRepository;
use crate::business::dog::config::service::DogConfigService;
use crate::business::dog::device::repository::SeaOrmDogDeviceRepository;
use crate::business::dog::device::service::DogDeviceService;
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::business::dog::map::service::DogMapService;
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::business::dog::media::service::DogMediaService;
use crate::business::dog::model::repository::SeaOrmDogModelRepository;
use crate::business::dog::model::service::DogModelService;
use crate::business::dog::statistics::service::DogStatisticsService;
use crate::common::error::AppError;
use crate::common::traits::{
    MenuRepository, OrgRepository, RoleRepository, SysDictItemRepository, SysDictRepository,
    SysLogRepository, TokenStore, UserRepository,
};
use crate::config::AppConfig;
use crate::migration::Migrator;
use crate::system::auth::repository::RedisTokenStore;
use crate::system::auth::service::AuthService;
use crate::system::sys_auth::service::SysAuthService;
use crate::system::sys_dict::repository::SeaOrmSysDictRepository;
use crate::system::sys_dict::service::SysDictService;
use crate::system::sys_dict_item::repository::SeaOrmSysDictItemRepository;
use crate::system::sys_dict_item::service::SysDictItemService;
use crate::system::sys_log::repository::SeaOrmSysLogRepository;
use crate::system::sys_log::service::SysLogService;
use crate::system::sys_menu::repository::SeaOrmMenuRepository;
use crate::system::sys_menu::service::MenuService;
use crate::system::sys_org::repository::SeaOrmOrgRepository;
use crate::system::sys_org::service::OrgService;
use crate::system::sys_role::repository::SeaOrmRoleRepository;
use crate::system::sys_role::service::RoleService;
use crate::system::sys_user::repository::SeaOrmUserRepository;
use crate::system::sys_user::service::UserService;
use sea_orm_migration::MigratorTrait;

pub struct App {
    conn: DatabaseConnection,
    /// Directory used to store files uploaded by devices.
    upload_dir: PathBuf,
}

/// Default directory for files uploaded by devices.
pub const DEFAULT_UPLOAD_DIR: &str = "data/device_uploads";

impl App {
    pub async fn new(database_url: &str) -> Result<Self, AppError> {
        let conn = sea_orm::Database::connect(database_url)
            .await
            .map_err(AppError::DatabaseError)?;

        crate::migration::prune_stale_migrations(&conn)
            .await
            .map_err(AppError::DatabaseError)?;

        Migrator::up(&conn, None)
            .await
            .map_err(AppError::DatabaseError)?;

        Ok(Self {
            conn,
            upload_dir: PathBuf::from(DEFAULT_UPLOAD_DIR),
        })
    }

    /// Build an `App` from an already established (and migrated) connection.
    /// Useful for tests that create their own schema.
    pub fn from_connection(conn: DatabaseConnection) -> Self {
        Self {
            conn,
            upload_dir: PathBuf::from(DEFAULT_UPLOAD_DIR),
        }
    }

    /// Same as [`App::from_connection`] with an explicit device upload directory.
    pub fn from_connection_with_upload_dir(
        conn: DatabaseConnection,
        upload_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            conn,
            upload_dir: upload_dir.into(),
        }
    }

    pub fn upload_dir(&self) -> &std::path::Path {
        &self.upload_dir
    }

    pub fn build_state(&self, config: &AppConfig) -> AppState {
        let token_store: Arc<dyn TokenStore> = Arc::new(RedisTokenStore::new(&config.redis.url()));
        // An explicitly configured `[dog] upload_dir` wins; otherwise fall back
        // to the directory the `App` was constructed with so that
        // `from_connection_with_upload_dir` keeps working outside tests.
        let upload_dir = match config.dog.upload_dir.as_deref().map(str::trim) {
            Some(configured) if !configured.is_empty() => PathBuf::from(configured),
            _ => self.upload_dir.clone(),
        };
        self.build_state_full(token_store, &config.jwt_secret, upload_dir)
    }

    /// Build the application state with an explicit token store so that tests
    /// can use an in-memory implementation instead of Redis.
    pub fn build_state_with_token_store(
        &self,
        token_store: Arc<dyn TokenStore>,
        jwt_secret: &str,
    ) -> AppState {
        self.build_state_full(token_store, jwt_secret, self.upload_dir.clone())
    }

    fn build_state_full(
        &self,
        token_store: Arc<dyn TokenStore>,
        jwt_secret: &str,
        upload_dir: PathBuf,
    ) -> AppState {
        let conn = Arc::new(self.conn.clone());

        let role_repo: Arc<dyn RoleRepository> = Arc::new(SeaOrmRoleRepository::new(conn.clone()));
        let role_service = Arc::new(RoleService::new(role_repo.clone()));

        let org_repo: Arc<dyn OrgRepository> = Arc::new(SeaOrmOrgRepository::new(conn.clone()));
        let org_service = Arc::new(OrgService::new(org_repo.clone()));

        let user_repo: Arc<dyn UserRepository> = Arc::new(SeaOrmUserRepository::new(conn.clone()));
        let user_service = Arc::new(UserService::new(
            user_repo.clone(),
            role_repo.clone(),
            org_repo.clone(),
        ));

        let sys_dict_repo: Arc<dyn SysDictRepository> =
            Arc::new(SeaOrmSysDictRepository::new(conn.clone()));
        let sys_dict_service = Arc::new(SysDictService::new(sys_dict_repo.clone()));

        let sys_dict_item_repo: Arc<dyn SysDictItemRepository> =
            Arc::new(SeaOrmSysDictItemRepository::new(conn.clone()));
        let sys_dict_item_service = Arc::new(SysDictItemService::new(
            sys_dict_item_repo,
            sys_dict_repo.clone(),
        ));

        let auth_service = Arc::new(AuthService::new(
            user_repo,
            token_store,
            role_repo.clone(),
            jwt_secret,
            sys_dict_item_service.clone(),
        ));

        let menu_repo: Arc<dyn MenuRepository> = Arc::new(SeaOrmMenuRepository::new(conn.clone()));
        let menu_service = Arc::new(MenuService::new(menu_repo.clone(), role_repo.clone()));

        let sys_auth_service = Arc::new(SysAuthService::new(menu_repo, role_repo.clone()));

        let sys_log_repo: Arc<dyn SysLogRepository> =
            Arc::new(SeaOrmSysLogRepository::new(conn.clone()));
        let sys_log_service = Arc::new(SysLogService::new(sys_log_repo));

        let dog_map_repo = SeaOrmDogMapRepository::new(conn.clone());
        let dog_map_service = Arc::new(DogMapService::new(dog_map_repo.clone()));

        let dog_media_repo = SeaOrmDogMediaRepository::new(conn.clone());
        let dog_media_service = Arc::new(DogMediaService::new(dog_media_repo.clone()));

        let dog_model_repo = SeaOrmDogModelRepository::new(conn.clone());
        let dog_model_service = Arc::new(DogModelService::new(dog_model_repo.clone()));

        let dog_config_repo = SeaOrmDogConfigRepository::new(conn.clone());
        let dog_config_service = Arc::new(DogConfigService::new(dog_config_repo.clone()));

        let dog_device_repo = SeaOrmDogDeviceRepository::new(conn.clone());
        let dog_device_service = Arc::new(DogDeviceService::new(dog_device_repo));

        let dog_statistics_service = Arc::new(DogStatisticsService::new(
            dog_map_repo,
            dog_model_repo,
            dog_media_repo,
            dog_config_repo,
        ));

        AppState {
            user_service,
            role_service,
            auth_service,
            menu_service,
            org_service,
            sys_auth_service,
            sys_dict_service,
            sys_dict_item_service,
            sys_log_service,
            dog_map_service,
            dog_media_service,
            dog_model_service,
            dog_config_service,
            dog_device_service,
            dog_statistics_service,
            upload_dir,
        }
    }

    pub async fn run(self, config: &AppConfig, addr: SocketAddr) -> Result<(), AppError> {
        let state = self.build_state(config);
        let router = create_router(state);

        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))
            .expect("Failed to create socket");
        if let Err(e) = socket.set_reuse_address(true) {
            tracing::warn!("Failed to set SO_REUSEADDR: {}", e);
        }
        if let Err(e) = socket.set_reuse_port(true) {
            tracing::warn!("Failed to set SO_REUSEPORT: {}", e);
        }
        socket
            .bind(&socket2::SockAddr::from(addr))
            .expect("Failed to bind to address");
        socket.listen(1024).expect("Failed to listen on socket");
        socket
            .set_nonblocking(true)
            .expect("Failed to set non-blocking mode");
        let std_listener: std::net::TcpListener = socket.into();
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .expect("Failed to create tokio listener");

        tracing::info!("Server running on http://{}", addr);
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("Server failed");

        Ok(())
    }
}

pub fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:data/users.db".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DogConfig, RedisConfig, ServerConfig};

    fn config_with(upload_dir: Option<&str>) -> AppConfig {
        AppConfig {
            server: ServerConfig {
                host: "127.0.0.1".to_string(),
                port: 0,
            },
            redis: RedisConfig {
                host: "localhost".to_string(),
                port: 6379,
                password: String::new(),
            },
            jwt_secret: "test-secret".to_string(),
            database_url: "sqlite::memory:".to_string(),
            dog: DogConfig {
                upload_dir: upload_dir.map(str::to_string),
            },
        }
    }

    #[tokio::test]
    async fn explicit_upload_dir_survives_build_state() {
        let conn = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        let app = App::from_connection_with_upload_dir(conn, "/tmp/rust_admin_explicit_uploads");
        let state = app.build_state_with_token_store(
            Arc::new(crate::system::auth::repository::RedisTokenStore::new(
                "redis://127.0.0.1:6379",
            )),
            "test-secret",
        );

        assert_eq!(
            state.upload_dir,
            std::path::Path::new("/tmp/rust_admin_explicit_uploads")
        );
    }

    #[tokio::test]
    async fn configured_upload_dir_overrides_the_default() {
        let conn = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        let app = App::from_connection(conn);
        let state = app.build_state(&config_with(Some("/tmp/rust_admin_configured")));

        assert_eq!(
            state.upload_dir,
            std::path::Path::new("/tmp/rust_admin_configured")
        );
    }

    #[tokio::test]
    async fn empty_configured_upload_dir_falls_back_to_app_default() {
        let conn = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
        let app = App::from_connection(conn);
        let state = app.build_state(&config_with(None));

        assert_eq!(state.upload_dir, std::path::Path::new(DEFAULT_UPLOAD_DIR));
    }
}
