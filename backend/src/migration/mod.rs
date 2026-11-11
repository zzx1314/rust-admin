use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, Value};
use sea_orm_migration::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

const MIGRATIONS_DIR: &str = "migrations";
const MIGRATION_NAME_DIRECTIVE: &str = "-- @migration-name:";
const IF_NOT_EXISTS_DIRECTIVE: &str = "-- @if-not-exists:";

/// Drops `seaql_migrations` records whose migration files were removed.
///
/// Historical migrations (for example the deleted Harbor / app review domains)
/// remain recorded in old databases. `Migrator::up` aborts when an applied
/// migration has no matching file, so the stale records are pruned before the
/// pending migrations run.
///
/// Returns the number of removed records.
pub async fn prune_stale_migrations<C>(conn: &C) -> Result<u64, DbErr>
where
    C: ConnectionTrait,
{
    let backend = conn.get_database_backend();
    if backend != DbBackend::Sqlite {
        return Ok(0);
    }

    let table = conn
        .query_one(Statement::from_string(
            backend,
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'seaql_migrations'",
        ))
        .await?;
    if table.is_none() {
        return Ok(0);
    }

    let known: HashSet<String> = <Migrator as MigratorTrait>::migrations()
        .iter()
        .map(|migration| migration.name().to_string())
        .collect();

    let applied = conn
        .query_all(Statement::from_string(
            backend,
            "SELECT version FROM seaql_migrations",
        ))
        .await?;

    let mut removed = 0u64;
    for row in applied {
        let version: String = row.try_get("", "version")?;
        if known.contains(&version) {
            continue;
        }
        tracing::warn!(
            migration = %version,
            "dropping stale migration record: its file no longer exists"
        );
        conn.execute(Statement::from_sql_and_values(
            backend,
            "DELETE FROM seaql_migrations WHERE version = ?",
            [Value::from(version)],
        ))
        .await?;
        removed += 1;
    }

    Ok(removed)
}

pub struct Migrator;

struct SqlFileMigration {
    name: String,
    files: Vec<SqlMigrationFile>,
}

struct SqlMigrationFile {
    path: PathBuf,
    version: u64,
    precondition: Option<Precondition>,
}

enum Precondition {
    ColumnAbsent { table: String, column: String },
}

impl MigrationName for SqlFileMigration {
    fn name(&self) -> &str {
        &self.name
    }
}

#[async_trait::async_trait]
impl MigrationTrait for SqlFileMigration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();

        for file in &self.files {
            if let Some(precondition) = &file.precondition
                && !precondition.should_apply(conn).await?
            {
                continue;
            }

            let sql = fs::read_to_string(&file.path).map_err(|error| {
                DbErr::Custom(format!(
                    "Failed to read migration file {}: {}",
                    file.path.display(),
                    error
                ))
            })?;
            conn.execute_unprepared(&sql).await?;
        }

        Ok(())
    }
}

impl Precondition {
    async fn should_apply<C: ConnectionTrait>(&self, conn: &C) -> Result<bool, DbErr> {
        match self {
            Self::ColumnAbsent { table, column } => {
                let table = escape_sql_literal(table);
                let column = escape_sql_literal(column);
                let statement = Statement::from_string(
                    DbBackend::Sqlite,
                    format!(
                        "SELECT 1 FROM pragma_table_info('{}') WHERE name = '{}'",
                        table, column
                    ),
                );
                Ok(conn.query_one(statement).await?.is_none())
            }
        }
    }
}

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        discover_migrations()
            .unwrap_or_else(|error| panic!("Failed to discover SQL migrations: {}", error))
            .into_iter()
            .map(|migration| Box::new(migration) as Box<dyn MigrationTrait>)
            .collect()
    }
}

fn discover_migrations() -> Result<Vec<SqlFileMigration>, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(MIGRATIONS_DIR);
    let mut paths = Vec::new();
    collect_sql_files(&root, &mut paths)
        .map_err(|error| format!("failed to scan {}: {}", root.display(), error))?;

    let mut files = paths
        .into_iter()
        .map(|path| parse_sql_file(&root, path))
        .collect::<Result<Vec<_>, _>>()?;
    files.sort_by(|left, right| {
        left.version
            .cmp(&right.version)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });

    let mut grouped: HashMap<String, Vec<DiscoveredSqlFile>> = HashMap::new();
    let mut group_order = Vec::new();
    for file in files {
        if !grouped.contains_key(&file.name) {
            group_order.push(file.name.clone());
        }
        grouped.entry(file.name.clone()).or_default().push(file);
    }

    let mut migrations = group_order
        .into_iter()
        .map(|name| {
            let files = grouped
                .remove(&name)
                .expect("migration group must exist while building migrations");
            SqlFileMigration {
                name,
                files: files
                    .into_iter()
                    .map(|file| SqlMigrationFile {
                        path: file.path,
                        version: file.version,
                        precondition: file.precondition,
                    })
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    // Order migrations by their numeric version (directory layout must not matter),
    // falling back to the file path only to break ties deterministically.
    migrations.sort_by_key(|migration| {
        migration
            .files
            .first()
            .map(|file| (file.version, file.path.clone()))
    });

    Ok(migrations)
}

struct DiscoveredSqlFile {
    path: PathBuf,
    relative_path: String,
    version: u64,
    name: String,
    precondition: Option<Precondition>,
}

fn collect_sql_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_sql_files(&path, files)?;
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("sql") {
            files.push(path);
        }
    }
    Ok(())
}

fn parse_sql_file(root: &Path, path: PathBuf) -> Result<DiscoveredSqlFile, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid migration file name: {}", path.display()))?;
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("invalid migration file name: {}", path.display()))?;
    let (version, description) = stem.split_once('_').ok_or_else(|| {
        format!(
            "migration file {} must use <number>_<description>.sql",
            path.display()
        )
    })?;
    let version = version.parse::<u64>().map_err(|_| {
        format!(
            "migration file {} must start with a numeric version",
            path.display()
        )
    })?;
    if description.is_empty() {
        return Err(format!(
            "migration file {} must have a non-empty description",
            path.display()
        ));
    }

    let contents = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {}", path.display(), error))?;
    let (explicit_name, precondition) = parse_directives(&contents, file_name)?;
    let relative_path = path
        .strip_prefix(root)
        .map_err(|_| {
            format!(
                "migration file {} is outside the migration root",
                path.display()
            )
        })?
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    let name = explicit_name.unwrap_or_else(|| default_migration_name(version, description));

    Ok(DiscoveredSqlFile {
        path,
        relative_path,
        version,
        name,
        precondition,
    })
}

fn parse_directives(
    contents: &str,
    file_name: &str,
) -> Result<(Option<String>, Option<Precondition>), String> {
    let mut migration_name = None;
    let mut precondition = None;

    for line in contents.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix(MIGRATION_NAME_DIRECTIVE) {
            let value = value.trim();
            if value.is_empty() {
                return Err(format!("{} has an empty migration name", file_name));
            }
            migration_name = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix(IF_NOT_EXISTS_DIRECTIVE) {
            precondition = Some(parse_precondition(value.trim(), file_name)?);
        }
    }

    Ok((migration_name, precondition))
}

fn parse_precondition(value: &str, file_name: &str) -> Result<Precondition, String> {
    let mut parts = value.split_whitespace();
    match (parts.next(), parts.next(), parts.next()) {
        (Some("column"), Some(identifier), None) => {
            let (table, column) = identifier.split_once('.').ok_or_else(|| {
                format!(
                    "{} uses an invalid column precondition: {}",
                    file_name, value
                )
            })?;
            if table.is_empty() || column.is_empty() {
                return Err(format!(
                    "{} uses an invalid column precondition: {}",
                    file_name, value
                ));
            }
            Ok(Precondition::ColumnAbsent {
                table: table.to_string(),
                column: column.to_string(),
            })
        }
        _ => Err(format!(
            "{} uses an unsupported precondition: {}",
            file_name, value
        )),
    }
}

fn default_migration_name(version: u64, description: &str) -> String {
    format!("m{:06}_{}", version, description)
}

fn escape_sql_literal(value: &str) -> String {
    value.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_existing_files_in_version_order_and_preserves_names() {
        let migrations = <Migrator as MigratorTrait>::migrations();
        let names: Vec<&str> = migrations
            .iter()
            .map(|migration| migration.name())
            .collect();

        assert_eq!(
            names,
            vec![
                "p_sys_tables_from_file",
                "add_sys_logrecord",
                "add_user_is_edit",
                "create_dog_tables",
                "remove_harbor_menus",
                "add_dog_menus",
                "add_park_map_tables",
                "add_task_user_scope",
                "add_park_map_menus",
                "create_user_device_bindings",
                "add_device_binding_menu",
                "flatten_dog_park_menus",
                "remove_device_binding_menu",
                "remove_workflow_menu",
                "reorder_map_menu",
                "add_map_file_fields",
                "reorder_menus",
                "add_route_park",
                "remove_retired_dog_menus",
                "add_map_baseline",
                "remove_command_services_menus",
                "add_map_dimensions",
                "drop_retired_dog_tables",
                "drop_command_services_tables",
                "add_map_park",
                "add_device_management",
                "add_device_management_menu",
            ]
        );
        assert_eq!(migrations[0].name(), "p_sys_tables_from_file");
    }

    #[tokio::test]
    async fn applies_discovered_migrations_to_a_fresh_sqlite_database() {
        let connection = sea_orm::Database::connect("sqlite::memory:").await.unwrap();

        <Migrator as MigratorTrait>::up(&connection, None)
            .await
            .unwrap();

        for table in ["p_sys_user", "p_sys_logrecord", "dog_map"] {
            let statement = Statement::from_string(
                DbBackend::Sqlite,
                format!(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '{}'",
                    table
                ),
            );
            assert!(connection.query_one(statement).await.unwrap().is_some());
        }

        for table in [
            "dog_park",
            "dog_task",
            "dog_route_library",
            "dog_workflow",
            "dog_command",
            "dog_services",
        ] {
            let statement = Statement::from_string(
                DbBackend::Sqlite,
                format!(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '{}'",
                    table
                ),
            );
            assert!(
                connection.query_one(statement).await.unwrap().is_none(),
                "{} must be dropped by the cleanup migration",
                table
            );
        }

        // 设备台账与用户分配由 add_device_management 重建。
        for table in ["dog_device", "dog_user_device"] {
            let statement = Statement::from_string(
                DbBackend::Sqlite,
                format!(
                    "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '{}'",
                    table
                ),
            );
            assert!(
                connection.query_one(statement).await.unwrap().is_some(),
                "{} must exist after the device management migration",
                table
            );
        }

        let is_edit = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM pragma_table_info('p_sys_user') WHERE name = 'is_edit'",
        );
        assert!(connection.query_one(is_edit).await.unwrap().is_some());

        for column in ["map_status", "map_width", "map_height", "map_hash"] {
            let statement = Statement::from_string(
                DbBackend::Sqlite,
                format!(
                    "SELECT 1 FROM pragma_table_info('dog_map') WHERE name = '{}'",
                    column
                ),
            );
            assert!(
                connection.query_one(statement).await.unwrap().is_some(),
                "dog_map.{} must exist",
                column
            );
        }

        // 园区归属已移除：dog_map 不再有 park_id。
        let map_park_id = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM pragma_table_info('dog_map') WHERE name = 'park_id'",
        );
        assert!(
            connection.query_one(map_park_id).await.unwrap().is_none(),
            "dog_map.park_id must be dropped"
        );

        // 保留的页面仍是一级菜单：设备 / 地图 / 模型 / 媒体 / 参数配置。
        for (id, sort) in [(1210, 2), (1302, 3), (1204, 4), (1205, 5), (1207, 6)] {
            let statement = Statement::from_string(
                DbBackend::Sqlite,
                format!(
                    "SELECT 1 FROM p_sys_menu WHERE id = {} AND is_deleted = 0 AND parent_id = -1 AND sort = {}",
                    id, sort
                ),
            );
            assert!(
                connection.query_one(statement).await.unwrap().is_some(),
                "menu {} must stay enabled at sort {}",
                id,
                sort
            );
        }

        // 园区 / 路线 / 任务 / 工作流 / 旧设备绑定菜单已物理删除。
        for id in [
            1200, 1201, 1202, 1203, 1209, 1211, 1212, 1213, 1219, 1300, 1301, 1311,
        ] {
            let statement = Statement::from_string(
                DbBackend::Sqlite,
                format!("SELECT 1 FROM p_sys_menu WHERE id = {}", id),
            );
            assert!(
                connection.query_one(statement).await.unwrap().is_none(),
                "menu {} must be hard-deleted",
                id
            );
        }

        let retired_grants = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM p_sys_role_menu WHERE menu_id IN (1201, 1206, 1208, 1301)",
        );
        assert!(
            connection
                .query_one(retired_grants)
                .await
                .unwrap()
                .is_none(),
            "retired menu grants must be removed"
        );

        let kept_grant = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM p_sys_role_menu WHERE role_id = 1 AND menu_id = 1302",
        );
        assert!(connection.query_one(kept_grant).await.unwrap().is_some());

        let harbor_menu = Statement::from_string(
            DbBackend::Sqlite,
            "SELECT 1 FROM p_sys_menu WHERE id = 1104 AND is_deleted = 1",
        );
        assert!(connection.query_one(harbor_menu).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn prunes_applied_migrations_that_no_longer_have_files() {
        let connection = sea_orm::Database::connect("sqlite::memory:").await.unwrap();

        <Migrator as MigratorTrait>::up(&connection, None)
            .await
            .unwrap();

        connection
            .execute_unprepared(
                "INSERT INTO seaql_migrations (version, applied_at) \
                 VALUES ('add_app_review', 1), ('add_startup_config', 2)",
            )
            .await
            .unwrap();

        assert!(
            <Migrator as MigratorTrait>::up(&connection, None)
                .await
                .is_err(),
            "up must refuse to run while stale records exist"
        );

        let removed = prune_stale_migrations(&connection).await.unwrap();
        assert_eq!(removed, 2);

        <Migrator as MigratorTrait>::up(&connection, None)
            .await
            .expect("migrations must run after pruning stale records");
    }

    #[test]
    fn parses_file_directives() {
        let (name, precondition) = parse_directives(
            "-- @migration-name: add_user_is_edit\n-- @if-not-exists: column p_sys_user.is_edit\nALTER TABLE p_sys_user ADD COLUMN is_edit INTEGER;",
            "4_add_user_is_edit.sql",
        )
        .unwrap();

        assert_eq!(name.as_deref(), Some("add_user_is_edit"));
        assert!(matches!(
            precondition,
            Some(Precondition::ColumnAbsent { ref table, ref column })
                if table == "p_sys_user" && column == "is_edit"
        ));
    }

    #[test]
    fn defaults_migration_name_from_version_and_description() {
        assert_eq!(
            default_migration_name(6, "add_audit_log"),
            "m000006_add_audit_log"
        );
    }
}
