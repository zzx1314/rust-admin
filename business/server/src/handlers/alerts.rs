use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{Alert, AlertQuery, AlertRequest};
use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::{Value, json};
use sqlx::QueryBuilder;

const ALERT_SELECT: &str = "SELECT a.id, a.alert_code, a.level, a.content, a.device_id,
    d.device_id AS device_code, a.park_id, p.park_name, a.status, a.occurred_at, a.handled_at,
    a.created_at
    FROM alerts a
    LEFT JOIN devices d ON d.id = a.device_id
    LEFT JOIN parks p ON p.id = a.park_id";

pub async fn list_alerts(
    State(state): State<AppState>,
    Query(query): Query<AlertQuery>,
) -> Result<Json<ApiResponse<Vec<Alert>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(ALERT_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND a.status = ").push_bind(status);
    }
    if let Some(level) = query
        .level
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND a.level = ").push_bind(level);
    }
    if let Some(park_id) = query.park_id {
        builder.push(" AND a.park_id = ").push_bind(park_id);
    }
    builder.push(" ORDER BY a.occurred_at DESC, a.id DESC");
    let alerts = builder
        .build_query_as::<Alert>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(alerts))
}

pub async fn alert_stats(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM alerts GROUP BY status")
            .fetch_all(&state.db)
            .await?;
    let total: i64 = rows.iter().map(|(_, count)| count).sum();
    let mut counts = serde_json::Map::new();
    for (status, count) in rows {
        counts.insert(status, json!(count));
    }
    Ok(ApiResponse::ok(json!({
        "total": total,
        "counts": counts,
        "pending": counts.get("未处理").and_then(Value::as_i64).unwrap_or_default(),
    })))
}

pub async fn get_alert(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Alert>>, AppError> {
    Ok(ApiResponse::ok(fetch_alert(&state, id).await?))
}

pub async fn create_alert(
    State(state): State<AppState>,
    Json(payload): Json<AlertRequest>,
) -> Result<Json<ApiResponse<Alert>>, AppError> {
    if payload.content.trim().is_empty() {
        return Err(AppError::BadRequest("告警内容不能为空".to_string()));
    }
    let next_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM alerts")
        .fetch_one(&state.db)
        .await?;
    let alert_code = payload
        .alert_code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("AL-{next_id:03}"));
    let occurred_at = payload.occurred_at.unwrap_or_else(now);
    let status = payload.status.unwrap_or_else(|| "未处理".to_string());
    let result = sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, device_id, park_id, status,
            occurred_at, handled_at, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&alert_code)
    .bind(payload.level)
    .bind(payload.content.trim())
    .bind(payload.device_id)
    .bind(payload.park_id)
    .bind(&status)
    .bind(&occurred_at)
    .bind(if status == "已处理" {
        Some(now())
    } else {
        None
    })
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(
        fetch_alert(&state, result.last_insert_rowid()).await?,
    ))
}

pub async fn update_alert(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<AlertRequest>,
) -> Result<Json<ApiResponse<Alert>>, AppError> {
    let existing = fetch_alert(&state, id).await?;
    let status = payload.status.unwrap_or(existing.status);
    sqlx::query(
        "UPDATE alerts SET level = ?1, content = ?2, device_id = ?3, park_id = ?4,
            status = ?5, occurred_at = ?6, handled_at = ?7 WHERE id = ?8",
    )
    .bind(payload.level)
    .bind(if payload.content.trim().is_empty() {
        existing.content
    } else {
        payload.content.trim().to_string()
    })
    .bind(payload.device_id.or(existing.device_id))
    .bind(payload.park_id.or(existing.park_id))
    .bind(&status)
    .bind(payload.occurred_at.unwrap_or(existing.occurred_at))
    .bind(if status == "已处理" {
        existing.handled_at.or_else(|| Some(now()))
    } else {
        None
    })
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_alert(&state, id).await?))
}

pub async fn handle_alert(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Alert>>, AppError> {
    let alert = fetch_alert(&state, id).await?;
    let timestamp = now();
    sqlx::query("UPDATE alerts SET status = '已处理', handled_at = ?1 WHERE id = ?2")
        .bind(&timestamp)
        .bind(id)
        .execute(&state.db)
        .await?;
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('alert', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(format!("{} 已标记为处理完成", alert.alert_code))
    .bind(&alert.content)
    .bind(alert.device_id)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_alert(&state, id).await?))
}

pub async fn handle_all_alerts(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<i64>>, AppError> {
    let timestamp = now();
    let result = sqlx::query(
        "UPDATE alerts SET status = '已处理', handled_at = ?1 WHERE status <> '已处理'",
    )
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(result.rows_affected() as i64))
}

pub async fn delete_alert(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let result = sqlx::query("DELETE FROM alerts WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

async fn fetch_alert(state: &AppState, id: i64) -> Result<Alert, AppError> {
    let sql = format!("{ALERT_SELECT} WHERE a.id = ?1");
    sqlx::query_as::<_, Alert>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("告警不存在".to_string()))
}
