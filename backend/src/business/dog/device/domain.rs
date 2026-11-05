use crate::business::dog::device::entity::ActiveModel as DeviceActiveModel;
use crate::business::dog::device::entity::Model as DeviceModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogDevice = DeviceModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeviceRequest {
    pub device_id: String,
    pub device_type: Option<String>,
    pub status: Option<String>,
    pub device_ip: Option<String>,
    pub model: Option<String>,
    pub speed: Option<String>,
    pub height: Option<String>,
    pub battery: Option<String>,
    pub course: Option<String>,
    pub location: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDeviceRequest {
    pub device_type: Option<String>,
    pub status: Option<String>,
    pub device_ip: Option<String>,
    pub model: Option<String>,
    pub speed: Option<String>,
    pub height: Option<String>,
    pub battery: Option<String>,
    pub course: Option<String>,
    pub location: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DevicePageQuery {
    pub device_id: Option<String>,
    pub device_type: Option<String>,
    pub status: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceVO {
    pub id: i64,
    pub device_id: String,
    pub device_type: Option<String>,
    pub status: String,
    pub device_ip: Option<String>,
    pub model: Option<String>,
    pub speed: Option<String>,
    pub height: Option<String>,
    pub battery: Option<String>,
    pub course: Option<String>,
    pub location: Option<String>,
    pub comm_time: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogDevice> for DeviceVO {
    fn from(d: DogDevice) -> Self {
        Self {
            id: d.id,
            device_id: d.device_id,
            device_type: d.device_type,
            status: d.status,
            device_ip: d.device_ip,
            model: d.model,
            speed: d.speed,
            height: d.height,
            battery: d.battery,
            course: d.course,
            location: d.location,
            comm_time: d.comm_time.map(format_datetime),
            create_time: format_datetime(d.create_time),
            update_time: d.update_time.map(format_datetime),
        }
    }
}

impl CreateDeviceRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> DeviceActiveModel {
        DeviceActiveModel {
            id: ActiveValue::set(id),
            device_id: ActiveValue::set(self.device_id.clone()),
            device_type: ActiveValue::set(self.device_type.clone()),
            status: ActiveValue::set(self.status.clone().unwrap_or_else(|| "offline".to_string())),
            device_ip: ActiveValue::set(self.device_ip.clone()),
            model: ActiveValue::set(self.model.clone()),
            speed: ActiveValue::set(self.speed.clone()),
            height: ActiveValue::set(self.height.clone()),
            battery: ActiveValue::set(self.battery.clone()),
            course: ActiveValue::set(self.course.clone()),
            location: ActiveValue::set(self.location.clone()),
            comm_time: ActiveValue::set(None),
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

fn format_datetime(dt: DateTime<Utc>) -> String {
    let beijing = chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset");
    dt.with_timezone(&beijing)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}
