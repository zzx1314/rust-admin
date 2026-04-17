use sea_orm_migration::prelude::*;
use std::fs;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        let schema_sql = fs::read_to_string(
            "/home/zhangzexin/IdeaProjects/rust-admin/backend/migrations/p_sys/1_init.sql",
        )
        .expect("Failed to read schema SQL file");
        conn.execute_unprepared(&schema_sql).await?;

        let seed_sql = fs::read_to_string(
            "/home/zhangzexin/IdeaProjects/rust-admin/backend/migrations/p_sys/2_seed.sql",
        )
        .expect("Failed to read seed SQL file");
        conn.execute_unprepared(&seed_sql).await?;

        Ok(())
    }
}
