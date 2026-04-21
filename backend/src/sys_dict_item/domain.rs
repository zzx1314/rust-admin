use crate::sys_dict_item::entity::ActiveModel as SysDictItemActiveModel;
use crate::sys_dict_item::entity::Model as SysDictItemModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type SysDictItem = SysDictItemModel;

#[derive(Debug, Deserialize, Clone)]
pub struct CreateSysDictItemRequest {
    pub r#type: String,
    pub label: Option<String>,
    pub dict_id: Option<i64>,
    pub value: Option<String>,
    pub sort: Option<i32>,
    pub description: Option<String>,
    pub remarks: Option<String>,
    pub allow_deletion: Option<i32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpdateSysDictItemRequest {
    pub r#type: Option<String>,
    pub label: Option<String>,
    pub dict_id: Option<i64>,
    pub value: Option<String>,
    pub sort: Option<i32>,
    pub description: Option<String>,
    pub remarks: Option<String>,
    pub allow_deletion: Option<i32>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct SysDictItemPageQuery {
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
    pub dict_id: Option<i64>,
    pub r#type: Option<String>,
    pub label: Option<String>,
}

impl SysDictItemPageQuery {
    pub fn page(&self) -> i64 {
        self.current.max(1)
    }

    pub fn size(&self) -> i64 {
        self.size.max(1)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SysDictItemVO {
    pub id: i64,
    pub r#type: String,
    pub label: Option<String>,
    pub dict_id: Option<i64>,
    pub value: Option<String>,
    pub sort: i32,
    pub description: Option<String>,
    pub create_time: Option<DateTime<Utc>>,
    pub update_time: Option<DateTime<Utc>>,
    pub is_deleted: i32,
    pub remarks: Option<String>,
    pub allow_deletion: Option<i32>,
}

impl From<SysDictItemModel> for SysDictItemVO {
    fn from(m: SysDictItemModel) -> Self {
        Self {
            id: m.id,
            r#type: m.r#type,
            label: m.label,
            dict_id: m.dict_id,
            value: m.value,
            sort: m.sort,
            description: m.description,
            create_time: m.create_time,
            update_time: m.update_time,
            is_deleted: m.is_deleted,
            remarks: m.remarks,
            allow_deletion: m.allow_deletion,
        }
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}

impl CreateSysDictItemRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> SysDictItemActiveModel {
        SysDictItemActiveModel {
            id: ActiveValue::set(id),
            r#type: ActiveValue::set(self.r#type.clone()),
            label: set_opt_string(self.label.clone()),
            dict_id: set_opt_i64(self.dict_id),
            value: set_opt_string(self.value.clone()),
            sort: ActiveValue::set(self.sort.unwrap_or(0)),
            description: set_opt_string(self.description.clone()),
            create_time: ActiveValue::set(Some(now)),
            update_time: ActiveValue::set(Some(now)),
            is_deleted: ActiveValue::set(0),
            remarks: set_opt_string(self.remarks.clone()),
            allow_deletion: set_opt_i32(self.allow_deletion),
        }
    }
}

impl UpdateSysDictItemRequest {
    pub fn to_active_model(&self, id: i64) -> SysDictItemActiveModel {
        SysDictItemActiveModel {
            id: ActiveValue::unchanged(id),
            r#type: set_string(self.r#type.clone()),
            label: set_opt_string(self.label.clone()),
            dict_id: set_opt_i64(self.dict_id),
            value: set_opt_string(self.value.clone()),
            sort: match self.sort {
                Some(v) => ActiveValue::set(v),
                None => ActiveValue::not_set(),
            },
            description: set_opt_string(self.description.clone()),
            update_time: ActiveValue::set(Some(Utc::now())),
            is_deleted: ActiveValue::unchanged(0),
            remarks: set_opt_string(self.remarks.clone()),
            allow_deletion: set_opt_i32(self.allow_deletion),
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

fn set_opt_i64(opt: Option<i64>) -> ActiveValue<Option<i64>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}
