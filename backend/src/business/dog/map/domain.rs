use crate::business::dog::map::entity::ActiveModel as MapActiveModel;
use crate::business::dog::map::entity::Model as MapModel;
use crate::common::util::format_datetime;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogMap = MapModel;

/// 地图版本状态：草稿 → 部分标定 / 已发布 → 已归档。
/// 只有「已发布」「部分标定」的版本才能被园区引用、被路线冻结。
pub const MAP_STATUS_DRAFT: &str = "草稿";
pub const MAP_STATUS_PUBLISHED: &str = "已发布";
pub const MAP_STATUS_PARTIAL: &str = "部分标定";
pub const MAP_STATUS_ARCHIVED: &str = "已归档";

pub fn is_publishable_map_status(status: &str) -> bool {
    matches!(status, MAP_STATUS_PUBLISHED | MAP_STATUS_PARTIAL)
}

pub fn is_valid_map_status(status: &str) -> bool {
    matches!(
        status,
        MAP_STATUS_DRAFT | MAP_STATUS_PUBLISHED | MAP_STATUS_PARTIAL | MAP_STATUS_ARCHIVED
    )
}

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
    pub map_version: Option<String>,
    pub frame_id: Option<String>,
    pub crs: Option<String>,
    pub map_origin: Option<String>,
    pub resolution: Option<String>,
    pub floors: Option<String>,
    pub road_count: Option<i32>,
    pub nogo_count: Option<i32>,
    pub map_status: Option<String>,
    pub map_hash: Option<String>,
    pub map_width: Option<i32>,
    pub map_height: Option<i32>,
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
    pub map_version: Option<String>,
    pub frame_id: Option<String>,
    pub crs: Option<String>,
    pub map_origin: Option<String>,
    pub resolution: Option<String>,
    pub floors: Option<String>,
    pub road_count: Option<i32>,
    pub nogo_count: Option<i32>,
    pub map_status: Option<String>,
    pub map_hash: Option<String>,
    pub map_width: Option<i32>,
    pub map_height: Option<i32>,
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
    pub map_version: Option<String>,
    pub frame_id: Option<String>,
    pub crs: Option<String>,
    pub map_origin: Option<String>,
    pub resolution: Option<String>,
    pub floors: Option<String>,
    pub road_count: Option<i32>,
    pub nogo_count: Option<i32>,
    pub map_status: Option<String>,
    pub map_hash: Option<String>,
    pub map_width: Option<i32>,
    pub map_height: Option<i32>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapPageQuery {
    pub map_name: Option<String>,
    pub file_name: Option<String>,
    pub park_id: Option<i64>,
    pub map_status: Option<String>,
    /// 园区页 / 路线表单使用：只返回可被引用的版本（不含已归档）。
    pub publishable_only: Option<bool>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
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
    pub park_id: Option<i64>,
    /// 所属园区名称，列表接口按 `park_id` 关联 `dog_park` 填充。
    pub park_name: Option<String>,
    pub user_id: i64,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_format: Option<String>,
    pub file_size: Option<i64>,
    pub remarks: Option<String>,
    pub map_version: Option<String>,
    pub frame_id: Option<String>,
    pub crs: Option<String>,
    pub map_origin: Option<String>,
    pub resolution: Option<String>,
    pub floors: Option<String>,
    pub road_count: Option<i32>,
    pub nogo_count: Option<i32>,
    pub map_status: String,
    pub map_hash: Option<String>,
    pub publish_time: Option<String>,
    pub map_width: Option<i32>,
    pub map_height: Option<i32>,
    pub create_time: String,
    pub update_time: Option<String>,
}

/// 地图基线的引用情况：哪些路线冻结了该版本、这些路线被多少任务引用。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapRefsVO {
    pub map_id: i64,
    pub routes: Vec<MapRouteRefVO>,
    pub task_count: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapRouteRefVO {
    pub id: i64,
    pub route_name: String,
    pub route_status: Option<String>,
    pub route_version: Option<String>,
    pub waypoint_count: usize,
    /// 新版本发布后仍冻结在旧版本，需人工确认迁移。
    pub frozen: bool,
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

impl DogMap {
    /// 下发给设备的地图基线快照；没有 `map_hash` 时返回 `None`（禁止下发）。
    pub fn dispatch_payload(&self) -> Option<serde_json::Value> {
        let map_hash = self
            .map_hash
            .as_deref()
            .map(str::trim)
            .filter(|hash| !hash.is_empty())?;
        Some(serde_json::json!({
            "mapId": self.id,
            "mapVersion": self.map_version,
            "mapHash": map_hash,
            "frameId": self.frame_id,
            "resolution": self.resolution,
            "origin": self.map_origin,
            "width": self.map_width,
            "height": self.map_height,
        }))
    }
}

impl From<DogMap> for MapVO {
    fn from(m: DogMap) -> Self {
        Self {
            id: m.id,
            map_name: m.map_name,
            map_type: m.map_type,
            map_data: m.map_data,
            park_id: m.park_id,
            park_name: None,
            user_id: m.user_id,
            file_name: m.file_name,
            file_path: m.file_path,
            file_format: m.file_format,
            file_size: m.file_size,
            remarks: m.remarks,
            map_version: m.map_version,
            frame_id: m.frame_id,
            crs: m.crs,
            map_origin: m.map_origin,
            resolution: m.resolution,
            floors: m.floors,
            road_count: m.road_count,
            nogo_count: m.nogo_count,
            map_status: m.map_status,
            map_hash: m.map_hash,
            publish_time: m.publish_time,
            map_width: m.map_width,
            map_height: m.map_height,
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
            map_version: ActiveValue::set(
                self.map_version.clone().filter(|v| !v.trim().is_empty()),
            ),
            frame_id: ActiveValue::set(self.frame_id.clone()),
            crs: ActiveValue::set(self.crs.clone()),
            map_origin: ActiveValue::set(self.map_origin.clone()),
            resolution: ActiveValue::set(self.resolution.clone()),
            floors: ActiveValue::set(self.floors.clone()),
            road_count: ActiveValue::set(self.road_count),
            nogo_count: ActiveValue::set(self.nogo_count),
            map_status: ActiveValue::set(
                self.map_status
                    .clone()
                    .unwrap_or_else(|| MAP_STATUS_DRAFT.to_string()),
            ),
            map_hash: ActiveValue::set(self.map_hash.clone()),
            publish_time: ActiveValue::set(None),
            map_width: ActiveValue::set(self.map_width),
            map_height: ActiveValue::set(self.map_height),
        }
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}
