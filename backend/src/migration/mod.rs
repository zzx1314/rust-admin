pub use sea_orm_migration::prelude::*;

pub mod m20260331064738_create_users;
pub mod m20260331070000_create_roles;
pub mod m20260331080000_add_password_hash;
pub mod m20260402000000_create_menus;
pub mod m20260402075644_create_sys_role_menu;
pub mod m20260402100000_restructure_users;
pub mod m20260402110000_add_role_fields;
pub mod m20260402120000_create_orgs;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260331064738_create_users::Migration),
            Box::new(m20260331070000_create_roles::Migration),
            Box::new(m20260331080000_add_password_hash::Migration),
            Box::new(m20260402000000_create_menus::Migration),
            Box::new(m20260402075644_create_sys_role_menu::Migration),
            Box::new(m20260402100000_restructure_users::Migration),
            Box::new(m20260402110000_add_role_fields::Migration),
            Box::new(m20260402120000_create_orgs::Migration),
        ]
    }
}
