use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_app_review")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "src_project", column_type = "Text")]
    pub src_project: String,
    #[sea_orm(column_name = "dest_project", column_type = "Text")]
    pub dest_project: String,
    #[sea_orm(column_name = "repository_name", column_type = "Text")]
    pub repository_name: String,
    #[sea_orm(column_name = "tag", column_type = "Text")]
    pub tag: String,
    #[sea_orm(column_name = "digest", column_type = "Text", nullable)]
    pub digest: Option<String>,
    #[sea_orm(column_name = "artifact_id", column_type = "Integer", nullable)]
    pub artifact_id: Option<i64>,
    #[sea_orm(column_name = "status", column_type = "Text")]
    pub status: String,
    #[sea_orm(column_name = "reviewer_comment", column_type = "Text", nullable)]
    pub reviewer_comment: Option<String>,
    #[sea_orm(column_name = "created_by", column_type = "Integer", nullable)]
    pub created_by: Option<i64>,
    #[sea_orm(column_name = "reviewer_id", column_type = "Integer", nullable)]
    pub reviewer_id: Option<i64>,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp")]
    pub create_time: DateTime<Utc>,
    #[sea_orm(column_name = "update_time", column_type = "Timestamp", nullable)]
    pub update_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "startup_config", column_type = "Text", nullable)]
    pub startup_config: Option<String>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
