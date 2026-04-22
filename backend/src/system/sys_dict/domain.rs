use crate::common::util::format_datetime_opt;
use crate::system::sys_dict::entity::ActiveModel as SysDictActiveModel;
use crate::system::sys_dict::entity::Model as SysDictModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type SysDict = SysDictModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateSysDictRequest {
    pub r#type: String,
    pub dict_type: Option<String>,
    pub description: Option<String>,
    pub remarks: Option<String>,
    pub allow_deletion: Option<i32>,
    pub is_show: Option<i32>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSysDictRequest {
    pub r#type: Option<String>,
    pub dict_type: Option<String>,
    pub description: Option<String>,
    pub remarks: Option<String>,
    pub allow_deletion: Option<i32>,
    pub is_show: Option<i32>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SysDictPageQuery {
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
    pub r#type: Option<String>,
    pub description: Option<String>,
}

impl SysDictPageQuery {
    pub fn page(&self) -> i64 {
        self.current.max(1)
    }

    pub fn size(&self) -> i64 {
        self.size.max(1)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SysDictVO {
    pub id: i64,
    pub r#type: String,
    pub dict_type: Option<String>,
    pub description: Option<String>,
    pub remarks: Option<String>,
    pub create_time: Option<String>,
    pub update_time: Option<String>,
    pub is_deleted: i32,
    pub allow_deletion: Option<i32>,
    pub is_show: Option<i32>,
}

impl From<SysDictModel> for SysDictVO {
    fn from(m: SysDictModel) -> Self {
        Self {
            id: m.id,
            r#type: m.r#type,
            dict_type: m.dict_type,
            description: m.description,
            remarks: m.remarks,
            create_time: format_datetime_opt(m.create_time),
            update_time: format_datetime_opt(m.update_time),
            is_deleted: m.is_deleted,
            allow_deletion: m.allow_deletion,
            is_show: m.is_show,
        }
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}

impl CreateSysDictRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> SysDictActiveModel {
        SysDictActiveModel {
            id: ActiveValue::set(id),
            r#type: ActiveValue::set(self.r#type.clone()),
            dict_type: set_opt_string(self.dict_type.clone()),
            description: set_opt_string(self.description.clone()),
            remarks: set_opt_string(self.remarks.clone()),
            create_time: ActiveValue::set(Some(now)),
            update_time: ActiveValue::set(Some(now)),
            is_deleted: ActiveValue::set(0),
            allow_deletion: set_opt_i32(self.allow_deletion),
            is_show: set_opt_i32(self.is_show),
        }
    }
}

impl UpdateSysDictRequest {
    pub fn to_active_model(&self, id: i64) -> SysDictActiveModel {
        SysDictActiveModel {
            id: ActiveValue::unchanged(id),
            r#type: set_string(self.r#type.clone()),
            dict_type: set_opt_string(self.dict_type.clone()),
            description: set_opt_string(self.description.clone()),
            remarks: set_opt_string(self.remarks.clone()),
            update_time: ActiveValue::set(Some(Utc::now())),
            is_deleted: ActiveValue::unchanged(0),
            allow_deletion: set_opt_i32(self.allow_deletion),
            is_show: set_opt_i32(self.is_show),
            create_time: ActiveValue::not_set(),
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

fn set_opt_i32(opt: Option<i32>) -> ActiveValue<Option<i32>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}
