use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let sql = r#"
CREATE TABLE IF NOT EXISTS orgs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    sort INTEGER DEFAULT 0,
    parent_id TEXT,
    parent_name TEXT,
    org_duty TEXT,
    desrc TEXT,
    type TEXT,
    is_out INTEGER DEFAULT 0,
    create_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    update_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT
)"#;
        manager.get_connection().execute_unprepared(sql).await?;
        Ok(())
    }
}
