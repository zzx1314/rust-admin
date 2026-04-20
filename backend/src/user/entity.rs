use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_sys_user")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id", column_type = "Text")]
    pub id: String,
    #[sea_orm(column_name = "username", column_type = "Text")]
    pub username: String,
    #[sea_orm(column_name = "phone", column_type = "Text", nullable)]
    pub phone: Option<String>,
    #[sea_orm(column_name = "email", column_type = "Text", nullable)]
    pub email: Option<String>,
    #[sea_orm(column_name = "real_name", column_type = "Text", nullable)]
    pub real_name: Option<String>,
    #[sea_orm(column_name = "password", column_type = "Text", nullable)]
    pub password: Option<String>,
    #[sea_orm(column_name = "org_id", column_type = "Text", nullable)]
    pub org_id: Option<String>,
    #[sea_orm(column_name = "lock_time", column_type = "Timestamp", nullable)]
    pub lock_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "last_login_time", column_type = "Timestamp", nullable)]
    pub last_login_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "try_count", column_type = "Integer", nullable)]
    pub try_count: Option<i32>,
    #[sea_orm(column_name = "lock_flag", column_type = "Integer", nullable)]
    pub lock_flag: Option<i32>,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp")]
    pub create_time: DateTime<Utc>,
    #[sea_orm(column_name = "update_time", column_type = "Timestamp", nullable)]
    pub update_time: DateTime<Utc>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
    #[sea_orm(column_name = "pass_update_time", column_type = "Timestamp", nullable)]
    pub pass_update_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "card", column_type = "Text", nullable)]
    pub card: Option<String>,
    #[sea_orm(column_name = "is_show", column_type = "Integer", nullable)]
    pub is_show: Option<i32>,
    #[sea_orm(column_name = "enable", column_type = "Integer", nullable)]
    pub enable: Option<i32>,
    #[sea_orm(column_name = "first_login", column_type = "Integer", nullable)]
    pub first_login: Option<i32>,
    #[sea_orm(column_name = "sex", column_type = "Text", nullable)]
    pub sex: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
