use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "p_sys_role_menu")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "role_id", column_type = "Text")]
    pub role_id: String,
    #[sea_orm(primary_key, column_name = "menu_id", column_type = "Text")]
    pub menu_id: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "crate::role::entity::Entity",
        from = "Column::RoleId",
        to = "crate::role::entity::Column::Id"
    )]
    Role,
    #[sea_orm(
        belongs_to = "crate::menu::entity::Entity",
        from = "Column::MenuId",
        to = "crate::menu::entity::Column::Id"
    )]
    Menu,
}

impl ActiveModelBehavior for ActiveModel {}
