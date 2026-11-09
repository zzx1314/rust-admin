use crate::business::dog::park::entity::ActiveModel as ParkActiveModel;
use crate::business::dog::park::entity::Model as ParkModel;
use crate::common::util::format_datetime;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogPark = ParkModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateParkRequest {
    pub park_name: String,
    pub park_description: Option<String>,
    pub park_address: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateParkRequest {
    pub park_name: Option<String>,
    pub park_description: Option<String>,
    pub park_address: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParkPageQuery {
    pub park_name: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ParkVO {
    pub id: i64,
    pub park_name: String,
    pub park_description: Option<String>,
    pub park_address: Option<String>,
    pub user_id: i64,
    /// 当前生效的地图基线版本（`dog_map.id`）。
    pub map_id: Option<i64>,
    /// 标定状态：未绑定 / 待标定 / 已标定 / 部分标定。
    pub map_status: String,
    /// 所属用户的用户名，列表接口按 `user_id` 关联 `p_sys_user` 填充。
    pub user_name: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogPark> for ParkVO {
    fn from(p: DogPark) -> Self {
        Self {
            id: p.id,
            park_name: p.park_name,
            park_description: p.park_description,
            park_address: p.park_address,
            user_id: p.user_id,
            map_id: p.map_id,
            map_status: p.map_status,
            user_name: None,
            create_time: format_datetime(p.create_time),
            update_time: p.update_time.map(format_datetime),
        }
    }
}

impl CreateParkRequest {
    pub fn to_active_model(&self, id: i64, user_id: i64, now: DateTime<Utc>) -> ParkActiveModel {
        ParkActiveModel {
            id: ActiveValue::set(id),
            park_name: ActiveValue::set(self.park_name.clone()),
            park_description: ActiveValue::set(self.park_description.clone()),
            park_address: ActiveValue::set(self.park_address.clone()),
            user_id: ActiveValue::set(user_id),
            map_id: ActiveValue::set(None),
            map_status: ActiveValue::set("未绑定".to_string()),
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
