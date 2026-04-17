use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let sql = r#"
CREATE TABLE IF NOT EXISTS menus (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    code TEXT,
    permission TEXT,
    path_url TEXT,
    icon TEXT,
    parent_id TEXT,
    component TEXT,
    sort INTEGER,
    keep_alive INTEGER,
    type INTEGER,
    is_deleted INTEGER NOT NULL DEFAULT 0,
    remarks TEXT,
    leaf INTEGER,
    role_code TEXT,
    disabled INTEGER,
    find_auth_id INTEGER,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
)"#;
        manager.get_connection().execute_unprepared(sql).await?;
        Ok(())
    }
}
