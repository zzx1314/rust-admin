//! 中台 → 业务平台内部接口。
//!
//! 中台设备页的「同步地图」按钮不能直连设备：它调用这里，由业务平台通过
//! 已建立的设备 WebSocket 下发 `同步地图` 指令。接口用共享服务令牌鉴权，
//! 令牌只在两个服务端之间配置，不下发到浏览器或设备。

use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::gateway;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};

/// 服务令牌请求头（与中台 `APP_DOG__BUSINESS_API_TOKEN` 对应）
const SERVICE_TOKEN_HEADER: &str = "x-service-token";

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncMapsRequest {
    /// 保留字段：manual（中台手动触发）/ auto（设备自动回传）
    #[serde(default)]
    pub mode: Option<String>,
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let expected = state.config.service_token.as_deref().ok_or_else(|| {
        AppError::Internal(
            "业务平台未配置服务令牌（ROBODOG_SERVICE_TOKEN），无法接受中台内部请求".to_string(),
        )
    })?;
    let provided = headers
        .get(SERVICE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if provided.is_empty() || provided != expected {
        return Err(AppError::Unauthorized("服务令牌无效".to_string()));
    }
    Ok(())
}

/// `POST /api/zentai/devices/{device_id}/sync-maps`
///
/// 中台设备列表点击「同步地图」→ 这里向设备下发 `同步地图` 指令 →
/// 客户端用 4012 拉地图列表、从文件服务下载缺失地图并回传。
pub async fn sync_device_maps(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    payload: Option<Json<SyncMapsRequest>>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    authorize(&state, &headers)?;
    let mode = payload
        .and_then(|Json(request)| request.mode)
        .unwrap_or_else(|| "manual".to_string());

    let device: Option<(i64, Option<i64>, String)> =
        sqlx::query_as("SELECT id, park_id, status FROM devices WHERE device_id = ?1 LIMIT 1")
            .bind(device_id.trim())
            .fetch_optional(&state.db)
            .await?;
    let Some((device_pk, park_id, status)) = device else {
        return Err(AppError::NotFound(format!(
            "设备 {} 不在业务平台台账中",
            device_id.trim()
        )));
    };
    let Some(park_id) = park_id else {
        return Err(AppError::BadRequest(
            "设备尚未绑定园区，无法确定地图归属；请先在业务平台绑定园区".to_string(),
        ));
    };

    let timestamp = now();
    let dispatched = gateway::send_command_to_device(
        &state.device_connections,
        device_id.trim(),
        "同步地图",
        Some(json!({
            "mode": mode,
            "onlyMissing": true,
            "parkId": park_id,
            "requestedAt": timestamp,
        })),
    )
    .await;
    if !dispatched {
        return Err(AppError::BadRequest(format!(
            "设备 {} 当前未连接（状态：{}），无法下发同步指令",
            device_id.trim(),
            status
        )));
    }

    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('map_sync', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(format!("中台触发地图同步：{}", device_id.trim()))
    .bind(format!("模式：{mode}；园区：{park_id}；只同步缺失地图"))
    .bind(device_pk)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;

    Ok(ApiResponse::ok(json!({
        "deviceId": device_id.trim(),
        "parkId": park_id,
        "mode": mode,
        "dispatched": true,
        "requestedAt": timestamp,
    })))
}

// ─── 中台控制面：指令 / 任务 / 运行态 ──────────────────────────────
//
// 中台可以有「设备页下发指令」「给设备下任务」的按钮，但它不持有设备 WebSocket：
// 一律调用这里，由业务平台在既有连接上下发。三条约定：
//   1. `x-request-id` 是幂等键，重试时原样带回，命中则回放上次响应；
//   2. `x-operator-id` / `x-operator-admin` 是中台账号身份，用于二次鉴权与留痕；
//   3. 设备离线时显式报错，不把「未送达」记成成功。

use super::{devices, tasks};
use crate::models::TaskRequest;
use crate::zentai;
use tracing::warn;

const OPERATOR_HEADER: &str = "x-operator-id";
const OPERATOR_ADMIN_HEADER: &str = "x-operator-admin";
const REQUEST_ID_HEADER: &str = "x-request-id";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRequest {
    pub command: String,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDispatchRequest {
    pub task_name: String,
    pub route_id: i64,
    #[serde(default)]
    pub plan_start: Option<String>,
    #[serde(default)]
    pub plan_end: Option<String>,
    #[serde(default)]
    pub execute_mode: Option<String>,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
}

fn header_of(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn operator_of(headers: &HeaderMap) -> Option<i64> {
    header_of(headers, OPERATOR_HEADER).and_then(|value| value.parse::<i64>().ok())
}

fn operator_is_admin(headers: &HeaderMap) -> bool {
    matches!(header_of(headers, OPERATOR_ADMIN_HEADER).as_deref(), Some("1") | Some("true"))
}

/// 幂等键：优先用请求头，其次请求体；都没有就生成一个（中台没有走幂等的老客户端）。
fn request_id_of(headers: &HeaderMap, body: Option<String>) -> String {
    header_of(headers, REQUEST_ID_HEADER)
        .or_else(|| body.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()))
        .unwrap_or_else(|| {
            format!(
                "zt-{}",
                chrono::Local::now().timestamp_nanos_opt().unwrap_or_default()
            )
        })
}

/// 命中幂等键时返回上次的响应体（`response IS NULL` 表示上次没送达，允许重试）。
async fn replay_request(state: &AppState, request_id: &str) -> Result<Option<Value>, AppError> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT response FROM zentai_requests WHERE request_id = ?1 AND response IS NOT NULL",
    )
    .bind(request_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(row.map(|(response,)| {
        response
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or(Value::Null)
    }))
}

async fn begin_request(
    state: &AppState,
    request_id: &str,
    kind: &str,
    device_code: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT OR IGNORE INTO zentai_requests (request_id, kind, device_code, created_at) \
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(request_id)
    .bind(kind)
    .bind(device_code)
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(())
}

async fn finish_request(state: &AppState, request_id: &str, response: &Value) {
    if let Err(error) = sqlx::query("UPDATE zentai_requests SET response = ?2 WHERE request_id = ?1")
        .bind(request_id)
        .bind(response.to_string())
        .execute(&state.db)
        .await
    {
        warn!("记录中台请求响应失败（{request_id}）: {error}");
    }
}

/// 二次鉴权：中台已按 `dog_user_device` 校验过一次，这里再用中台分配关系核一遍。
///
/// 中台查询失败时放行（授权在中台，共享令牌本身已被信任），只记日志，
/// 避免中台偶发抖动导致指令链路整条不可用。
async fn ensure_operator_access(
    state: &AppState,
    headers: &HeaderMap,
    device_code: &str,
) -> Result<(), AppError> {
    let Some(operator) = operator_of(headers) else {
        return Ok(());
    };
    match zentai::user_can_access_device(
        &state.config,
        Some(operator),
        operator_is_admin(headers),
        device_code,
    )
    .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(AppError::Unauthorized(format!(
            "中台账号 {operator} 未被分配设备 {device_code}"
        ))),
        Err(error) => {
            warn!("中台分配关系校验失败，按中台授权结果放行（{device_code}）: {error}");
            Ok(())
        }
    }
}

/// `POST /api/zentai/devices/{device_id}/command`
///
/// 中台设备页的指令按钮 → 这里做白名单/园区校验 → 业务平台 WS 下发。
pub async fn device_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_code): Path<String>,
    Json(payload): Json<CommandRequest>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    authorize(&state, &headers)?;
    let device_code = device_code.trim().to_string();
    let request_id = request_id_of(&headers, payload.request_id.clone());

    if let Some(previous) = replay_request(&state, &request_id).await? {
        return Ok(ApiResponse::ok(json!({
            "duplicated": true,
            "requestId": request_id,
            "result": previous,
        })));
    }

    ensure_operator_access(&state, &headers, &device_code).await?;
    let device = devices::fetch_device_by_code(&state, &device_code).await?;
    let operator = header_of(&headers, OPERATOR_HEADER);

    begin_request(&state, &request_id, "command", &device_code).await?;
    let view = devices::push_command(
        &state,
        &device,
        &payload.command,
        payload.params,
        operator.as_deref(),
    )
    .await?;

    // 未送达要显式失败：中台据此把审计记成 failed，而不是「已下发」
    if view.get("wsDelivered").and_then(Value::as_bool) == Some(false) {
        return Err(AppError::BadRequest(format!(
            "设备 {} 当前未连接（状态：{}），指令未送达",
            device_code, device.status
        )));
    }

    finish_request(&state, &request_id, &view).await;
    Ok(ApiResponse::ok(json!({
        "duplicated": false,
        "requestId": request_id,
        "result": view,
    })))
}

/// `POST /api/zentai/devices/{device_id}/tasks`
///
/// 中台只选路线与计划时间；航点/航段/动作、地图绑定、限速与结束动作
/// 全部由业务平台按 `routes.route_data` 组装后下发。
pub async fn dispatch_task(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_code): Path<String>,
    Json(payload): Json<TaskDispatchRequest>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    authorize(&state, &headers)?;
    let device_code = device_code.trim().to_string();
    let request_id = request_id_of(&headers, payload.request_id.clone());

    if let Some(previous) = replay_request(&state, &request_id).await? {
        return Ok(ApiResponse::ok(json!({
            "duplicated": true,
            "requestId": request_id,
            "result": previous,
        })));
    }

    if payload.task_name.trim().is_empty() {
        return Err(AppError::BadRequest("任务名称不能为空".to_string()));
    }
    ensure_operator_access(&state, &headers, &device_code).await?;
    let device = devices::fetch_device_by_code(&state, &device_code).await?;
    let park_id = device.park_id.ok_or_else(|| {
        AppError::BadRequest(format!(
            "设备 {device_code} 尚未绑定园区，无法下发任务；请先在业务平台绑定园区"
        ))
    })?;

    begin_request(&state, &request_id, "task", &device_code).await?;
    let task = tasks::create_task_core(
        &state,
        TaskRequest {
            task_code: None,
            task_name: payload.task_name.trim().to_string(),
            task_type: None,
            execute_mode: payload.execute_mode,
            level: payload.level,
            device_id: Some(device.id),
            park_id: Some(park_id),
            zone_id: device.zone_id,
            route_id: Some(payload.route_id),
            plan_start: payload.plan_start,
            plan_end: payload.plan_end,
            task_status: Some("待执行".to_string()),
            progress: None,
            dispatch_policy: None,
            description: payload.description,
        },
    )
    .await?;

    let view = json!({
        "taskId": task.id,
        "taskCode": task.task_code,
        "taskName": task.task_name,
        "taskStatus": task.task_status,
        "deviceId": device_code,
        "routeId": payload.route_id,
    });
    finish_request(&state, &request_id, &view).await;
    Ok(ApiResponse::ok(json!({
        "duplicated": false,
        "requestId": request_id,
        "result": view,
    })))
}

/// `GET /api/zentai/devices/{device_id}/runtime`
///
/// 只读运行态：中台设备页/看板展示用，不改变任何状态，因此不做幂等。
pub async fn device_runtime(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_code): Path<String>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    authorize(&state, &headers)?;
    let device_code = device_code.trim().to_string();
    let device = devices::fetch_device_by_code(&state, &device_code).await?;
    let online = state
        .device_connections
        .read()
        .await
        .contains_key(&device_code);

    let current: Option<(String, String, String, i64, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT task_code, task_name, task_status, progress, plan_start, plan_end \
             FROM tasks WHERE device_id = ?1 ORDER BY id DESC LIMIT 1",
        )
        .bind(device.id)
        .fetch_optional(&state.db)
        .await?;

    Ok(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "online": online,
        "status": device.status,
        "battery": device.battery,
        "signal": device.signal,
        "temperature": device.temperature,
        "position": device.position,
        "lastHeartbeat": device.last_heartbeat,
        "currentTaskName": device.current_task_name,
        "parkId": device.park_id,
        "parkName": device.park_name,
        "teleopHolder": device.teleop_holder,
        "videoUrl": device.video_url,
        "videoType": device.video_type,
        "currentTask": current.map(|(task_code, task_name, task_status, progress, plan_start, plan_end)| json!({
            "taskCode": task_code,
            "taskName": task_name,
            "taskStatus": task_status,
            "progress": progress,
            "planStart": plan_start,
            "planEnd": plan_end,
        })),
        "serverTs": now(),
    })))
}
