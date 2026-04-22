use crate::common::util::format_datetime_opt;
use crate::system::sys_log::entity::ActiveModel as SysLogActiveModel;
use crate::system::sys_log::entity::Model as SysLogModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type SysLog = SysLogModel;

#[derive(Debug, Deserialize, Clone)]
pub struct CreateSysLogRequest {
    pub tenant: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub sub_type: Option<String>,
    pub biz_no: Option<String>,
    pub operator: Option<String>,
    pub action: Option<String>,
    pub fail: Option<bool>,
    pub extra: Option<String>,
    pub code_variable: Option<String>,
    pub ip: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpdateSysLogRequest {
    pub tenant: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub sub_type: Option<String>,
    pub biz_no: Option<String>,
    pub operator: Option<String>,
    pub action: Option<String>,
    pub fail: Option<bool>,
    pub extra: Option<String>,
    pub code_variable: Option<String>,
    pub ip: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct SysLogPageQuery {
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
    pub tenant: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub sub_type: Option<String>,
    pub biz_no: Option<String>,
    pub operator: Option<String>,
    pub action: Option<String>,
    pub ip: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}

impl SysLogPageQuery {
    pub fn page(&self) -> i64 {
        self.current.max(1)
    }

    pub fn size(&self) -> i64 {
        self.size.max(1)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SysLogVO {
    pub id: i64,
    pub tenant: Option<String>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub sub_type: Option<String>,
    pub biz_no: Option<String>,
    pub operator: Option<String>,
    pub action: Option<String>,
    pub fail: bool,
    pub create_time: Option<String>,
    pub extra: Option<String>,
    pub code_variable: Option<String>,
    pub ip: Option<String>,
    pub is_deleted: i32,
}

impl From<SysLogModel> for SysLogVO {
    fn from(m: SysLogModel) -> Self {
        Self {
            id: m.id,
            tenant: m.tenant,
            type_: m.type_,
            sub_type: m.sub_type,
            biz_no: m.biz_no,
            operator: m.operator,
            action: m.action,
            fail: m.fail,
            create_time: format_datetime_opt(m.create_time),
            extra: m.extra,
            code_variable: m.code_variable,
            ip: m.ip,
            is_deleted: m.is_deleted,
        }
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}

impl CreateSysLogRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> SysLogActiveModel {
        SysLogActiveModel {
            id: ActiveValue::set(id),
            tenant: set_opt_string(self.tenant.clone()),
            type_: set_opt_string(self.type_.clone()),
            sub_type: set_opt_string(self.sub_type.clone()),
            biz_no: set_opt_string(self.biz_no.clone()),
            operator: set_opt_string(self.operator.clone()),
            action: set_opt_string(self.action.clone()),
            fail: ActiveValue::set(self.fail.unwrap_or(false)),
            create_time: ActiveValue::set(Some(now)),
            extra: set_opt_string(self.extra.clone()),
            code_variable: set_opt_string(self.code_variable.clone()),
            ip: set_opt_string(self.ip.clone()),
            is_deleted: ActiveValue::set(0),
        }
    }
}

impl UpdateSysLogRequest {
    pub fn to_active_model(&self, id: i64) -> SysLogActiveModel {
        SysLogActiveModel {
            id: ActiveValue::unchanged(id),
            tenant: set_opt_string(self.tenant.clone()),
            type_: set_opt_string(self.type_.clone()),
            sub_type: set_opt_string(self.sub_type.clone()),
            biz_no: set_opt_string(self.biz_no.clone()),
            operator: set_opt_string(self.operator.clone()),
            action: set_opt_string(self.action.clone()),
            fail: self.fail.map(ActiveValue::Set).unwrap_or(ActiveValue::NotSet),
            extra: set_opt_string(self.extra.clone()),
            code_variable: set_opt_string(self.code_variable.clone()),
            ip: set_opt_string(self.ip.clone()),
            is_deleted: ActiveValue::unchanged(0),
            create_time: ActiveValue::not_set(),
        }
    }
}

fn set_opt_string(opt: Option<String>) -> ActiveValue<Option<String>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}