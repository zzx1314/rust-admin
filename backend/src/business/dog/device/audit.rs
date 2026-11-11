//! 指令审计台账（`dog_command_audit`）。
//!
//! 中台既然承担控制面，就必须能回答「谁在什么时候对哪台设备下了什么指令、
//! 结果如何」。同时 `request_id` 上的唯一索引是幂等键：中台重试时不会再向
//! 设备重复下发（见 `handlers::send_device_command_handler`）。

use crate::common::util::{format_datetime, generate_id};
use chrono::Utc;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, DbErr, FromQueryResult, Statement, Value,
};

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_SUCCEEDED: &str = "succeeded";
pub const STATUS_FAILED: &str = "failed";

/// 一条指令记录（幂等回放只需要这几个字段）。
#[derive(Debug, Clone, FromQueryResult)]
pub struct CommandAudit {
    pub id: i64,
    pub request_id: String,
    pub status: String,
    pub result: Option<String>,
    pub message: Option<String>,
}

/// 待写入的指令记录。
pub struct NewCommandAudit<'a> {
    pub user_id: i64,
    pub username: &'a str,
    pub device_code: &'a str,
    pub action: &'a str,
    pub request_id: &'a str,
    pub params: Option<String>,
}

fn text(value: Option<String>) -> Value {
    Value::String(value.map(Box::new))
}

pub async fn find_by_request(
    conn: &DatabaseConnection,
    request_id: &str,
) -> Result<Option<CommandAudit>, DbErr> {
    CommandAudit::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "SELECT id, request_id, status, result, message FROM dog_command_audit \
         WHERE is_deleted = 0 AND request_id = ?1 LIMIT 1",
        vec![Value::from(request_id.trim())],
    ))
    .one(conn)
    .await
}

/// 写入 pending 记录并返回主键；`request_id` 冲突时说明并发重试已抢到，
/// 由调用方重新读一次即可。
pub async fn insert_pending(
    conn: &DatabaseConnection,
    entry: &NewCommandAudit<'_>,
) -> Result<i64, DbErr> {
    let id = generate_id();
    let now = format_datetime(Utc::now());
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO dog_command_audit \
         (id, user_id, username, device_code, action, request_id, params, status, \
          create_time, update_time, is_deleted) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, 0)",
        vec![
            Value::from(id),
            Value::from(entry.user_id),
            Value::from(entry.username),
            Value::from(entry.device_code),
            Value::from(entry.action),
            Value::from(entry.request_id),
            text(entry.params.clone()),
            Value::from(STATUS_PENDING),
            Value::from(now),
        ],
    ))
    .await?;
    Ok(id)
}

pub async fn finish(
    conn: &DatabaseConnection,
    id: i64,
    status: &str,
    result: Option<&str>,
    message: Option<&str>,
    latency_ms: i64,
) -> Result<(), DbErr> {
    let now = format_datetime(Utc::now());
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "UPDATE dog_command_audit \
         SET status = ?2, result = ?3, message = ?4, latency_ms = ?5, update_time = ?6 \
         WHERE id = ?1",
        vec![
            Value::from(id),
            Value::from(status),
            Value::String(result.map(|text| Box::new(text.to_string()))),
            Value::String(message.map(|text| Box::new(text.to_string()))),
            Value::from(latency_ms),
            Value::from(now),
        ],
    ))
    .await?;
    Ok(())
}
