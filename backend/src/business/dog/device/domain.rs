use crate::business::dog::device::entity::ActiveModel as DeviceActiveModel;
use crate::business::dog::device::entity::Model as DeviceModel;
use crate::common::util::format_datetime;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogDevice = DeviceModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeviceRequest {
    /// 设备编号（业务平台 `devices.device_id`）
    pub device_id: String,
    pub device_name: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub firmware: Option<String>,
    pub device_ip: Option<String>,
    pub park_code: Option<String>,
    pub park_name: Option<String>,
    pub remarks: Option<String>,
    /// 同步时指定设备已分配的中台用户（仅管理员可用）
    #[serde(default)]
    pub user_ids: Option<Vec<i64>>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDeviceRequest {
    pub device_name: Option<String>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub firmware: Option<String>,
    pub device_ip: Option<String>,
    pub park_code: Option<String>,
    pub park_name: Option<String>,
    pub remarks: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DevicePageQuery {
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub status: Option<String>,
    pub park_code: Option<String>,
    /// 只看分配给某个中台用户的设备（管理员筛选用）
    pub user_id: Option<i64>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFilter {
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub status: Option<String>,
    pub park_code: Option<String>,
    pub user_id: Option<i64>,
}

/// 批量分配 / 解除分配。
///
/// `mode = replace` 时先清空这些设备已有的分配，再写入 `userIds`；
/// `mode = append`（默认）只追加，保留已有分配。
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssignUsersRequest {
    pub device_ids: Vec<i64>,
    pub user_ids: Vec<i64>,
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UnassignUsersRequest {
    pub device_ids: Vec<i64>,
    pub user_ids: Vec<i64>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceUserVO {
    pub id: i64,
    pub username: String,
    pub real_name: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceVO {
    pub id: i64,
    pub device_id: String,
    pub device_name: Option<String>,
    pub model: Option<String>,
    pub status: String,
    pub firmware: Option<String>,
    pub device_ip: Option<String>,
    pub park_code: Option<String>,
    pub park_name: Option<String>,
    pub remarks: Option<String>,
    pub user_id: Option<i64>,
    /// 已分配的中台用户
    pub users: Vec<DeviceUserVO>,
    pub user_ids: Vec<i64>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl DeviceVO {
    pub fn from_model(device: DogDevice, users: Vec<DeviceUserVO>) -> Self {
        let user_ids = users.iter().map(|user| user.id).collect();
        Self {
            id: device.id,
            device_id: device.device_id,
            device_name: device.device_name,
            model: device.model,
            status: device.status,
            firmware: device.firmware,
            device_ip: device.device_ip,
            park_code: device.park_code,
            park_name: device.park_name,
            remarks: device.remarks,
            user_id: device.user_id,
            users,
            user_ids,
            create_time: format_datetime(device.create_time),
            update_time: device.update_time.map(format_datetime),
        }
    }
}

impl CreateDeviceRequest {
    pub fn to_active_model(&self, id: i64, user_id: i64, now: DateTime<Utc>) -> DeviceActiveModel {
        DeviceActiveModel {
            id: ActiveValue::set(id),
            device_id: ActiveValue::set(self.device_id.trim().to_string()),
            device_name: ActiveValue::set(self.device_name.clone()),
            model: ActiveValue::set(self.model.clone()),
            status: ActiveValue::set(self.status.clone().unwrap_or_else(|| "离线".to_string())),
            firmware: ActiveValue::set(self.firmware.clone()),
            device_ip: ActiveValue::set(self.device_ip.clone()),
            park_code: ActiveValue::set(self.park_code.clone()),
            park_name: ActiveValue::set(self.park_name.clone()),
            remarks: ActiveValue::set(self.remarks.clone()),
            user_id: ActiveValue::set(Some(user_id)),
            create_time: ActiveValue::set(now),
            update_time: ActiveValue::set(Some(now)),
            is_deleted: ActiveValue::set(0),
        }
    }
}

/// 可分配用户下拉项
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssignableUserVO {
    pub id: i64,
    pub username: String,
    pub real_name: Option<String>,
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}
