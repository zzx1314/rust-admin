use crate::business::dog::config::service::DogConfigService;
use crate::business::dog::device::service::DogDeviceService;
use crate::business::dog::map::service::DogMapService;
use crate::business::dog::media::service::DogMediaService;
use crate::business::dog::model::service::DogModelService;
use crate::business::dog::statistics::service::DogStatisticsService;
use crate::system::auth::service::AuthService;
use crate::system::sys_auth::service::SysAuthService;
use crate::system::sys_dict::service::SysDictService;
use crate::system::sys_dict_item::service::SysDictItemService;
use crate::system::sys_log::service::SysLogService;
use crate::system::sys_menu::service::MenuService;
use crate::system::sys_org::service::OrgService;
use crate::system::sys_role::service::RoleService;
use crate::system::sys_user::service::UserService;
use std::path::PathBuf;
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
    pub sys_log_service: Arc<SysLogService>,
    pub dog_map_service: Arc<DogMapService>,
    pub dog_media_service: Arc<DogMediaService>,
    pub dog_model_service: Arc<DogModelService>,
    pub dog_config_service: Arc<DogConfigService>,
    pub dog_device_service: Arc<DogDeviceService>,
    pub dog_statistics_service: Arc<DogStatisticsService>,
    /// Directory for uploaded map packages (`<upload_dir>/maps`).
    pub upload_dir: PathBuf,
    /// 业务平台地址，用于中台设备页向设备下发「同步地图」指令。
    pub business_api_url: Option<String>,
    /// 调用业务平台内部接口的服务令牌。
    pub business_api_token: Option<String>,
}
