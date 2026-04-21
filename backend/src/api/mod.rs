use crate::auth::service::AuthService;
use crate::menu::service::MenuService;
use crate::org::service::OrgService;
use crate::role::service::RoleService;
use crate::sys_auth::service::SysAuthService;
use crate::sys_dict::service::SysDictService;
use crate::sys_dict_item::service::SysDictItemService;
use crate::user::service::UserService;
use std::sync::Arc;

pub mod middleware;
pub mod routes;

#[derive(Clone)]
pub struct AppState {
    pub user_service: Arc<UserService>,
    pub role_service: Arc<RoleService>,
    pub auth_service: Arc<AuthService>,
    pub menu_service: Arc<MenuService>,
    pub org_service: Arc<OrgService>,
    pub sys_auth_service: Arc<SysAuthService>,
    pub sys_dict_service: Arc<SysDictService>,
    pub sys_dict_item_service: Arc<SysDictItemService>,
}
