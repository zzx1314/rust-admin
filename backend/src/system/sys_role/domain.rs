use crate::common::util::format_datetime;
use crate::system::sys_role::entity::ActiveModel as RoleActiveModel;
use crate::system::sys_role::entity::Model as RoleModel;
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type Role = RoleModel;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RoleVO {
    pub id: i64,
    pub name: String,
    pub code: Option<String>,
    pub description: Option<String>,
    pub create_time: String,
    pub update_time: String,
    pub is_deleted: i32,
    pub remarks: Option<String>,
    pub is_edit: Option<bool>,
    pub ds_type: Option<i32>,
    pub ds_scope: Option<String>,
}

impl From<RoleModel> for RoleVO {
    fn from(m: RoleModel) -> Self {
        Self {
            id: m.id,
            name: m.name,
            code: m.code,
            description: m.description,
            create_time: format_datetime(m.create_time),
            update_time: format_datetime(m.update_time),
            is_deleted: m.is_deleted,
            remarks: m.remarks,
            is_edit: m.is_edit,
            ds_type: m.ds_type,
            ds_scope: m.ds_scope,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct CreateRoleRequest {
    pub name: String,
    pub code: Option<String>,
    pub description: Option<String>,
    pub remarks: Option<String>,
    pub ds_type: Option<i32>,
    pub ds_scope: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpdateRoleRequest {
    pub name: Option<String>,
    pub code: Option<String>,
    pub description: Option<String>,
    pub remarks: Option<String>,
    pub is_edit: Option<bool>,
    pub ds_type: Option<i32>,
    pub ds_scope: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RolePageQuery {
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
    pub name: Option<String>,
    pub code: Option<String>,
}

impl RolePageQuery {
    pub fn page(&self) -> i64 {
        self.current.max(1)
    }

    pub fn size(&self) -> i64 {
        self.size.max(1)
    }

    pub fn offset(&self) -> i64 {
        (self.page() - 1) * self.size()
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}

impl CreateRoleRequest {
    pub fn to_active_model(&self, id: i64) -> RoleActiveModel {
        RoleActiveModel {
            id: ActiveValue::set(id),
            name: ActiveValue::set(self.name.clone()),
            code: ActiveValue::set(self.code.clone()),
            description: ActiveValue::set(self.description.clone()),
            remarks: ActiveValue::set(self.remarks.clone()),
            is_edit: ActiveValue::set(Some(false)),
            ds_type: ActiveValue::set(self.ds_type),
            ds_scope: ActiveValue::set(self.ds_scope.clone()),
            create_time: ActiveValue::set(chrono::Utc::now()),
            update_time: ActiveValue::set(chrono::Utc::now()),
            is_deleted: ActiveValue::set(0),
            ..Default::default()
        }
    }
}

impl UpdateRoleRequest {
    pub fn to_active_model(&self, id: i64) -> RoleActiveModel {
        RoleActiveModel {
            id: ActiveValue::unchanged(id),
            name: set_string(self.name.clone()),
            code: set_opt_string(self.code.clone()),
            description: set_opt_string(self.description.clone()),
            remarks: set_opt_string(self.remarks.clone()),
            is_edit: set_opt_bool(self.is_edit),
            ds_type: set_opt_i32(self.ds_type),
            ds_scope: set_opt_string(self.ds_scope.clone()),
            update_time: ActiveValue::set(chrono::Utc::now()),
            is_deleted: ActiveValue::unchanged(0),
            ..Default::default()
        }
    }
}

fn set_string(opt: Option<String>) -> ActiveValue<String> {
    match opt {
        Some(v) => ActiveValue::set(v),
        None => ActiveValue::not_set(),
    }
}

fn set_opt_string(opt: Option<String>) -> ActiveValue<Option<String>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}

fn set_opt_bool(opt: Option<bool>) -> ActiveValue<Option<bool>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}

fn set_opt_i32(opt: Option<i32>) -> ActiveValue<Option<i32>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}
