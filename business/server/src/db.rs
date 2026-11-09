use crate::config::Config;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, SqlitePool};
use std::fs;
use std::path::Path;
use std::str::FromStr;

pub async fn connect(config: &Config) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = config.database_path.parent() {
        fs::create_dir_all(parent).ok();
    }

    let options = SqliteConnectOptions::from_str(&config.database_url())?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true);

    SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
}

pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    pool.execute(
        "CREATE TABLE IF NOT EXISTS _migrations (
            name TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        )",
    )
    .await?;

    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<_> = fs::read_dir(&directory)
        .map_err(sqlx::Error::Io)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sql"))
        .collect();
    files.sort();

    for path in files {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        let applied: Option<(String,)> =
            sqlx::query_as("SELECT name FROM _migrations WHERE name = ?1")
                .bind(&name)
                .fetch_optional(pool)
                .await?;
        if applied.is_some() {
            continue;
        }

        let sql = fs::read_to_string(&path).map_err(sqlx::Error::Io)?;
        sqlx::raw_sql(&sql).execute(pool).await?;
        sqlx::query("INSERT INTO _migrations (name, applied_at) VALUES (?1, ?2)")
            .bind(&name)
            .bind(now())
            .execute(pool)
            .await?;
        tracing::info!(migration = %name, "applied database migration");
    }

    Ok(())
}

pub fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
