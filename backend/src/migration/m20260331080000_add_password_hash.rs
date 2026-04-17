use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let sql = "ALTER TABLE users ADD COLUMN password_hash TEXT";
        manager.get_connection().execute_unprepared(sql).await?;
        Ok(())
    }
}
