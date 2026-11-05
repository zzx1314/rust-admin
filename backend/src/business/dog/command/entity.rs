use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_command")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "command_name", column_type = "Text")]
    pub command_name: String,
    #[sea_orm(column_name = "command_type", column_type = "Text", nullable)]
    pub command_type: Option<String>,
    #[sea_orm(column_name = "command_params", column_type = "Text", nullable)]
    pub command_params: Option<String>,
    #[sea_orm(column_name = "device_id", column_type = "Text", nullable)]
    pub device_id: Option<String>,
    #[sea_orm(column_name = "status", column_type = "Text")]
    pub status: String,
    #[sea_orm(column_name = "return_value", column_type = "Text", nullable)]
    pub return_value: Option<String>,
    #[sea_orm(column_name = "task_id", column_type = "Integer", nullable)]
    pub task_id: Option<i64>,
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
