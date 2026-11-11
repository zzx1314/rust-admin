use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::device::audit::{self, NewCommandAudit};
use crate::business::dog::device::business_client;
use crate::business::dog::device::control_channel::{ChannelError, DEFAULT_TIMEOUT};
use crate::business::dog::device::domain::{
    AssignUsersRequest, AssignableUserVO, CreateDeviceRequest, DeviceCommandRequest, DeviceFilter,
    DevicePageQuery, DeviceVO, DispatchTaskRequest, UnassignUsersRequest, UpdateDeviceRequest,
};
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use crate::common::util::generate_id;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use serde_json::{Value, json};

/// 允许从中台发起的设备指令白名单（与业务平台 `teleop::BUSINESS_COMMANDS` 对齐）。
/// 「同步地图」有独立入口 `syncMaps`，这里不重复暴露。
const ALLOWED_COMMANDS: [&str; 6] = [
    "开始巡检",
    "暂停任务",
    "继续任务",
    "召回充电",
    "远程接管",
    "释放接管",
];

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

#[derive(serde::Deserialize)]
pub struct DeviceCodeParam {
    pub device_code: String,
}

pub async fn create_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state
        .dog_device_service
        .create(req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

/// 业务平台设备同步入口：按设备编号幂等写入。
pub async fn upsert_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError("仅管理员可以同步设备台账".to_string()));
    }
    let device = state
        .dog_device_service
        .upsert(req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

pub async fn get_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state
        .dog_device_service
        .get_vo(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

pub async fn get_device_by_code_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<DeviceCodeParam>,
) -> Result<Json<ApiResponse<Option<DeviceVO>>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError(
            "仅管理员可以按编号查询设备".to_string(),
        ));
    }
    let device = state
        .dog_device_service
        .get_vo_by_code(&params.device_code)
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

pub async fn list_devices_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(query): Query<DevicePageQuery>,
) -> Result<Json<ApiResponse<PageResponse<DeviceVO>>>, AppError> {
    let page = state
        .dog_device_service
        .list_page(query, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(page)))
}

pub async fn list_all_devices_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(filter): Query<DeviceFilter>,
) -> Result<Json<ApiResponse<Vec<DeviceVO>>>, AppError> {
    let devices = state
        .dog_device_service
        .list_all(filter, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(devices)))
}

pub async fn update_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state
        .dog_device_service
        .update(params.id, req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

pub async fn delete_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .dog_device_service
        .delete(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

pub async fn assign_users_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<AssignUsersRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError("仅管理员可以分配设备".to_string()));
    }
    let affected = state.dog_device_service.assign_users(req).await?;
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "affected": affected,
    }))))
}

pub async fn unassign_users_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<UnassignUsersRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError("仅管理员可以解除设备分配".to_string()));
    }
    let affected = state.dog_device_service.unassign_users(req).await?;
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "affected": affected,
    }))))
}

pub async fn list_assignable_users_handler(
    State(state): State<AppState>,
    Extension(_user): Extension<RequestUser>,
) -> Result<Json<ApiResponse<Vec<AssignableUserVO>>>, AppError> {
    let users = state.dog_device_service.assignable_users().await?;
    Ok(Json(ApiResponse::ok(users)))
}

/// `POST /api/dogDevice/{id}/syncMaps` — 中台设备列表手动触发地图同步。
///
/// 中台不直连设备，而是调用业务平台内部接口，由业务平台通过设备 WebSocket
/// 下发 `同步地图` 指令；客户端再用 4012 拉取地图列表并从文件服务下载后回传。
pub async fn sync_device_maps_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError(
            "仅管理员可以触发设备地图同步".to_string(),
        ));
    }
    let device = state
        .dog_device_service
        .get(params.id, &user.data_scope())
        .await?;
    let data = deliver(
        &state,
        &user,
        &device.device_id,
        "sync-maps",
        "同步地图",
        "",
        json!({ "mode": "manual" }),
    )
    .await?;
    Ok(Json(ApiResponse::ok(data)))
}

/// 幂等键：调用方未指定时由中台生成一个，重试需原样回传。
fn resolve_request_id(value: Option<String>) -> String {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("cmd-{}", generate_id()))
}

/// 幂等命中时把上次的结果原样回放。
fn replay(
    device_code: &str,
    action: &str,
    request_id: &str,
    previous: &audit::CommandAudit,
) -> Value {
    json!({
        "deviceId": device_code,
        "action": action,
        "requestId": request_id,
        "duplicated": true,
        "status": previous.status,
        "result": previous
            .result
            .as_deref()
            .and_then(|text| serde_json::from_str::<Value>(text).ok()),
        "message": previous.message,
    })
}

/// 把一次控制请求送到业务平台。
///
/// 优先走业务平台主动拨入的反向信道（内网部署、无入站端口时唯一可用通路）；
/// 信道不在线时回退到直连 HTTP（中台与业务平台同网 / 已打通入站的老部署）。
async fn deliver(
    state: &AppState,
    user: &RequestUser,
    device_code: &str,
    suffix: &str,
    action: &str,
    request_id: &str,
    body: Value,
) -> Result<Value, AppError> {
    let kind = match suffix {
        "tasks" => "task",
        "runtime" => "runtime",
        "sync-maps" => "sync-maps",
        _ => "command",
    };
    // 运行态是只读查询，没有调用方提供的幂等键，这里补一个仅用于关联回执
    let request_id = if request_id.trim().is_empty() {
        format!("rt-{}", generate_id())
    } else {
        request_id.trim().to_string()
    };

    if state.control_channel.online().await {
        let payload = json!({
            "requestId": request_id,
            "kind": kind,
            "deviceCode": device_code,
            "action": action,
            "operatorId": user.user_id,
            "operatorAdmin": user.is_admin,
            "body": body,
        });
        match state
            .control_channel
            .request(&request_id, "request", payload, DEFAULT_TIMEOUT)
            .await
        {
            // 与 HTTP 直连模式返回同样形状的业务数据：调用方不需要知道链路
            Ok(data) => return Ok(data),
            // 业务平台的拒绝消息本身就是给运维看的（设备离线 / 未绑园区 / 指令非法）
            Err(ChannelError::Rejected(message)) => return Err(AppError::BadRequest(message)),
            Err(ChannelError::Timeout) => {
                return Err(AppError::BadRequest(format!(
                    "业务平台未在 {} 秒内回执，请检查业务平台与控制信道状态",
                    DEFAULT_TIMEOUT.as_secs()
                )));
            }
            Err(ChannelError::Disconnected) => {
                return Err(AppError::InternalError(
                    "控制信道在等待回执时断开".to_string(),
                ));
            }
            // 刚好断开：继续尝试 HTTP，避免因为一次重连把指令丢掉
            Err(ChannelError::Offline) => {}
        }
    }

    let path = business_client::device_path(device_code, suffix);
    let body = if kind == "runtime" { Value::Null } else { body };
    match kind {
        "runtime" => business_client::get(state, &path).await,
        _ => business_client::post(state, &path, &request_id, user.user_id, body).await,
    }
}

/// 一次控制面下发：查幂等键 → 落 pending 审计 → 调业务平台 → 回填审计。
///
/// 中台只做控制与审计，真正的载荷组装与 WebSocket 下发在业务平台。
async fn forward(
    state: &AppState,
    user: &RequestUser,
    device_code: &str,
    action: &str,
    suffix: &str,
    request_id: &str,
    body: Value,
) -> Result<Value, AppError> {
    let audit_id = match state.dog_device_service.find_command(request_id).await? {
        // 已经有结论：回放上次结果，不再向设备重复下发
        Some(previous) if previous.status != audit::STATUS_PENDING => {
            return Ok(replay(device_code, action, request_id, &previous));
        }
        // 上一次没拿到回执（超时/断线）：复用同一条审计继续，而不是插新行
        Some(previous) => previous.id,
        None => {
            state
                .dog_device_service
                .begin_command(NewCommandAudit {
                    user_id: user.user_id,
                    username: &user.username,
                    device_code,
                    action,
                    request_id,
                    params: Some(body.to_string()),
                })
                .await?
        }
    };

    let started = std::time::Instant::now();
    let outcome = deliver(state, user, device_code, suffix, action, request_id, body).await;
    let latency_ms = started.elapsed().as_millis() as i64;

    match outcome {
        Ok(data) => {
            state
                .dog_device_service
                .finish_command(
                    audit_id,
                    audit::STATUS_SUCCEEDED,
                    Some(&data.to_string()),
                    None,
                    latency_ms,
                )
                .await;
            Ok(json!({
                "deviceId": device_code,
                "action": action,
                "requestId": request_id,
                "duplicated": false,
                "status": audit::STATUS_SUCCEEDED,
                "latencyMs": latency_ms,
                "result": data,
            }))
        }
        Err(error) => {
            state
                .dog_device_service
                .finish_command(
                    audit_id,
                    audit::STATUS_FAILED,
                    None,
                    Some(&error.to_string()),
                    latency_ms,
                )
                .await;
            Err(error)
        }
    }
}

/// `POST /api/dogDevice/{id}/command` — 中台向设备下发一条指令。
///
/// 授权：管理员可操作全部设备，普通用户只能操作分配给自己的设备（`dog_user_device`）。
pub async fn send_device_command_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<DeviceCommandRequest>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let device = state
        .dog_device_service
        .get(params.id, &user.data_scope())
        .await?;
    let command = req.command.trim().to_string();
    if command.is_empty() {
        return Err(AppError::BadRequest("指令不能为空".to_string()));
    }
    if !ALLOWED_COMMANDS.contains(&command.as_str()) {
        return Err(AppError::BadRequest(format!("不支持的指令：{command}")));
    }
    let request_id = resolve_request_id(req.request_id);
    let body = json!({
        "command": command,
        "params": req.params,
        "requestId": request_id,
    });
    let data = forward(
        &state,
        &user,
        &device.device_id,
        &command,
        "command",
        &request_id,
        body,
    )
    .await?;
    Ok(Json(ApiResponse::ok(data)))
}

/// `POST /api/dogDevice/{id}/tasks` — 中台给指定设备下发一条巡检任务。
///
/// 路线（含航点/航段/动作）由业务平台持有，中台只选择路线与计划时间。
pub async fn dispatch_device_task_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<DispatchTaskRequest>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let device = state
        .dog_device_service
        .get(params.id, &user.data_scope())
        .await?;
    if req.task_name.trim().is_empty() {
        return Err(AppError::BadRequest("任务名称不能为空".to_string()));
    }
    if req.route_id <= 0 {
        return Err(AppError::BadRequest("请选择要执行的路线".to_string()));
    }
    let request_id = resolve_request_id(req.request_id);
    let body = json!({
        "taskName": req.task_name.trim(),
        "routeId": req.route_id,
        "planStart": req.plan_start,
        "planEnd": req.plan_end,
        "executeMode": req.execute_mode,
        "level": req.level,
        "description": req.description,
        "requestId": request_id,
    });
    let data = forward(
        &state,
        &user,
        &device.device_id,
        "下发任务",
        "tasks",
        &request_id,
        body,
    )
    .await?;
    Ok(Json(ApiResponse::ok(data)))
}

/// `GET /api/dogDevice/{id}/runtime` — 只读设备运行态（在线/任务/进度/电量）。
///
/// 不转发审计：这是查询，不是控制动作。
pub async fn device_runtime_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let device = state
        .dog_device_service
        .get(params.id, &user.data_scope())
        .await?;
    let data = deliver(
        &state,
        &user,
        &device.device_id,
        "runtime",
        "运行态查询",
        "",
        Value::Null,
    )
    .await?;
    Ok(Json(ApiResponse::ok(data)))
}
