use std::net::SocketAddr;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::api::{AppState, routes::create_router};
use crate::auth::repository::RedisTokenStore;
use crate::auth::service::AuthService;
use crate::common::error::AppError;
use crate::common::traits::{
    MenuRepository, OrgRepository, RoleRepository, SysDictItemRepository, SysLogRepository,
    SysDictRepository, TokenStore, UserRepository,
};
use crate::config::AppConfig;
use crate::system::sys_menu::repository::SeaOrmMenuRepository;
use crate::system::sys_menu::service::MenuService;
use crate::migration::Migrator;
use crate::system::sys_org::repository::SeaOrmOrgRepository;
use crate::system::sys_org::service::OrgService;
use crate::system::sys_role::repository::SeaOrmRoleRepository;
use crate::system::sys_role::service::RoleService;
use crate::system::sys_auth::service::SysAuthService;
use crate::system::sys_dict::repository::SeaOrmSysDictRepository;
use crate::system::sys_dict::service::SysDictService;
use crate::system::sys_dict_item::repository::SeaOrmSysDictItemRepository;
use crate::system::sys_dict_item::service::SysDictItemService;
use crate::system::sys_log::repository::SeaOrmSysLogRepository;
use crate::system::sys_log::service::SysLogService;
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
        let user_service = Arc::new(UserService::new(user_repo.clone(), role_repo.clone(), org_repo.clone()));

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
        let sys_dict_item_service = Arc::new(SysDictItemService::new(sys_dict_item_repo, sys_dict_repo.clone()));

        let sys_log_repo: Arc<dyn SysLogRepository> =
            Arc::new(SeaOrmSysLogRepository::new(conn.clone()));
        let sys_log_service = Arc::new(SysLogService::new(sys_log_repo));

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
        }
    }

    pub async fn run(self, config: &AppConfig, addr: SocketAddr) -> Result<(), AppError> {
        let state = self.build_state(config);
        let router = create_router(state);

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("Failed to bind to address");

        tracing::info!("Server running on http://{}", addr);
        axum::serve(listener, router).await.expect("Server failed");

        Ok(())
    }
}

pub fn database_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:data/users.db".to_string())
}
