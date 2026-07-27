use sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::prelude::*;
use std::fs;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        // 1_init.sql already creates the is_edit column on p_sys_user, so fresh
        // databases would fail with "duplicate column name". Skip the migration
        // when the column already exists.
        let has_column = conn
            .query_one(Statement::from_string(
                DbBackend::Sqlite,
                "SELECT 1 FROM pragma_table_info('p_sys_user') WHERE name = 'is_edit'",
            ))
            .await?
            .is_some();

        if has_column {
            return Ok(());
        }

        let manifest_dir = env!("CARGO_MANIFEST_DIR");

        let sql = fs::read_to_string(format!(
            "{}/migrations/p_sys/4_add_user_is_edit.sql",
            manifest_dir
        ))
        .expect("Failed to read add_user_is_edit SQL file");
        conn.execute_unprepared(&sql).await?;

        Ok(())
    }
}
