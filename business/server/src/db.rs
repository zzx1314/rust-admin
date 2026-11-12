use crate::config::Config;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, SqlitePool};
use std::fs;
use std::path::{Path, PathBuf};
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

/// 迁移目录名与显式覆盖环境变量。
const MIGRATIONS_DIR: &str = "migrations";
const MIGRATIONS_DIR_ENV: &str = "ROBODOG_MIGRATIONS_DIR";

/// 定位迁移目录：**运行期**解析，不能把编译机的绝对路径烧进二进制。
/// 顺序：`ROBODOG_MIGRATIONS_DIR` → 工作目录下的 `migrations/` →
/// 可执行文件旁的 `migrations/`。都没命中时返回工作目录下的相对路径，
/// 由 `read_dir` 报出原始错误。
fn migrations_directory() -> PathBuf {
    if let Ok(dir) = std::env::var(MIGRATIONS_DIR_ENV) {
        let dir = dir.trim();
        if !dir.is_empty() && Path::new(dir).is_dir() {
            return PathBuf::from(dir);
        }
    }

    let relative = PathBuf::from(MIGRATIONS_DIR);
    if relative.is_dir() {
        return relative;
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let beside_exe = dir.join(MIGRATIONS_DIR);
        if beside_exe.is_dir() {
            return beside_exe;
        }
    }
    relative
}

pub async fn migrate(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    pool.execute(
        "CREATE TABLE IF NOT EXISTS _migrations (
            name TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL
        )",
    )
    .await?;

    let directory = migrations_directory();
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

    // SQL 迁移只能改结构，坐标换算是纯数据变换（需要解析 JSON + 地图几何），
    // 放在 Rust 里做：历史路线「画布像素 → 地图米」一次性换算，幂等。
    match crate::coords::migrate_route_coordinates(pool).await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "路线坐标迁移完成（画布像素 → 地图米）"),
        Err(error) => tracing::warn!("路线坐标迁移失败: {error}"),
    }

    // 本体地图目录名（`source_ref`）回填：中台导入的重复包靠「同名 zip」认回原目录，
    // 否则下发时只能拿业务 ID 合成一个本体上不存在的目录名。
    match crate::mapdir::backfill_source_refs(pool).await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "地图本体目录名回填完成（source_ref）"),
        Err(error) => tracing::warn!("地图本体目录名回填失败: {error}"),
    }

    Ok(())
}

/// 历史机器人地图回填设备编号。
///
/// `map_versions.device_code` 是迁移 015 才加的列，迁移前入库的行没有设备信息，
/// 页面上就只能显示「设备未登记」，也没法按设备去中台对图。中台 `dog_map.device_code`
/// 一直有，按 `map_versions.source_id = dog_map.id` 回填即可。
///
/// 中台库只读、允许缺失（没部署中台 / 旧结构没有这一列）：拿不到就跳过，
/// 不影响启动。返回本次回填的行数。
pub async fn backfill_map_device_codes(
    pool: &SqlitePool,
    backend_db: &Path,
) -> Result<u64, sqlx::Error> {
    if !backend_db.is_file() {
        return Ok(0);
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", backend_db.display()))?
        .read_only(true)
        .create_if_missing(false);
    let zentai = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    let has_column: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM pragma_table_info('dog_map') WHERE name = 'device_code'",
    )
    .fetch_optional(&zentai)
    .await?;
    if has_column.is_none() {
        zentai.close().await;
        return Ok(0);
    }

    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, device_code FROM dog_map
         WHERE device_code IS NOT NULL AND device_code <> ''",
    )
    .fetch_all(&zentai)
    .await?;
    zentai.close().await;

    let mut updated = 0;
    for (id, device_code) in rows {
        updated += sqlx::query(
            "UPDATE map_versions SET device_code = ?1
             WHERE source_id = ?2 AND (device_code IS NULL OR device_code = '')",
        )
        .bind(&device_code)
        .bind(id.to_string())
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(updated)
}

pub fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
