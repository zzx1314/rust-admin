use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_route_library")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "route_name", column_type = "Text")]
    pub route_name: String,
    #[sea_orm(column_name = "route_type", column_type = "Text", nullable)]
    pub route_type: Option<String>,
    #[sea_orm(column_name = "route_status", column_type = "Text", nullable)]
    pub route_status: Option<String>,
    #[sea_orm(column_name = "route_description", column_type = "Text", nullable)]
    pub route_description: Option<String>,
    #[sea_orm(column_name = "park_id", column_type = "Integer", nullable)]
    pub park_id: Option<i64>,
    #[sea_orm(column_name = "model_id", column_type = "Integer")]
    pub model_id: i64,
    #[sea_orm(column_name = "route_data", column_type = "Text", nullable)]
    pub route_data: Option<String>,
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
