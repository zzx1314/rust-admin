use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::api::{AppState, routes::create_router};
use crate::business::dog::command::repository::SeaOrmDogCommandRepository;
use crate::business::dog::command::service::DogCommandService;
use crate::business::dog::config::repository::SeaOrmDogConfigRepository;
use crate::business::dog::config::service::DogConfigService;
use crate::business::dog::device::repository::SeaOrmDogDeviceRepository;
use crate::business::dog::device::service::DogDeviceService;
use crate::business::dog::gateway::DogGateway;
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::business::dog::map::service::DogMapService;
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::business::dog::media::service::DogMediaService;
use crate::business::dog::model::repository::SeaOrmDogModelRepository;
use crate::business::dog::model::service::DogModelService;
use crate::business::dog::park::repository::SeaOrmDogParkRepository;
use crate::business::dog::park::service::DogParkService;
use crate::business::dog::route_library::repository::SeaOrmDogRouteLibraryRepository;
use crate::business::dog::route_library::service::DogRouteLibraryService;
use crate::business::dog::services::repository::SeaOrmDogServicesRepository;
use crate::business::dog::services::service::DogServicesService;
use crate::business::dog::statistics::service::DogStatisticsService;
use crate::business::dog::task::repository::SeaOrmDogTaskRepository;
use crate::business::dog::task::service::DogTaskService;
use crate::business::dog::user_device::repository::SeaOrmDogUserDeviceRepository;
use crate::business::dog::user_device::service::DogUserDeviceService;
use crate::business::dog::workflow::repository::SeaOrmDogWorkflowRepository;
use crate::business::dog::workflow::service::DogWorkflowService;
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
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        crate::migration::prune_stale_migrations(&conn)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Migrator::up(&conn, None)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

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
        self.build_state_with(
            token_store,
            &config.jwt_secret,
            PathBuf::from(&config.dog.upload_dir),
        )
    }

    /// Build the application state with an explicit token store so that tests
    /// can use an in-memory implementation instead of Redis.
    pub fn build_state_with_token_store(
        &self,
        token_store: Arc<dyn TokenStore>,
        jwt_secret: &str,
    ) -> AppState {
        self.build_state_with(token_store, jwt_secret, self.upload_dir.clone())
    }

    fn build_state_with(
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

        let dog_device_repo = SeaOrmDogDeviceRepository::new(conn.clone());
        let dog_device_service = Arc::new(DogDeviceService::new(dog_device_repo.clone()));

        let dog_user_device_repo = SeaOrmDogUserDeviceRepository::new(conn.clone());
        let dog_user_device_service = Arc::new(DogUserDeviceService::new(
            dog_user_device_repo,
            dog_device_repo.clone(),
        ));

        let dog_task_repo = SeaOrmDogTaskRepository::new(conn.clone());
        let dog_workflow_repo = SeaOrmDogWorkflowRepository::new(conn.clone());
        let dog_command_repo = SeaOrmDogCommandRepository::new(conn.clone());
        let dog_route_repo = SeaOrmDogRouteLibraryRepository::new(conn.clone());
        let dog_gateway = Arc::new(DogGateway::new(conn.clone(), upload_dir));
        let dog_task_service = Arc::new(DogTaskService::new(
            dog_task_repo.clone(),
            dog_device_repo.clone(),
            dog_workflow_repo.clone(),
            dog_command_repo.clone(),
            dog_route_repo.clone(),
            dog_gateway.clone(),
        ));

        let dog_park_repo = SeaOrmDogParkRepository::new(conn.clone());
        let dog_park_service = Arc::new(DogParkService::new(dog_park_repo.clone()));

        let dog_map_repo = SeaOrmDogMapRepository::new(conn.clone());
        let dog_map_service = Arc::new(DogMapService::new(dog_map_repo, dog_park_repo));

        let dog_media_repo = SeaOrmDogMediaRepository::new(conn.clone());
        let dog_media_service = Arc::new(DogMediaService::new(dog_media_repo.clone()));

        let dog_model_repo = SeaOrmDogModelRepository::new(conn.clone());
        let dog_model_service = Arc::new(DogModelService::new(dog_model_repo.clone()));

        let dog_config_repo = SeaOrmDogConfigRepository::new(conn.clone());
        let dog_config_service = Arc::new(DogConfigService::new(dog_config_repo));

        let dog_command_service = Arc::new(DogCommandService::new(dog_command_repo.clone()));

        let dog_route_library_service =
            Arc::new(DogRouteLibraryService::new(dog_route_repo.clone()));

        let dog_workflow_service = Arc::new(DogWorkflowService::new(dog_workflow_repo));

        let dog_services_repo = SeaOrmDogServicesRepository::new(conn.clone());
        let dog_services_service = Arc::new(DogServicesService::new(dog_services_repo));

        let dog_statistics_service = Arc::new(DogStatisticsService::new(
            dog_device_repo,
            dog_task_repo,
            dog_model_repo,
            dog_route_repo,
            dog_media_repo,
            dog_command_repo,
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
            dog_device_service,
            dog_user_device_service,
            dog_task_service,
            dog_park_service,
            dog_map_service,
            dog_media_service,
            dog_model_service,
            dog_config_service,
            dog_command_service,
            dog_route_library_service,
            dog_workflow_service,
            dog_services_service,
            dog_statistics_service,
            dog_gateway,
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
