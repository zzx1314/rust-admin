use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::gateway;
use crate::gateway_types::TaskDispatchPayload;
use crate::models::{Task, TaskQuery, TaskRequest, TaskStatusRequest};
use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::{Value, json};
use sqlx::QueryBuilder;
use tracing::info;

const TASK_SELECT: &str = "SELECT t.id, t.task_code, t.task_name, t.task_type, t.execute_mode,
    t.level, t.device_id, d.device_id AS device_code, t.park_id, p.park_name, t.zone_id,
    z.zone_name, t.route_id, r.route_code, r.route_name, t.plan_start, t.plan_end,
    t.task_status, t.progress, t.dispatch_policy, t.description, t.created_at, t.updated_at
    FROM tasks t
    LEFT JOIN devices d ON d.id = t.device_id
    LEFT JOIN parks p ON p.id = t.park_id
    LEFT JOIN zones z ON z.id = t.zone_id
    LEFT JOIN routes r ON r.id = t.route_id";

pub async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<TaskQuery>,
) -> Result<Json<ApiResponse<Vec<Task>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(TASK_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(status) = query
        .task_status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND t.task_status = ").push_bind(status);
    }
    if let Some(device_id) = query.device_id {
        builder.push(" AND t.device_id = ").push_bind(device_id);
    }
    if let Some(park_id) = query.park_id {
        builder.push(" AND t.park_id = ").push_bind(park_id);
    }
    if let Some(route_id) = query.route_id {
        builder.push(" AND t.route_id = ").push_bind(route_id);
    }
    builder.push(" ORDER BY t.created_at DESC, t.id DESC");
    let tasks = builder
        .build_query_as::<Task>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(tasks))
}

pub async fn task_stats(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT task_status, COUNT(*) FROM tasks GROUP BY task_status")
            .fetch_all(&state.db)
            .await?;
    let total: i64 = rows.iter().map(|(_, count)| count).sum();
    let mut counts = serde_json::Map::new();
    for (status, count) in rows {
        counts.insert(status, json!(count));
    }
    let done = counts
        .get("已完成")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let rate = if total == 0 {
        0
    } else {
        (done * 100 / total) as i64
    };
    Ok(ApiResponse::ok(json!({
        "total": total,
        "counts": counts,
        "doneRate": rate,
    })))
}

pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    Ok(ApiResponse::ok(fetch_task(&state, id).await?))
}

pub async fn create_task(
    State(state): State<AppState>,
    Json(payload): Json<TaskRequest>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    if payload.task_name.trim().is_empty() {
        return Err(AppError::BadRequest("任务名称不能为空".to_string()));
    }
    let next_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM tasks")
        .fetch_one(&state.db)
        .await?;
    let task_code = payload
        .task_code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("TK-{}-{next_id:02}", chrono::Local::now().format("%m%d")));
    let timestamp = now();
    let result = sqlx::query(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, level, device_id,
            park_id, zone_id, route_id, plan_start, plan_end, task_status, progress,
            dispatch_policy, description, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
    )
    .bind(&task_code)
    .bind(payload.task_name.trim())
    .bind(payload.task_type)
    .bind(payload.execute_mode)
    .bind(payload.level.unwrap_or_else(|| "常规".to_string()))
    .bind(payload.device_id)
    .bind(payload.park_id)
    .bind(payload.zone_id)
    .bind(payload.route_id)
    .bind(payload.plan_start)
    .bind(payload.plan_end)
    .bind(payload.task_status.unwrap_or_else(|| "待执行".to_string()))
    .bind(payload.progress.unwrap_or(0))
    .bind(payload.dispatch_policy)
    .bind(payload.description)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    let id = result.last_insert_rowid();
    record_event(
        &state,
        "created",
        format!("创建任务：{}", payload.task_name.trim()),
        "任务已加入待执行队列".to_string(),
        payload.device_id,
        Some(id),
    )
    .await?;

    // 如果任务已分配到设备，尝试通过 WebSocket 推送
    if let Some(device_id) = payload.device_id {
        let device_code: Option<String> =
            sqlx::query_scalar("SELECT device_id FROM devices WHERE id = ?1")
                .bind(device_id)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();

        if let Some(ref dcode) = device_code {
            let task = fetch_task(&state, id).await?;
            let route_data: Option<Value> = if let Some(rid) = task.route_id {
                let row: Option<String> = sqlx::query_scalar("SELECT route_data FROM routes WHERE id = ?1")
                    .bind(rid)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten();
                row.and_then(|s| serde_json::from_str(&s).ok())
            } else {
                None
            };

            let dispatch = TaskDispatchPayload {
                task_id: task.id,
                task_code: task.task_code.clone(),
                task_name: task.task_name.clone(),
                task_type: task.task_type.clone(),
                execute_mode: task.execute_mode.clone(),
                route: route_data.clone().map(|_| json!({
                    "routeId": task.route_id,
                    "routeCode": task.route_code,
                    "routeName": task.route_name,
                })),
                route_data,
                plan_start: task.plan_start,
                plan_end: task.plan_end,
                dispatch_policy: task.dispatch_policy,
                description: task.description,
            };

            let pushed = gateway::dispatch_task_to_device(&state.device_connections, dcode, &dispatch).await;
            if pushed {
                // 更新任务状态为执行中
                sqlx::query("UPDATE tasks SET task_status = '执行中', updated_at = ?1 WHERE id = ?2")
                    .bind(now())
                    .bind(id)
                    .execute(&state.db)
                    .await?;
                info!("task {} pushed via WebSocket to device {}", task.task_code, dcode);
            }
        }
    }

    Ok(ApiResponse::ok(fetch_task(&state, id).await?))
}

pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<TaskRequest>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let existing = fetch_task(&state, id).await?;
    let status = payload.task_status.unwrap_or(existing.task_status);
    let progress = payload.progress.unwrap_or(existing.progress);
    sqlx::query(
        "UPDATE tasks SET task_name = ?1, task_type = ?2, execute_mode = ?3, level = ?4,
            device_id = ?5, park_id = ?6, zone_id = ?7, route_id = ?8, plan_start = ?9,
            plan_end = ?10, task_status = ?11, progress = ?12, dispatch_policy = ?13,
            description = ?14, updated_at = ?15 WHERE id = ?16",
    )
    .bind(if payload.task_name.trim().is_empty() {
        existing.task_name
    } else {
        payload.task_name.trim().to_string()
    })
    .bind(payload.task_type.or(existing.task_type))
    .bind(payload.execute_mode.or(existing.execute_mode))
    .bind(payload.level.or(existing.level))
    .bind(payload.device_id.or(existing.device_id))
    .bind(payload.park_id.or(existing.park_id))
    .bind(payload.zone_id.or(existing.zone_id))
    .bind(payload.route_id.or(existing.route_id))
    .bind(payload.plan_start.or(existing.plan_start))
    .bind(payload.plan_end.or(existing.plan_end))
    .bind(status)
    .bind(progress)
    .bind(payload.dispatch_policy.or(existing.dispatch_policy))
    .bind(payload.description.or(existing.description))
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_task(&state, id).await?))
}

pub async fn update_task_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<TaskStatusRequest>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let existing = fetch_task(&state, id).await?;
    let valid = ["待执行", "执行中", "已完成", "异常", "已暂停"];
    if !valid.contains(&payload.task_status.as_str()) {
        return Err(AppError::BadRequest("无效的任务状态".to_string()));
    }
    let progress = payload
        .progress
        .unwrap_or(match payload.task_status.as_str() {
            "已完成" => 100,
            "待执行" => 0,
            _ => existing.progress,
        });
    sqlx::query("UPDATE tasks SET task_status = ?1, progress = ?2, updated_at = ?3 WHERE id = ?4")
        .bind(&payload.task_status)
        .bind(progress)
        .bind(now())
        .bind(id)
        .execute(&state.db)
        .await?;
    record_event(
        &state,
        "status",
        format!(
            "任务 {} 状态变更为 {}",
            existing.task_code, payload.task_status
        ),
        format!("当前进度 {}%", progress),
        existing.device_id,
        Some(id),
    )
    .await?;
    Ok(ApiResponse::ok(fetch_task(&state, id).await?))
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    sqlx::query("DELETE FROM task_events WHERE task_id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    let result = sqlx::query("DELETE FROM tasks WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

async fn fetch_task(state: &AppState, id: i64) -> Result<Task, AppError> {
    let sql = format!("{TASK_SELECT} WHERE t.id = ?1");
    sqlx::query_as::<_, Task>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("任务不存在".to_string()))
}

async fn record_event(
    state: &AppState,
    event_type: &str,
    title: String,
    description: String,
    device_id: Option<i64>,
    task_id: Option<i64>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(event_type)
    .bind(title)
    .bind(description)
    .bind(device_id)
    .bind(task_id)
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(())
}
