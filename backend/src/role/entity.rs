use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_sys_role")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id", column_type = "Text")]
    pub id: String,
    #[sea_orm(column_name = "name", column_type = "Text")]
    pub name: String,
    #[sea_orm(column_name = "code", column_type = "Text", nullable)]
    pub code: Option<String>,
    #[sea_orm(column_name = "description", column_type = "Text", nullable)]
    pub description: Option<String>,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp")]
    pub create_time: DateTime<Utc>,
    #[sea_orm(column_name = "update_time", column_type = "Timestamp")]
    pub update_time: DateTime<Utc>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
    #[sea_orm(column_name = "is_edit", column_type = "Boolean", nullable)]
    pub is_edit: Option<bool>,
    #[sea_orm(column_name = "ds_type", column_type = "Integer", nullable)]
    pub ds_type: Option<i32>,
    #[sea_orm(column_name = "ds_scope", column_type = "Text", nullable)]
    pub ds_scope: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
