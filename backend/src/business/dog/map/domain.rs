use crate::business::dog::map::entity::ActiveModel as MapActiveModel;
use crate::business::dog::map::entity::Model as MapModel;
use crate::common::util::format_datetime;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogMap = MapModel;

/// 地图版本状态：草稿 → 部分标定 / 已发布 → 已归档。
pub const MAP_STATUS_DRAFT: &str = "草稿";
pub const MAP_STATUS_PUBLISHED: &str = "已发布";
pub const MAP_STATUS_PARTIAL: &str = "部分标定";
pub const MAP_STATUS_ARCHIVED: &str = "已归档";

/// 未指定基线版本时的默认值（机器人回传 / 手工上传都不传版本）。
///
/// 以前这里的处理是「空就存 NULL」，结果地图列表的「基线版本」列一直是空的，
/// 而编辑弹窗又把 NULL 显示成 `v1.0`，看起来像数据丢了。版本是基线的一部分，
/// 落库时就该有值，派生新版本（v1.0 → v1.1）也才有起点。
pub const DEFAULT_MAP_VERSION: &str = "v1.0";

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
    /// 归属园区编码（业务库 `parks.park_code`）
    pub park_code: Option<String>,
    /// 归属园区名称（冗余展示）
    pub park_name: Option<String>,
    /// 上报该地图的设备编号（业务平台 `devices.device_id`）
    pub device_code: Option<String>,
    pub map_data: Option<String>,
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
    /// 归属园区编码（业务库 `parks.park_code`）
    pub park_code: Option<String>,
    /// 归属园区名称（冗余展示）
    pub park_name: Option<String>,
    /// 上报该地图的设备编号（业务平台 `devices.device_id`）
    pub device_code: Option<String>,
    /// 地图归属用户。仅管理员可用：业务平台以服务账号回传地图时，
    /// 通过该字段把地图记到设备所属用户名下（普通用户会被忽略）。
    pub owner_user_id: Option<i64>,
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
    pub park_code: Option<String>,
    pub park_name: Option<String>,
    /// 上报该地图的设备编号（业务平台 `devices.device_id`）
    pub device_code: Option<String>,
    pub map_data: Option<String>,
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
    pub map_status: Option<String>,
    /// 按业务园区编码筛选
    pub park_code: Option<String>,
    /// 按上报该地图的设备编号模糊筛选
    pub device_code: Option<String>,
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
    pub park_code: Option<String>,
    pub park_name: Option<String>,
    /// 上报该地图的设备编号（手工上传的地图为空）
    pub device_code: Option<String>,
    pub map_data: Option<String>,
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

/// 地图包解析结果：用于前端展示栅格地图
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
            park_code: m.park_code,
            park_name: m.park_name,
            device_code: m.device_code,
            map_data: m.map_data,
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
            park_code: ActiveValue::set(normalize_text(self.park_code.clone())),
            park_name: ActiveValue::set(normalize_text(self.park_name.clone())),
            device_code: ActiveValue::set(normalize_text(self.device_code.clone())),
            map_data: ActiveValue::set(self.map_data.clone()),
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
                self.map_version
                    .clone()
                    .map(|version| version.trim().to_string())
                    .filter(|version| !version.is_empty())
                    .or_else(|| Some(DEFAULT_MAP_VERSION.to_string())),
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

fn normalize_text(value: Option<String>) -> Option<String> {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}
