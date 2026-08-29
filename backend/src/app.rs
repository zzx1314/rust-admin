use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::api::{AppState, routes::create_router};
use crate::business::app_review::repository::SeaOrmAppReviewRepository;
use crate::business::app_review::service::AppReviewService;
use crate::business::harbor::client::HarborClient;
use crate::business::harbor::service::HarborService;
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
}

impl App {
    pub async fn new(database_url: &str) -> Result<Self, AppError> {
        let conn = sea_orm::Database::connect(database_url)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Migrator::up(&conn, None)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(Self { conn })
    }

    pub fn build_state(&self, config: &AppConfig) -> AppState {
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

        let redis_url = config.redis.url();
        let token_store: Arc<dyn TokenStore> = Arc::new(RedisTokenStore::new(&redis_url));
        let auth_service = Arc::new(AuthService::new(
            user_repo,
            token_store,
            role_repo.clone(),
            &config.jwt_secret,
        ));

        let menu_repo: Arc<dyn MenuRepository> = Arc::new(SeaOrmMenuRepository::new(conn.clone()));
        let menu_service = Arc::new(MenuService::new(menu_repo.clone(), role_repo.clone()));

        let sys_auth_service = Arc::new(SysAuthService::new(menu_repo, role_repo.clone()));

        let sys_dict_repo: Arc<dyn SysDictRepository> =
            Arc::new(SeaOrmSysDictRepository::new(conn.clone()));
        let sys_dict_service = Arc::new(SysDictService::new(sys_dict_repo.clone()));

        let sys_dict_item_repo: Arc<dyn SysDictItemRepository> =
            Arc::new(SeaOrmSysDictItemRepository::new(conn.clone()));
        let sys_dict_item_service = Arc::new(SysDictItemService::new(
            sys_dict_item_repo,
            sys_dict_repo.clone(),
        ));

        let sys_log_repo: Arc<dyn SysLogRepository> =
            Arc::new(SeaOrmSysLogRepository::new(conn.clone()));
        let sys_log_service = Arc::new(SysLogService::new(sys_log_repo));

        let app_review_repo = SeaOrmAppReviewRepository::new(conn.clone());

        let harbor_service = if let Some(harbor_config) = config.harbor.clone() {
            let registry_endpoint_id = harbor_config.registry_endpoint_id;
            let registry_insecure = harbor_config.registry_insecure;
            let replication_timeout_secs = harbor_config.replication_timeout_secs;
            let replication_poll_interval_secs = harbor_config.replication_poll_interval_secs;
            let harbor_client = Arc::new(HarborClient::new(&harbor_config));
            Arc::new(
                HarborService::new(harbor_client)
                    .with_registry_endpoint_id(registry_endpoint_id)
                    .with_registry_insecure(registry_insecure)
                    .with_replication_timeout_secs(replication_timeout_secs)
                    .with_replication_poll_interval_secs(replication_poll_interval_secs),
            )
        } else {
            tracing::warn!(
                "Harbor config is missing, Harbor endpoints will return an error until [harbor] is configured in config.toml"
            );
            Arc::new(HarborService::new(Arc::new(HarborClient::new(
                &crate::config::HarborConfig {
                    url: String::new(),
                    username: String::new(),
                    password: String::new(),
                    staging_project: String::new(),
                    production_project: String::new(),
                    registry_endpoint_id: None,
                    registry_insecure: None,
                    webhook_secret: None,
                    replication_timeout_secs: 30,
                    replication_poll_interval_secs: 1,
                },
            ))))
        };

        let app_review_service = Arc::new(AppReviewService::new(
            app_review_repo,
            harbor_service.clone(),
        ));
        let harbor_config = config.harbor.clone();

        if let Some(harbor_cfg) = &harbor_config
            && harbor_cfg
                .webhook_secret
                .as_ref()
                .map(|s| s.is_empty())
                .unwrap_or(true)
        {
            tracing::warn!(
                "Harbor webhook secret is not configured. The /api/webhooks/harbor endpoint is publicly accessible without verification."
            );
        }

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
            harbor_service,
            app_review_service,
            harbor_config,
        }
    }

    pub async fn run(self, config: &AppConfig, addr: SocketAddr) -> Result<(), AppError> {
        let state = self.build_state(config);
        let router = create_router(state);

        // Enable SO_REUSEADDR + SO_REUSEPORT so the port is immediately
        // reusable when restarting with tools like `cargo watch -x run`.
        // SO_REUSEADDR alone does not let you bind to a port still held by
        // a living process; SO_REUSEPORT allows exactly that.
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
        // Tokio requires a non-blocking socket; socket2::Socket defaults to blocking.
        socket
            .set_nonblocking(true)
            .expect("Failed to set non-blocking mode");
        let std_listener: std::net::TcpListener = socket.into();
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .expect("Failed to create tokio listener");

        tracing::info!("Server running on http://{}", addr);
        axum::serve(listener, router).await.expect("Server failed");

        Ok(())
    }
}

pub fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:data/users.db".to_string())
}
