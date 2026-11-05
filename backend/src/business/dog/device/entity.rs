use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_device")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "device_id", column_type = "Text")]
    pub device_id: String,
    #[sea_orm(column_name = "device_type", column_type = "Text", nullable)]
    pub device_type: Option<String>,
    #[sea_orm(column_name = "status", column_type = "Text")]
    pub status: String,
    #[sea_orm(column_name = "device_ip", column_type = "Text", nullable)]
    pub device_ip: Option<String>,
    #[sea_orm(column_name = "model", column_type = "Text", nullable)]
    pub model: Option<String>,
    #[sea_orm(column_name = "speed", column_type = "Text", nullable)]
    pub speed: Option<String>,
    #[sea_orm(column_name = "height", column_type = "Text", nullable)]
    pub height: Option<String>,
    #[sea_orm(column_name = "battery", column_type = "Text", nullable)]
    pub battery: Option<String>,
    #[sea_orm(column_name = "course", column_type = "Text", nullable)]
    pub course: Option<String>,
    #[sea_orm(column_name = "location", column_type = "Text", nullable)]
    pub location: Option<String>,
    #[sea_orm(column_name = "comm_time", column_type = "Timestamp", nullable)]
    pub comm_time: Option<DateTime<Utc>>,
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
