use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_sys_user_role")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "user_id")]
    pub user_id: i64,
    #[sea_orm(primary_key, column_name = "role_id")]
    pub role_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "crate::user::entity::Entity",
        from = "Column::UserId",
        to = "crate::user::entity::Column::Id"
    )]
    User,
    #[sea_orm(
        belongs_to = "crate::role::entity::Entity",
        from = "Column::RoleId",
        to = "crate::role::entity::Column::Id"
    )]
    Role,
}

impl ActiveModelBehavior for ActiveModel {}
