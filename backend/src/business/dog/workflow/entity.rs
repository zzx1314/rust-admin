use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_workflow")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "uuid", column_type = "Text")]
    pub uuid: String,
    #[sea_orm(column_name = "nodes", column_type = "Text", nullable)]
    pub nodes: Option<String>,
    #[sea_orm(column_name = "edges", column_type = "Text", nullable)]
    pub edges: Option<String>,
    #[sea_orm(column_name = "task_id", column_type = "Integer", nullable)]
    pub task_id: Option<i64>,
    #[sea_orm(column_name = "command_json_string", column_type = "Text", nullable)]
    pub command_json_string: Option<String>,
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
