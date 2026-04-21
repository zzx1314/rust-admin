use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Deserialize, Clone)]
pub struct SetMenuAuthRequest {
    pub role_code: String,
    pub auth_list: Vec<i64>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SysAuthMenuVo {
    pub id: i64,
    pub title: String,
    pub auth_list: Vec<SysAuthTitleVo>,
    #[serde(rename = "useAuthList")]
    pub use_auth_list: HashSet<i64>,
    #[serde(rename = "isCheckAll")]
    pub is_check_all: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct SysAuthTitleVo {
    pub id: i64,
    pub name: String,
    pub permission: Option<String>,
}

impl From<crate::system::sys_menu::domain::Menu> for SysAuthTitleVo {
    fn from(menu: crate::system::sys_menu::domain::Menu) -> Self {
        Self {
            id: menu.id,
            name: menu.name,
            permission: menu.permission,
        }
    }
}
