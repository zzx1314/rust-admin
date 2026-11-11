use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 中台设备台账。
///
/// `device_id` 与业务平台 `devices.device_id` 对齐，业务平台通过 `/api/dogDevice/upsert`
/// 把设备同步进来；园区只保存编码/名称冗余，中台不持有园区表。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_device")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "device_id", column_type = "Text")]
    pub device_id: String,
    #[sea_orm(column_name = "device_name", column_type = "Text", nullable)]
    pub device_name: Option<String>,
    #[sea_orm(column_name = "model", column_type = "Text", nullable)]
    pub model: Option<String>,
    #[sea_orm(column_name = "status", column_type = "Text")]
    pub status: String,
    #[sea_orm(column_name = "firmware", column_type = "Text", nullable)]
    pub firmware: Option<String>,
    #[sea_orm(column_name = "device_ip", column_type = "Text", nullable)]
    pub device_ip: Option<String>,
    #[sea_orm(column_name = "park_code", column_type = "Text", nullable)]
    pub park_code: Option<String>,
    #[sea_orm(column_name = "park_name", column_type = "Text", nullable)]
    pub park_name: Option<String>,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
    /// 创建者（中台账号 id）
    #[sea_orm(column_name = "user_id", column_type = "Integer", nullable)]
    pub user_id: Option<i64>,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp")]
    pub create_time: DateTime<Utc>,
    #[sea_orm(column_name = "update_time", column_type = "Timestamp", nullable)]
    pub update_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
