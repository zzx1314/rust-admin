use crate::business::dog::map::entity::ActiveModel as MapActiveModel;
use crate::business::dog::map::entity::Model as MapModel;
use crate::common::util::format_datetime;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogMap = MapModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateMapRequest {
    pub map_name: String,
    pub map_type: Option<String>,
    pub map_data: Option<String>,
    pub park_id: i64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMapRequest {
    pub map_name: Option<String>,
    pub map_type: Option<String>,
    pub map_data: Option<String>,
    pub park_id: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapPageQuery {
    pub map_name: Option<String>,
    pub park_id: Option<i64>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapVO {
    pub id: i64,
    pub map_name: String,
    pub map_type: Option<String>,
    pub map_data: Option<String>,
    pub park_id: i64,
    pub user_id: i64,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogMap> for MapVO {
    fn from(m: DogMap) -> Self {
        Self {
            id: m.id,
            map_name: m.map_name,
            map_type: m.map_type,
            map_data: m.map_data,
            park_id: m.park_id,
            user_id: m.user_id,
            create_time: format_datetime(m.create_time),
            update_time: m.update_time.map(format_datetime),
        }
    }
}

impl CreateMapRequest {
    pub fn to_active_model(&self, id: i64, user_id: i64, now: DateTime<Utc>) -> MapActiveModel {
        MapActiveModel {
            id: ActiveValue::set(id),
            map_name: ActiveValue::set(self.map_name.clone()),
            map_type: ActiveValue::set(self.map_type.clone()),
            map_data: ActiveValue::set(self.map_data.clone()),
            park_id: ActiveValue::set(self.park_id),
            user_id: ActiveValue::set(user_id),
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
