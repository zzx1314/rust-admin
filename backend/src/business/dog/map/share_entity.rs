use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// 地图分享关系：`dog_map` 共享给哪些设备使用。
///
/// `device_code` 与 `dog_map.device_code` 同口径（业务平台 `devices.device_id`），
/// 中台不建外键；`map_id` 指向中台 `dog_map.id`。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_map_share")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "map_id", column_type = "Integer")]
    pub map_id: i64,
    #[sea_orm(column_name = "device_code", column_type = "Text")]
    pub device_code: String,
    /// 操作人（中台账号 id），仅留痕
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
