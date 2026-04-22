use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_sys_logrecord")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "tenant", column_type = "Text", nullable)]
    pub tenant: Option<String>,
    #[sea_orm(column_name = "type", column_type = "Text", nullable)]
    pub type_: Option<String>,
    #[sea_orm(column_name = "sub_type", column_type = "Text", nullable)]
    pub sub_type: Option<String>,
    #[sea_orm(column_name = "biz_no", column_type = "Text", nullable)]
    pub biz_no: Option<String>,
    #[sea_orm(column_name = "operator", column_type = "Text", nullable)]
    pub operator: Option<String>,
    #[sea_orm(column_name = "action", column_type = "Text", nullable)]
    pub action: Option<String>,
    #[sea_orm(column_name = "fail", column_type = "Boolean")]
    pub fail: bool,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp", nullable)]
    pub create_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "extra", column_type = "Text", nullable)]
    pub extra: Option<String>,
    #[sea_orm(column_name = "code_variable", column_type = "Text", nullable)]
    pub code_variable: Option<String>,
    #[sea_orm(column_name = "ip", column_type = "Text", nullable)]
    pub ip: Option<String>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}