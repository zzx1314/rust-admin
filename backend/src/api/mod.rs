use crate::business::dog::command::service::DogCommandService;
use crate::business::dog::config::service::DogConfigService;
use crate::business::dog::device::service::DogDeviceService;
use crate::business::dog::gateway::DogGateway;
use crate::business::dog::media::service::DogMediaService;
use crate::business::dog::model::service::DogModelService;
use crate::business::dog::route_library::service::DogRouteLibraryService;
use crate::business::dog::services::service::DogServicesService;
use crate::business::dog::statistics::service::DogStatisticsService;
use crate::business::dog::task::service::DogTaskService;
use crate::business::dog::workflow::service::DogWorkflowService;
use crate::system::auth::service::AuthService;
use crate::system::sys_auth::service::SysAuthService;
use crate::system::sys_dict::service::SysDictService;
use crate::system::sys_dict_item::service::SysDictItemService;
use crate::system::sys_log::service::SysLogService;
use crate::system::sys_menu::service::MenuService;
use crate::system::sys_org::service::OrgService;
use crate::system::sys_role::service::RoleService;
use crate::system::sys_user::service::UserService;
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
    pub dog_device_service: Arc<DogDeviceService>,
    pub dog_task_service: Arc<DogTaskService>,
    pub dog_media_service: Arc<DogMediaService>,
    pub dog_model_service: Arc<DogModelService>,
    pub dog_config_service: Arc<DogConfigService>,
    pub dog_command_service: Arc<DogCommandService>,
    pub dog_route_library_service: Arc<DogRouteLibraryService>,
    pub dog_workflow_service: Arc<DogWorkflowService>,
    pub dog_services_service: Arc<DogServicesService>,
    pub dog_statistics_service: Arc<DogStatisticsService>,
    pub dog_gateway: Arc<DogGateway>,
}
