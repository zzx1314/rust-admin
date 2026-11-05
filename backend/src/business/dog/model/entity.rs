use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_model")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "model_name", column_type = "Text")]
    pub model_name: String,
    #[sea_orm(column_name = "model_type", column_type = "Text", nullable)]
    pub model_type: Option<String>,
    #[sea_orm(column_name = "file_name", column_type = "Text", nullable)]
    pub file_name: Option<String>,
    #[sea_orm(column_name = "file_path", column_type = "Text", nullable)]
    pub file_path: Option<String>,
    #[sea_orm(column_name = "file_format", column_type = "Text", nullable)]
    pub file_format: Option<String>,
    #[sea_orm(column_name = "file_size", column_type = "Integer", nullable)]
    pub file_size: Option<i64>,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
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
