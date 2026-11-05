use crate::business::dog::user_device::entity::ActiveModel as UserDeviceActiveModel;
use crate::business::dog::user_device::entity::Model as UserDeviceModel;
use crate::common::util::format_datetime;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogUserDevice = UserDeviceModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserDeviceRequest {
    /// Required for administrators binding on behalf of another user; ignored
    /// (forced to the caller) for regular users.
    pub user_id: Option<i64>,
    /// Primary key of `dog_device` (not the hardware device id).
    pub device_id: i64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UserDevicePageQuery {
    pub user_id: Option<i64>,
    pub device_id: Option<i64>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UserDeviceVO {
    pub id: i64,
    pub user_id: i64,
    pub device_id: i64,
    pub device_code: Option<String>,
    pub create_time: String,
}

impl UserDeviceVO {
    pub fn from_model(binding: DogUserDevice, device_code: Option<String>) -> Self {
        Self {
            id: binding.id,
            user_id: binding.user_id,
            device_id: binding.device_id,
            device_code,
            create_time: format_datetime(binding.create_time),
        }
    }
}

impl CreateUserDeviceRequest {
    pub fn to_active_model(
        &self,
        id: i64,
        user_id: i64,
        now: DateTime<Utc>,
    ) -> UserDeviceActiveModel {
        UserDeviceActiveModel {
            id: ActiveValue::set(id),
            user_id: ActiveValue::set(user_id),
            device_id: ActiveValue::set(self.device_id),
            create_time: ActiveValue::set(now),
            update_time: ActiveValue::set(Some(now)),
            is_deleted: ActiveValue::set(0),
        }
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}
