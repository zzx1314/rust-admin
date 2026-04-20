use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_sys_org")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id", column_type = "Text")]
    pub id: String,
    #[sea_orm(column_name = "name", column_type = "Text")]
    pub name: String,
    #[sea_orm(column_name = "sort", column_type = "Integer", nullable)]
    pub sort: Option<i32>,
    #[sea_orm(column_name = "parent_id", column_type = "Text", nullable)]
    pub parent_id: Option<String>,
    #[sea_orm(column_name = "parent_name", column_type = "Text", nullable)]
    pub parent_name: Option<String>,
    #[sea_orm(column_name = "org_duty", column_type = "Text", nullable)]
    pub org_duty: Option<String>,
    #[sea_orm(column_name = "desrc", column_type = "Text", nullable)]
    pub desrc: Option<String>,
    #[sea_orm(column_name = "type", column_type = "Text", nullable)]
    pub r#type: Option<String>,
    #[sea_orm(column_name = "is_out", column_type = "Boolean", nullable)]
    pub is_out: Option<bool>,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp")]
    pub create_time: DateTime<Utc>,
    #[sea_orm(column_name = "update_time", column_type = "Timestamp", nullable)]
    pub update_time: DateTime<Utc>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
