use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_sys_dict")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "type", column_type = "Text")]
    pub r#type: String,
    #[sea_orm(column_name = "dict_type", column_type = "Text", nullable)]
    pub dict_type: Option<String>,
    #[sea_orm(column_name = "description", column_type = "Text", nullable)]
    pub description: Option<String>,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp", nullable)]
    pub create_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "update_time", column_type = "Timestamp", nullable)]
    pub update_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
    #[sea_orm(column_name = "allow_deletion", column_type = "Integer", nullable)]
    pub allow_deletion: Option<i32>,
    #[sea_orm(column_name = "is_show", column_type = "Integer", nullable)]
    pub is_show: Option<i32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}