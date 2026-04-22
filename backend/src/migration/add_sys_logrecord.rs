use sea_orm_migration::prelude::*;
use std::fs;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        let manifest_dir = env!("CARGO_MANIFEST_DIR");

        let sql = fs::read_to_string(format!(
            "{}/migrations/p_sys/3_sys_logrecord.sql",
            manifest_dir
        ))
        .expect("Failed to read sys_logrecord SQL file");
        conn.execute_unprepared(&sql).await?;

        Ok(())
    }
}
