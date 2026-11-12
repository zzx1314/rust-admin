use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_map")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "map_name", column_type = "Text")]
    pub map_name: String,
    #[sea_orm(column_name = "map_type", column_type = "Text", nullable)]
    pub map_type: Option<String>,
    /// 归属园区编码（业务库 `parks.park_code`，中台不建园区表）
    #[sea_orm(column_name = "park_code", column_type = "Text", nullable)]
    pub park_code: Option<String>,
    /// 归属园区名称，冗余存储便于中台列表展示
    #[sea_orm(column_name = "park_name", column_type = "Text", nullable)]
    pub park_name: Option<String>,
    /// 上报该地图的设备编号（业务库 `devices.device_id`，中台不做外键约束）
    #[sea_orm(column_name = "device_code", column_type = "Text", nullable)]
    pub device_code: Option<String>,
    #[sea_orm(column_name = "map_data", column_type = "Text", nullable)]
    pub map_data: Option<String>,
    #[sea_orm(column_name = "user_id", column_type = "Integer")]
    pub user_id: i64,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp")]
    pub create_time: DateTime<Utc>,
    #[sea_orm(column_name = "update_time", column_type = "Timestamp", nullable)]
    pub update_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
    #[sea_orm(column_name = "file_name", column_type = "Text", nullable)]
    pub file_name: Option<String>,
    #[sea_orm(column_name = "file_path", column_type = "Text", nullable)]
    pub file_path: Option<String>,
    #[sea_orm(column_name = "file_format", column_type = "Text", nullable)]
    pub file_format: Option<String>,
    #[sea_orm(column_name = "file_size", column_type = "BigInteger", nullable)]
    pub file_size: Option<i64>,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
    #[sea_orm(column_name = "map_version", column_type = "Text", nullable)]
    pub map_version: Option<String>,
    #[sea_orm(column_name = "frame_id", column_type = "Text", nullable)]
    pub frame_id: Option<String>,
    #[sea_orm(column_name = "crs", column_type = "Text", nullable)]
    pub crs: Option<String>,
    #[sea_orm(column_name = "map_origin", column_type = "Text", nullable)]
    pub map_origin: Option<String>,
    #[sea_orm(column_name = "resolution", column_type = "Text", nullable)]
    pub resolution: Option<String>,
    #[sea_orm(column_name = "floors", column_type = "Text", nullable)]
    pub floors: Option<String>,
    #[sea_orm(column_name = "road_count", column_type = "Integer", nullable)]
    pub road_count: Option<i32>,
    #[sea_orm(column_name = "nogo_count", column_type = "Integer", nullable)]
    pub nogo_count: Option<i32>,
    #[sea_orm(column_name = "map_status", column_type = "Text")]
    pub map_status: String,
    #[sea_orm(column_name = "map_hash", column_type = "Text", nullable)]
    pub map_hash: Option<String>,
    #[sea_orm(column_name = "publish_time", column_type = "Text", nullable)]
    pub publish_time: Option<String>,
    #[sea_orm(column_name = "map_width", column_type = "Integer", nullable)]
    pub map_width: Option<i32>,
    #[sea_orm(column_name = "map_height", column_type = "Integer", nullable)]
    pub map_height: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
