pub use sea_orm_migration::prelude::*;

pub mod add_sys_logrecord;
pub mod p_sys_tables_from_file;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(p_sys_tables_from_file::Migration),
            Box::new(add_sys_logrecord::Migration),
        ]
    }
}
