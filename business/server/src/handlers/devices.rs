use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{CommandRequest, Device, DeviceQuery, DeviceRequest};
use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::{Value, json};
use sqlx::QueryBuilder;

const DEVICE_SELECT: &str = "SELECT d.id, d.device_id, d.model, d.park_id, p.park_name,
    d.zone_id, z.zone_name, d.status, d.battery, d.signal, d.temperature, d.runtime_minutes,
    d.odometer, d.position, d.current_task_name, d.last_heartbeat, d.firmware, d.device_ip,
    d.created_at, d.updated_at
    FROM devices d
    LEFT JOIN parks p ON p.id = d.park_id
    LEFT JOIN zones z ON z.id = d.zone_id";

pub async fn list_devices(
    State(state): State<AppState>,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<ApiResponse<Vec<Device>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(DEVICE_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(q) = query.q.filter(|value| !value.trim().is_empty()) {
        let keyword = format!("%{}%", q.trim());
        builder
            .push(" AND (d.device_id LIKE ")
            .push_bind(keyword.clone())
            .push(" OR d.model LIKE ")
            .push_bind(keyword.clone())
            .push(" OR p.park_name LIKE ")
            .push_bind(keyword.clone())
            .push(" OR z.zone_name LIKE ")
            .push_bind(keyword.clone())
            .push(" OR d.current_task_name LIKE ")
            .push_bind(keyword)
            .push(")");
    }
    if let Some(park_id) = query.park_id {
        builder.push(" AND d.park_id = ").push_bind(park_id);
    }
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        if status == "在线" {
            builder.push(" AND d.status IN ('执行任务', '在线待命')");
        } else {
            builder.push(" AND d.status = ").push_bind(status);
        }
    }
    builder.push(" ORDER BY d.id ASC");
    let devices = builder
        .build_query_as::<Device>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(devices))
}

pub async fn device_stats(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM devices GROUP BY status")
            .fetch_all(&state.db)
            .await?;
    let total: i64 = rows.iter().map(|(_, count)| count).sum();
    let mut counts = serde_json::Map::new();
    let mut online = 0;
    for (status, count) in rows {
        if status == "执行任务" || status == "在线待命" {
            online += count;
        }
        counts.insert(status, json!(count));
    }
    Ok(ApiResponse::ok(json!({
        "total": total,
        "online": online,
        "counts": counts,
    })))
}

pub async fn get_device(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Device>>, AppError> {
    Ok(ApiResponse::ok(fetch_device(&state, id).await?))
}

pub async fn create_device(
    State(state): State<AppState>,
    Json(payload): Json<DeviceRequest>,
) -> Result<Json<ApiResponse<Device>>, AppError> {
    if payload.device_id.trim().is_empty() {
        return Err(AppError::BadRequest("设备编号不能为空".to_string()));
    }
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM devices WHERE device_id = ?1")
        .bind(payload.device_id.trim())
        .fetch_optional(&state.db)
        .await?;
    if exists.is_some() {
        return Err(AppError::BadRequest("设备编号已存在".to_string()));
    }
    let timestamp = now();
    let result = sqlx::query(
        "INSERT INTO devices (device_id, model, park_id, zone_id, status, battery, signal,
            temperature, runtime_minutes, odometer, position, current_task_name, last_heartbeat,
            firmware, device_ip, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )
    .bind(payload.device_id.trim())
    .bind(payload.model)
    .bind(payload.park_id)
    .bind(payload.zone_id)
    .bind(payload.status.unwrap_or_else(|| "在线待命".to_string()))
    .bind(payload.battery.unwrap_or(100))
    .bind(payload.signal.unwrap_or(4))
    .bind(payload.temperature.unwrap_or(32.0))
    .bind(payload.runtime_minutes.unwrap_or(0))
    .bind(payload.odometer.unwrap_or(0.0))
    .bind(payload.position)
    .bind(payload.current_task_name.unwrap_or_else(|| "—".to_string()))
    .bind(payload.last_heartbeat.unwrap_or_else(now))
    .bind(payload.firmware.unwrap_or_else(|| "v2.6.1".to_string()))
    .bind(payload.device_ip)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    let device = fetch_device(&state, result.last_insert_rowid()).await?;
    Ok(ApiResponse::ok(device))
}

pub async fn update_device(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<DeviceRequest>,
) -> Result<Json<ApiResponse<Device>>, AppError> {
    let existing = fetch_device(&state, id).await?;
    sqlx::query(
        "UPDATE devices SET device_id = ?1, model = ?2, park_id = ?3, zone_id = ?4,
            status = ?5, battery = ?6, signal = ?7, temperature = ?8, runtime_minutes = ?9,
            odometer = ?10, position = ?11, current_task_name = ?12, last_heartbeat = ?13,
            firmware = ?14, device_ip = ?15, updated_at = ?16 WHERE id = ?17",
    )
    .bind(if payload.device_id.trim().is_empty() {
        existing.device_id
    } else {
        payload.device_id.trim().to_string()
    })
    .bind(payload.model.or(existing.model))
    .bind(payload.park_id.or(existing.park_id))
    .bind(payload.zone_id.or(existing.zone_id))
    .bind(payload.status.unwrap_or(existing.status))
    .bind(payload.battery.unwrap_or(existing.battery))
    .bind(payload.signal.unwrap_or(existing.signal))
    .bind(payload.temperature.unwrap_or(existing.temperature))
    .bind(payload.runtime_minutes.unwrap_or(existing.runtime_minutes))
    .bind(payload.odometer.unwrap_or(existing.odometer))
    .bind(payload.position.or(existing.position))
    .bind(payload.current_task_name.or(existing.current_task_name))
    .bind(payload.last_heartbeat.or(existing.last_heartbeat))
    .bind(payload.firmware.or(existing.firmware))
    .bind(payload.device_ip.or(existing.device_ip))
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_device(&state, id).await?))
}

pub async fn delete_device(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    sqlx::query("UPDATE tasks SET device_id = NULL WHERE device_id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    let result = sqlx::query("DELETE FROM devices WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

pub async fn refresh_devices(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<i64>>, AppError> {
    let timestamp = now();
    let result = sqlx::query(
        "UPDATE devices SET last_heartbeat = ?1, updated_at = ?1 WHERE status <> '离线'",
    )
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(result.rows_affected() as i64))
}

pub async fn send_command(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<CommandRequest>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let device = fetch_device(&state, id).await?;
    let next_status = match payload.command.as_str() {
        "开始巡检" => Some("执行任务"),
        "暂停任务" => Some("在线待命"),
        "召回充电" => Some("充电中"),
        _ => None,
    };
    let timestamp = now();
    if let Some(status) = next_status {
        sqlx::query("UPDATE devices SET status = ?1, updated_at = ?2 WHERE id = ?3")
            .bind(status)
            .bind(&timestamp)
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('command', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(format!(
        "已向 {} 下发指令：{}",
        device.device_id, payload.command
    ))
    .bind(format!("当前设备状态：{}", device.status))
    .bind(id)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;

    Ok(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "command": payload.command,
        "dispatchedAt": timestamp,
    })))
}

async fn fetch_device(state: &AppState, id: i64) -> Result<Device, AppError> {
    let sql = format!("{DEVICE_SELECT} WHERE d.id = ?1");
    sqlx::query_as::<_, Device>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("设备不存在".to_string()))
}
