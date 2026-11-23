//! 业务编号（`TK-…` / `PK-…` / `WO-…` …）的自增分配。
//!
//! **为什么有这一层**：以前每个建单接口都自己写一遍
//!
//! ```sql
//! SELECT COALESCE(MAX(id), 0) + 1 FROM tasks
//! ```
//!
//! 读和写不在同一条语句里，而连接池有 8 条连接、WAL 下两个请求完全可能读到
//! 同一个值 —— 第二个 `INSERT` 就撞 `tasks.task_code ... UNIQUE`，用户看到 500。
//! 更糟的是错误原文会被回给前端（见 `error.rs` 的注释）。
//!
//! 这里把「读基线 + 加一」压成一条带写锁的语句（`UPDATE … RETURNING`），
//! 分配表 `code_sequences` 在首次使用时用现有 `MAX(id)` 初始化，
//! 因此升级前后的编号是连续的、不会重号；删除过最大 id 的行也不会再导致重号。
//!
//! 编号本身只是**展示用的连续序号**，不是主键：主键仍由 `INTEGER PRIMARY KEY
//! AUTOINCREMENT` 分配，两者不要求相等（历史数据里本来就是靠 `MAX(id)+1` 对齐的）。

use sqlx::{SqliteConnection, SqlitePool};

/// 从连接池分配下一个编号。
///
/// # Errors
///
/// 取号表写不进去（库只读 / 连接池超时）时把 `sqlx::Error` 交给调用方，
/// 由 `AppError` 统一翻译成对外错误。
pub async fn next_id(pool: &SqlitePool, name: &str, table: &str) -> Result<i64, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    next_id_on(&mut conn, name, table).await
}

/// 同上，但走调用方自己的连接 / 事务。
///
/// 事务里必须先取号再 `INSERT`，走同一条连接才不会自己把自己写锁住
/// （用池子取号会拿另一条连接，事务持有写锁时直接 `SQLITE_BUSY`）。
///
/// `table` 只用于首次分配时取基线，**只能传本文件调用点的常量**，
/// 不要接外部输入（这句话和 `seed.rs` 里的动态表名是同一套约定）。
///
/// # Errors
///
/// 同 [`next_id`]。
pub async fn next_id_on(
    conn: &mut SqliteConnection,
    name: &str,
    table: &str,
) -> Result<i64, sqlx::Error> {
    ensure_baseline(conn, name, table).await?;
    sqlx::query_scalar(
        "UPDATE code_sequences SET value = value + 1 WHERE name = ?1 RETURNING value",
    )
    .bind(name)
    .fetch_one(&mut *conn)
    .await
}

/// 首次使用时把分配器初始化成「当前最大 id」。
///
/// 幂等：并发的两个请求都会先算一次 `MAX(id)`，`ON CONFLICT DO NOTHING`
/// 保证只有一条基线写入，随后各自的 `UPDATE … RETURNING` 拿到不同的值。
///
/// `WHERE 1` 不是摆设：`INSERT … SELECT … ON CONFLICT` 没有它时 `SQLite`
/// 会把 `ON` 当成 join 的续写而报语法错误。
async fn ensure_baseline(
    conn: &mut SqliteConnection,
    name: &str,
    table: &str,
) -> Result<(), sqlx::Error> {
    let sql = format!(
        "INSERT INTO code_sequences (name, value)
         SELECT ?1, COALESCE(MAX(id), 0) FROM {table} WHERE 1
         ON CONFLICT(name) DO NOTHING"
    );
    sqlx::query(&sql).bind(name).execute(&mut *conn).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// 建一个只有编号分配器的最小库（迁移 031 的表）。
    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect("sqlite::memory:")
            .await
            .expect("连接内存库失败");
        sqlx::query(
            "CREATE TABLE code_sequences (name TEXT PRIMARY KEY, value INTEGER NOT NULL DEFAULT 0)",
        )
        .execute(&pool)
        .await
        .expect("建 code_sequences 失败");
        sqlx::query("CREATE TABLE tasks (id INTEGER PRIMARY KEY AUTOINCREMENT, task_code TEXT)")
            .execute(&pool)
            .await
            .expect("建 tasks 失败");
        pool
    }

    #[tokio::test]
    async fn numbers_keep_increasing_from_the_existing_max() {
        let pool = pool().await;
        for _ in 0..3 {
            sqlx::query("INSERT INTO tasks (task_code) VALUES ('old')")
                .execute(&pool)
                .await
                .unwrap();
        }
        // 基线 = 现有 MAX(id) = 3，下一个就是 4
        assert_eq!(next_id(&pool, "tasks", "tasks").await.unwrap(), 4);
        assert_eq!(next_id(&pool, "tasks", "tasks").await.unwrap(), 5);
    }

    /// 并发下不能给出重复编号（这就是替掉 `MAX(id)+1` 的原因）。
    #[tokio::test]
    async fn concurrent_callers_never_get_the_same_number() {
        let pool = pool().await;
        let mut handles = Vec::new();
        for _ in 0..32 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                next_id(&pool, "tasks", "tasks").await.unwrap()
            }));
        }
        let mut ids = Vec::new();
        for handle in handles {
            ids.push(handle.await.unwrap());
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 32, "并发取号出现了重复：{ids:?}");
        assert_eq!(ids.first(), Some(&1));
        assert_eq!(ids.last(), Some(&32));
    }

    /// 事务里取号走同一条连接：不会被自己的写锁卡住，提交后编号保留。
    #[tokio::test]
    async fn numbers_can_be_taken_inside_a_transaction() {
        let pool = pool().await;
        let mut tx = pool.begin().await.unwrap();
        let first = next_id_on(&mut tx, "tasks", "tasks").await.unwrap();
        let second = next_id_on(&mut tx, "tasks", "tasks").await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!((first, second), (1, 2));
        assert_eq!(next_id(&pool, "tasks", "tasks").await.unwrap(), 3);
    }
}
