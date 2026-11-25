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
use std::time::Duration;

/// 「下发地图」是同步等待设备真实回执的操作（写本体 + 重新载图），
/// 比普通指令慢得多；其余控制请求仍用 8s 的默认回执超时。
const PUSH_MAP_TIMEOUT: Duration = Duration::from_secs(65);

/// 「同步媒体」：异步长任务，中台下发后审计停在 pending，等客户端回报结论。
/// 超过这么久还没结论，就当这轮丢了（客户端崩溃 / 断网），允许重新下发。
const MEDIA_SYNC_ACTION: &str = "同步媒体";
/// 「回传录像」：同一类长任务。2026-11-25 起录像不再自动回传，
/// 板端把 mp4 攺在 `RECORD_DIR`，由中台/业务平台下发这条指令才传。
const RECORD_SYNC_ACTION: &str = "回传录像";
/// 长任务指令（下发后审计停在 pending，等客户端异步回报进度与结论）。
const LONG_TASK_ACTIONS: [&str; 2] = [MEDIA_SYNC_ACTION, RECORD_SYNC_ACTION];
const MEDIA_SYNC_STALE_SECS: i64 = 600;

/// 这条指令是不是「长任务」（下发后审计停在 pending，等客户端异步回报进度与结论）。
pub fn is_long_task_action(action: &str) -> bool {
    LONG_TASK_ACTIONS.contains(&action)
}

/// 允许从中台发起的设备指令白名单（与业务平台 `teleop::BUSINESS_COMMANDS` 对齐）。
/// 「同步地图」有独立入口 `syncMaps`，这里不重复暴露。
const ALLOWED_COMMANDS: [&str; 8] = [
    "开始巡检",
    "暂停任务",
    "继续任务",
    "召回充电",
    "远程接管",
    "释放接管",
    // 把板端 `/media/media-data/picture` 里的 AI 存证图回传到中台「媒体资源」
    "同步媒体",
    // 把板端 `RECORD_DIR` 里攺着的巡检录像回传到业务平台「任务录像」
    // （录像不再自动回传，这条是按需触发）
    "回传录像",
];

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

/// `?action=回传录像`：看哪一条长任务的状态（缺省走「同步媒体」，保持老页面兼容）。
#[derive(serde::Deserialize)]
pub struct LongTaskStatusQuery {
    #[serde(default)]
    pub action: Option<String>,
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
pub(crate) fn resolve_request_id(value: Option<String>) -> String {
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
        "push-map" => "push-map",
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
        let timeout = if kind == "push-map" {
            PUSH_MAP_TIMEOUT
        } else {
            DEFAULT_TIMEOUT
        };
        match state
            .control_channel
            .request(&request_id, "request", payload, timeout)
            .await
        {
            // 与 HTTP 直连模式返回同样形状的业务数据：调用方不需要知道链路
            Ok(data) => return Ok(data),
            // 业务平台的拒绝消息本身就是给运维看的（设备离线 / 未绑园区 / 指令非法）
            Err(ChannelError::Rejected(message)) => return Err(AppError::BadRequest(message)),
            Err(ChannelError::Timeout) => {
                return Err(AppError::BadRequest(format!(
                    "业务平台未在 {} 秒内回执，请检查业务平台与控制信道状态",
                    timeout.as_secs()
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
        "push-map" => {
            business_client::post_with_timeout(
                state,
                &path,
                &request_id,
                user.user_id,
                body,
                PUSH_MAP_TIMEOUT,
            )
            .await
        }
        _ => business_client::post(state, &path, &request_id, user.user_id, body).await,
    }
}

/// 一次控制面下发：查幂等键 → 落 pending 审计 → 调业务平台 → 回填审计。
///
/// 中台只做控制与审计，真正的载荷组装与 WebSocket 下发在业务平台。
pub(crate) async fn forward(
    state: &AppState,
    user: &RequestUser,
    device_code: &str,
    action: &str,
    suffix: &str,
    request_id: &str,
    body: Value,
) -> Result<Value, AppError> {
    // 已经有一轮长任务在跑（「同步媒体」/「回传录像」）：不重复下发（重复下发会让
    // 板端同时跑两轮扫描 / 上传，还抢同一条上行带宽），直接把「进行中」回给页面。
    // 超过 `MEDIA_SYNC_STALE_SECS` 没结论就当丢了，走原来的「复用审计 + 重新下发」。
    if is_long_task_action(action)
        && let Some(previous) = state.dog_device_service.find_command(request_id).await?
        && previous.status == audit::STATUS_PENDING
        && !audit_is_stale(&previous)
    {
        return Ok(replay(device_code, action, request_id, &previous));
    }
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
            // 长任务（「同步媒体」/「回传录像」）：下发成功只代表设备客户端收到了，
            // 图片要一张张 / 录像要一段段传上来（4G 上一轮可能跑几分钟到几十分钟）。
            // 真实结论由客户端异步回报（→ 业务平台 →
            // `POST /api/dogInternal/media/sync-status`），所以审计**停在 pending**，
            // 设备页显示「进行中」而不是假的「已完成」。
            if is_long_task_action(action)
                && data
                    .get("wsDelivered")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            {
                state
                    .dog_device_service
                    .record_device_report(
                        request_id,
                        None,
                        Some(&data.to_string()),
                        Some(if action == RECORD_SYNC_ACTION {
                            "已下发，等待设备客户端回传录像"
                        } else {
                            "已下发，等待设备客户端回传同步结果"
                        }),
                    )
                    .await
                    .ok();
                return Ok(json!({
                    "deviceId": device_code,
                    "action": action,
                    "requestId": request_id,
                    "duplicated": false,
                    "status": audit::STATUS_PENDING,
                    "awaitingDevice": true,
                    "latencyMs": latency_ms,
                    "result": data,
                }));
            }
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

/// pending 指令是否已经「等到发霉」：`update_time` / `create_time` 是北京时间字符串。
fn audit_is_stale(record: &audit::CommandAudit) -> bool {
    use chrono::TimeZone;
    let stamp = record
        .update_time
        .as_deref()
        .unwrap_or(record.create_time.as_str());
    let Ok(at) = chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S") else {
        return false;
    };
    let at = chrono::Utc.from_utc_datetime(&at) - chrono::Duration::hours(8);
    chrono::Utc::now().signed_duration_since(at).num_seconds() > MEDIA_SYNC_STALE_SECS
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
    let params = command_params(&state, &command, req.params, &user.username, &request_id).await;
    let body = json!({
        "command": command,
        "params": params,
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

/// 指令参数：设备侧需要的「板端事实」由中台补齐，前端只按按钮。
///
/// 长任务（「同步媒体」/「回传录像」）都带 `requestId`：客户端用它回传进度与最终
/// 结论（`command_progress` / `command_result` → 业务平台 → 本机内部接口）。
/// 「同步媒体」另外还要板端存证目录与固定帧数（在「参数配置」页维护
/// `dog_config.media_picture_dir` / `media_frames`），现场改配置即可生效。
async fn command_params(
    state: &AppState,
    command: &str,
    params: Option<Value>,
    operator: &str,
    request_id: &str,
) -> Value {
    let mut params = params.unwrap_or_else(|| json!({}));
    if !is_long_task_action(command) {
        return params;
    }
    // 长任务都要：客户端按 `requestId` 回传进度与最终结论（业务平台会把它包进
    // `params.params`，设备客户端少认一层就整条回执发不出来）。
    params["operator"] = json!(operator);
    params["requestId"] = json!(request_id);
    if command == MEDIA_SYNC_ACTION {
        // 板端存证目录与每航点帧数只属于「同步媒体」：现场在「参数配置」页改，
        // 不必登板子改 `config.py`；配置缺失/写坏时回退默认值，不影响指令下发。
        let config =
            crate::business::dog::media::service::load_capture_config(&state.dog_config_service)
                .await;
        params["pictureDir"] = json!(config.picture_dir);
        params["frames"] = json!(config.frames);
    }
    params
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
        "endTask": req.end_task,
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

/// `GET /api/dogDevice/{id}/mediaSyncStatus` — 最近一次长任务的「同步媒体」/「回传录像」状态。
///
/// 两条都是长任务（扫板端目录 → 传平台），下发接口只回「已下发」。
/// 设备页靠这个查询看进度与结论，`?action=` 选看哪一条（缺省 = 「同步媒体」）：
///
/// - `pending`：还在跑（`message` 里带「已上传 8/24 张」这种进度）；
/// - `succeeded`：客户端回报的真结论（扫了几张、传了几张、目录是否清空）；
/// - `failed`：客户端报错 / 本轮失败；
/// - `null`：这台设备从未「同步媒体」过。
pub async fn media_sync_status_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Query(query): Query<LongTaskStatusQuery>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let action = query
        .action
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(MEDIA_SYNC_ACTION);
    if !LONG_TASK_ACTIONS.contains(&action) {
        return Err(AppError::BadRequest(format!(
            "不支持的长任务指令：{action}"
        )));
    }
    let device = state
        .dog_device_service
        .get(params.id, &user.data_scope())
        .await?;
    let latest = state
        .dog_device_service
        .latest_command(&device.device_id, action)
        .await?;
    let Some(record) = latest else {
        return Ok(Json(ApiResponse::ok(Value::Null)));
    };
    let stale = record.status == audit::STATUS_PENDING && audit_is_stale(&record);
    Ok(Json(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "action": action,
        "requestId": record.request_id,
        "status": record.status,
        // 等太久还没结论：页面刷成「没有回报」而不是一直转圈
        "stale": stale,
        "message": record.message,
        "result": record
            .result
            .as_deref()
            .and_then(|text| serde_json::from_str::<Value>(text).ok()),
        "createTime": record.create_time,
        "updateTime": record.update_time,
    }))))
}
