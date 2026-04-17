use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let stmts = [
            "ALTER TABLE roles ADD COLUMN code TEXT",
            "ALTER TABLE roles ADD COLUMN create_time TIMESTAMP DEFAULT '1970-01-01 00:00:00'",
            "ALTER TABLE roles ADD COLUMN update_time TIMESTAMP DEFAULT '1970-01-01 00:00:00'",
            "ALTER TABLE roles ADD COLUMN is_deleted INT DEFAULT 0",
            "ALTER TABLE roles ADD COLUMN remarks TEXT",
            "ALTER TABLE roles ADD COLUMN is_edit INT",
            "ALTER TABLE roles ADD COLUMN ds_type INT",
            "ALTER TABLE roles ADD COLUMN ds_scope TEXT",
        ];
        for stmt in stmts {
            manager.get_connection().execute_unprepared(stmt).await?;
        }
        Ok(())
    }
}
