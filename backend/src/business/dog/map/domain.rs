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
    pub park_id: Option<i64>,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_format: Option<String>,
    pub file_size: Option<i64>,
    pub remarks: Option<String>,
}

/// Parsed from the `mapInfo` part of the upload request
/// (`POST /api/dogMap/upload`).
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UploadMapRequest {
    /// When present the existing map record is updated with the new package.
    pub id: Option<i64>,
    pub map_name: String,
    pub map_type: Option<String>,
    pub park_id: Option<i64>,
    pub remarks: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMapRequest {
    pub map_name: Option<String>,
    pub map_type: Option<String>,
    pub map_data: Option<String>,
    pub park_id: Option<i64>,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_format: Option<String>,
    pub file_size: Option<i64>,
    pub remarks: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapPageQuery {
    pub map_name: Option<String>,
    pub file_name: Option<String>,
    pub park_id: Option<i64>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapVO {    pub id: i64,
    pub map_name: String,
    pub map_type: Option<String>,
    pub map_data: Option<String>,
    pub park_id: Option<i64>,
    pub user_id: i64,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_format: Option<String>,
    pub file_size: Option<i64>,
    pub remarks: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

/// 地图包解析结果：用于路线规划页展示栅格地图
///
/// 地图包与参考实现保持一致：`.zip` 内为 ROS map_server 格式的
/// `model.pgm` + `model.yaml`。这里把 `pgm` 转成 `png` 并返回 base64，
/// 同时带回 `resolution` / `origin`，前端据此做像素↔世界坐标换算。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapImageVO {
    /// `data:image/png;base64,...`
    pub image: String,
    pub resolution: f64,
    pub origin: Vec<f64>,
    pub width: u32,
    pub height: u32,
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
            file_name: m.file_name,
            file_path: m.file_path,
            file_format: m.file_format,
            file_size: m.file_size,
            remarks: m.remarks,
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
            file_name: ActiveValue::set(self.file_name.clone()),
            file_path: ActiveValue::set(self.file_path.clone()),
            file_format: ActiveValue::set(self.file_format.clone()),
            file_size: ActiveValue::set(self.file_size),
            remarks: ActiveValue::set(self.remarks.clone()),
        }
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}
