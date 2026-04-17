use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "menus")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id", column_type = "Text")]
    pub id: String,
    #[sea_orm(column_name = "name", column_type = "Text")]
    pub name: String,
    #[sea_orm(column_name = "code", column_type = "Text", nullable)]
    pub code: Option<String>,
    #[sea_orm(column_name = "permission", column_type = "Text", nullable)]
    pub permission: Option<String>,
    #[sea_orm(column_name = "path_url", column_type = "Text", nullable)]
    pub path_url: Option<String>,
    #[sea_orm(column_name = "icon", column_type = "Text", nullable)]
    pub icon: Option<String>,
    #[sea_orm(column_name = "parent_id", column_type = "Text", nullable)]
    pub parent_id: Option<String>,
    #[sea_orm(column_name = "component", column_type = "Text", nullable)]
    pub component: Option<String>,
    #[sea_orm(column_name = "sort", column_type = "Integer", nullable)]
    pub sort: Option<i32>,
    #[sea_orm(column_name = "keep_alive", column_type = "Integer", nullable)]
    pub keep_alive: Option<i32>,
    #[sea_orm(column_name = "type", column_type = "Integer", nullable)]
    pub r#type: Option<i32>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
    #[sea_orm(column_name = "leaf", column_type = "Boolean", nullable)]
    pub leaf: Option<bool>,
    #[sea_orm(column_name = "role_code", column_type = "Text", nullable)]
    pub role_code: Option<String>,
    #[sea_orm(column_name = "disabled", column_type = "Boolean", nullable)]
    pub disabled: Option<bool>,
    #[sea_orm(column_name = "find_auth_id", column_type = "Integer", nullable)]
    pub find_auth_id: Option<i32>,
    #[sea_orm(column_name = "created_at", column_type = "Timestamp")]
    pub created_at: DateTime<Utc>,
    #[sea_orm(column_name = "updated_at", column_type = "Timestamp")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
