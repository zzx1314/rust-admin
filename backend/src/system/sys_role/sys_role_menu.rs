use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_sys_role_menu")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "role_id")]
    pub role_id: i64,
    #[sea_orm(primary_key, column_name = "menu_id")]
    pub menu_id: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "crate::system::sys_role::entity::Entity",
        from = "Column::RoleId",
        to = "crate::system::sys_role::entity::Column::Id"
    )]
    Role,
    #[sea_orm(
        belongs_to = "crate::system::sys_menu::entity::Entity",
        from = "Column::MenuId",
        to = "crate::system::sys_menu::entity::Column::Id"
    )]
    Menu,
}

impl ActiveModelBehavior for ActiveModel {}
