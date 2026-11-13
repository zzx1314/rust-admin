use crate::app::AppState;
use crate::auth::{self, Claims};
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::gateway;
use crate::models::{BindParkRequest, CommandRequest, Device, DeviceQuery, DeviceRequest, LocateRequest};
use crate::zentai;
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, SqlitePool};
use tracing::info;

/// 视频地址归一化：去空白，空串视为未配置
fn normalize_video_url(raw: Option<String>) -> Option<String> {
    raw.map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
}

/// 视频类型归一化：未显式给出时按地址后缀推断
///
/// - 用户明确选了类型 → 用用户的选择（非法值回落 `none`）
/// - 用户没选（`None` 或 `none`）但填了地址 → 按后缀推断
fn normalize_video_type(raw: Option<String>, url: Option<&str>) -> String {
    let explicit = raw
        .map(|kind| kind.trim().to_ascii_lowercase())
        .filter(|kind| !kind.is_empty() && kind != "none");
    match explicit {
        Some(kind) if VIDEO_KINDS.contains(&kind.as_str()) => kind,
        Some(_) => "none".to_string(),
        None => url.map(infer_video_type).unwrap_or("none").to_string(),
    }
}

const VIDEO_KINDS: [&str; 6] = ["hls", "flv", "webrtc", "rtsp", "mp4", "none"];

/// 从地址推断视频协议，供前端在用户只填了地址时自动选择播放器
fn infer_video_type(url: &str) -> &'static str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("rtsp://") {
        "rtsp"
    } else if lower.ends_with(".m3u8") {
        "hls"
    } else if lower.ends_with(".flv") {
        "flv"
    } else if lower.ends_with(".mp4") || lower.ends_with(".webm") {
        "mp4"
    } else if lower.starts_with("webrtc://") || lower.starts_with("whep://") {
        "webrtc"
    } else {
        "none"
    }
}

pub(crate) const DEVICE_SELECT: &str = "SELECT d.id, d.device_id, d.model, d.park_id, p.park_name,
    d.zone_id, z.zone_name, d.status, d.battery, d.temperature, d.runtime_minutes,
    d.odometer, d.position, d.current_task_name, d.last_heartbeat, d.firmware, d.device_ip,
    d.video_url, COALESCE(d.video_type, 'none') AS video_type,
    d.teleop_holder, d.teleop_at, COALESCE(d.teleop_force, 0) AS teleop_force,
    COALESCE(d.body_online, 0) AS body_online, d.run_mode,
    COALESCE(d.charging, 0) AS charging, d.robot_mode,
    COALESCE(d.loc_status, 255) AS loc_status, d.active_map_id,
    COALESCE(d.ctrl_source, 0) AS ctrl_source, d.motion_status,
    d.estop_software, d.sdk_node_available,
    d.created_at, d.updated_at
    FROM devices d
    LEFT JOIN parks p ON p.id = d.park_id
    LEFT JOIN zones z ON z.id = d.zone_id";

/// 未绑定园区的设备状态：新注册 / 解绑后的默认状态，不参与任务下发。
pub const DEVICE_STATUS_UNBOUND: &str = "待绑定";

/// 新建设备时的默认状态：有园区视为在线待命，无园区则为待绑定。
fn default_status(park_id: Option<i64>) -> String {
    if park_id.is_some() {
        "在线待命".to_string()
    } else {
        DEVICE_STATUS_UNBOUND.to_string()
    }
}

/// 校验园区存在，且（可选）区域属于该园区。
async fn validate_park_zone(
    db: &SqlitePool,
    park_id: Option<i64>,
    zone_id: Option<i64>,
) -> Result<(), AppError> {
    let Some(park_id) = park_id else {
        if zone_id.is_some() {
            return Err(AppError::BadRequest("绑定区域前请先选择园区".to_string()));
        }
        return Ok(());
    };
    let park_name: Option<String> = sqlx::query_scalar("SELECT park_name FROM parks WHERE id = ?1")
        .bind(park_id)
        .fetch_optional(db)
        .await?;
    if park_name.is_none() {
        return Err(AppError::NotFound(format!("园区 {park_id} 不存在")));
    }
    if let Some(zone_id) = zone_id {
        let belongs: Option<i64> =
            sqlx::query_scalar("SELECT id FROM zones WHERE id = ?1 AND park_id = ?2")
                .bind(zone_id)
                .bind(park_id)
                .fetch_optional(db)
                .await?;
        if belongs.is_none() {
            return Err(AppError::BadRequest(format!(
                "区域 {zone_id} 不属于园区 {park_id}"
            )));
        }
    }
    Ok(())
}

pub async fn list_devices(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<DeviceQuery>,
) -> Result<Json<ApiResponse<Vec<Device>>>, AppError> {
    let allowed = auth::allowed_device_codes(&state, &claims).await?;
    if matches!(&allowed, Some(codes) if codes.is_empty()) {
        return Ok(ApiResponse::ok(Vec::new()));
    }
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
    if let Some(codes) = &allowed {
        builder.push(" AND d.device_id IN (");
        for (index, code) in codes.iter().enumerate() {
            if index > 0 {
                builder.push(", ");
            }
            builder.push_bind(code.clone());
        }
        builder.push(")");
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
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let allowed = auth::allowed_device_codes(&state, &claims).await?;
    if matches!(&allowed, Some(codes) if codes.is_empty()) {
        return Ok(ApiResponse::ok(json!({
            "total": 0,
            "online": 0,
            "unbound": 0,
            "counts": {},
        })));
    }

    let mut builder =
        QueryBuilder::<sqlx::Sqlite>::new("SELECT status, COUNT(*) FROM devices WHERE 1 = 1");
    if let Some(codes) = &allowed {
        builder.push(" AND device_id IN (");
        for (index, code) in codes.iter().enumerate() {
            if index > 0 {
                builder.push(", ");
            }
            builder.push_bind(code.clone());
        }
        builder.push(")");
    }
    builder.push(" GROUP BY status");
    let rows: Vec<(String, i64)> = builder
        .build_query_as::<(String, i64)>()
        .fetch_all(&state.db)
        .await?;
    let total: i64 = rows.iter().map(|(_, count)| count).sum();
    let mut unbound_builder =
        QueryBuilder::<sqlx::Sqlite>::new("SELECT COUNT(*) FROM devices WHERE park_id IS NULL");
    if let Some(codes) = &allowed {
        unbound_builder.push(" AND device_id IN (");
        for (index, code) in codes.iter().enumerate() {
            if index > 0 {
                unbound_builder.push(", ");
            }
            unbound_builder.push_bind(code.clone());
        }
        unbound_builder.push(")");
    }
    let unbound: i64 = unbound_builder
        .build_query_scalar()
        .fetch_one(&state.db)
        .await?;
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
        "unbound": unbound,
        "counts": counts,
    })))
}

pub async fn get_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Device>>, AppError> {
    let device = fetch_device(&state, id).await?;
    auth::ensure_device_access(&state, &claims, &device.device_id).await?;
    Ok(ApiResponse::ok(device))
}

pub async fn create_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
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
    let park_id = payload.park_id;
    let zone_id = payload.zone_id;
    validate_park_zone(&state.db, park_id, zone_id).await?;
    let status = payload.status.unwrap_or_else(|| default_status(park_id));
    let video_url = normalize_video_url(payload.video_url);
    let video_type = normalize_video_type(payload.video_type, video_url.as_deref());
    let result = sqlx::query(
        "INSERT INTO devices (device_id, model, park_id, zone_id, status, battery,
            temperature, runtime_minutes, odometer, position, current_task_name, last_heartbeat,
            firmware, device_ip, video_url, video_type, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
            ?18)",
    )
    .bind(payload.device_id.trim())
    .bind(payload.model)
    .bind(park_id)
    .bind(zone_id)
    .bind(status)
    .bind(payload.battery.unwrap_or(100))
    .bind(payload.temperature.unwrap_or(32.0))
    .bind(payload.runtime_minutes.unwrap_or(0))
    .bind(payload.odometer.unwrap_or(0.0))
    .bind(payload.position)
    .bind(payload.current_task_name.unwrap_or_else(|| "—".to_string()))
    .bind(payload.last_heartbeat.unwrap_or_else(now))
    .bind(payload.firmware.unwrap_or_else(|| "v2.6.1".to_string()))
    .bind(payload.device_ip)
    .bind(video_url)
    .bind(video_type)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    let device = fetch_device(&state, result.last_insert_rowid()).await?;
    sync_device_to_zentai(&state, &device, &claims).await;
    Ok(ApiResponse::ok(device))
}

pub async fn update_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(payload): Json<DeviceRequest>,
) -> Result<Json<ApiResponse<Device>>, AppError> {
    let existing = fetch_device(&state, id).await?;
    auth::ensure_device_access(&state, &claims, &existing.device_id).await?;
    // 园区 / 区域：换了园区且未显式指定区域时自动清空区域，避免跨园区脏数据
    let park_changed = payload.park_id.is_some() && payload.park_id != existing.park_id;
    let park_id = payload.park_id.or(existing.park_id);
    let zone_id = match payload.zone_id {
        Some(zone_id) => Some(zone_id),
        None if park_changed => None,
        None => existing.zone_id,
    };
    validate_park_zone(&state.db, park_id, zone_id).await?;
    // 视频配置：请求里没带就沿用旧值；带了空串表示清空
    let video_url = match payload.video_url {
        Some(url) => normalize_video_url(Some(url)),
        None => existing.video_url.clone(),
    };
    let video_type = normalize_video_type(payload.video_type, video_url.as_deref());
    // 已绑定园区但仍是待绑定的设备，保存后恢复正常在线待命
    let status = payload.status.unwrap_or_else(|| {
        if park_id.is_some() && existing.status == DEVICE_STATUS_UNBOUND {
            "在线待命".to_string()
        } else {
            existing.status.clone()
        }
    });
    sqlx::query(
        "UPDATE devices SET device_id = ?1, model = ?2, park_id = ?3, zone_id = ?4,
            status = ?5, battery = ?6, temperature = ?7, runtime_minutes = ?8,
            odometer = ?9, position = ?10, current_task_name = ?11, last_heartbeat = ?12,
            firmware = ?13, device_ip = ?14, video_url = ?15, video_type = ?16, updated_at = ?17
         WHERE id = ?18",
    )
    .bind(if payload.device_id.trim().is_empty() {
        existing.device_id
    } else {
        payload.device_id.trim().to_string()
    })
    .bind(payload.model.or(existing.model))
    .bind(park_id)
    .bind(zone_id)
    .bind(status)
    .bind(payload.battery.unwrap_or(existing.battery))
    .bind(payload.temperature.unwrap_or(existing.temperature))
    .bind(payload.runtime_minutes.unwrap_or(existing.runtime_minutes))
    .bind(payload.odometer.unwrap_or(existing.odometer))
    .bind(payload.position.or(existing.position))
    .bind(payload.current_task_name.or(existing.current_task_name))
    .bind(payload.last_heartbeat.or(existing.last_heartbeat))
    .bind(payload.firmware.or(existing.firmware))
    .bind(payload.device_ip.or(existing.device_ip))
    .bind(&video_url)
    .bind(&video_type)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    let device = fetch_device(&state, id).await?;
    sync_device_to_zentai(&state, &device, &claims).await;
    Ok(ApiResponse::ok(device))
}

/// `POST /api/devices/{id}/bind` — 手动把设备绑定到园区（可同时指定区域）。
pub async fn bind_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(payload): Json<BindParkRequest>,
) -> Result<Json<ApiResponse<Device>>, AppError> {
    let device = fetch_device(&state, id).await?;
    auth::ensure_device_access(&state, &claims, &device.device_id).await?;
    validate_park_zone(&state.db, Some(payload.park_id), payload.zone_id).await?;
    let (park_code, park_name): (String, String) =
        sqlx::query_as("SELECT park_code, park_name FROM parks WHERE id = ?1")
            .bind(payload.park_id)
            .fetch_one(&state.db)
            .await?;
    let timestamp = now();
    // 只把「待绑定」推进为在线待命，已在线 / 充电 / 执行中的状态保持不变
    let status = if device.status == DEVICE_STATUS_UNBOUND {
        "在线待命".to_string()
    } else {
        device.status.clone()
    };
    sqlx::query(
        "UPDATE devices SET park_id = ?1, zone_id = ?2, status = ?3, updated_at = ?4 WHERE id = ?5",
    )
    .bind(payload.park_id)
    .bind(payload.zone_id)
    .bind(&status)
    .bind(&timestamp)
    .bind(id)
    .execute(&state.db)
    .await?;
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('binding', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(format!(
        "设备 {} 已绑定园区：{}",
        device.device_id, park_name
    ))
    .bind(format!("绑定前状态：{}；绑定后：{}", device.status, status))
    .bind(id)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    // 绑定后通知在线设备（建图回传归属），并补发该园区的待执行任务
    gateway::notify_park_binding(
        &state.device_connections,
        &device.device_id,
        Some(payload.park_id),
        Some(&park_code),
        Some(&park_name),
    )
    .await;
    gateway::dispatch_pending_for_device(&state.db, &state.device_connections, &device.device_id)
        .await;
    let device = fetch_device(&state, id).await?;
    sync_device_to_zentai(&state, &device, &claims).await;
    Ok(ApiResponse::ok(device))
}

/// `DELETE /api/devices/{id}/bind` — 解除园区绑定，设备回到「待绑定」。
pub async fn unbind_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Device>>, AppError> {
    let device = fetch_device(&state, id).await?;
    auth::ensure_device_access(&state, &claims, &device.device_id).await?;
    if device.park_id.is_none() {
        return Ok(ApiResponse::ok(device));
    }
    let timestamp = now();
    let status = if device.status == "离线" {
        "离线".to_string()
    } else {
        DEVICE_STATUS_UNBOUND.to_string()
    };
    sqlx::query(
        "UPDATE devices SET park_id = NULL, zone_id = NULL, status = ?1, updated_at = ?2
         WHERE id = ?3",
    )
    .bind(&status)
    .bind(&timestamp)
    .bind(id)
    .execute(&state.db)
    .await?;
    // 未开始执行的任务不能再留在未绑定设备上
    sqlx::query(
        "UPDATE tasks SET device_id = NULL WHERE device_id = ?1 AND task_status = '待执行'",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('binding', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(format!("设备 {} 已解除园区绑定", device.device_id))
    .bind(format!(
        "原园区：{}",
        device.park_name.unwrap_or_else(|| "—".to_string())
    ))
    .bind(id)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    // 通知在线设备：已解绑，建图不再自动回传
    gateway::notify_park_binding(
        &state.device_connections,
        &device.device_id,
        None,
        None,
        None,
    )
    .await;
    let device = fetch_device(&state, id).await?;
    sync_device_to_zentai(&state, &device, &claims).await;
    Ok(ApiResponse::ok(device))
}

pub async fn delete_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let device = fetch_device(&state, id).await?;
    auth::ensure_device_access(&state, &claims, &device.device_id).await?;
    sqlx::query("UPDATE tasks SET device_id = NULL WHERE device_id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    let result = sqlx::query("DELETE FROM devices WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    if state.config.zentai_ready()
        && let Err(error) = zentai::delete_device(&state.config, &device.device_id).await
    {
        tracing::warn!("删除中台设备 {} 失败: {error}", device.device_id);
    }
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

/// 设备「刷新状态」：探活，而不是伪造心跳。
///
/// 以前这个接口把所有非离线设备的 `last_heartbeat` 直接刷成当前时间，
/// 页面一看全是「刚刚上报」，和设备是否真的在连毫无关系。
/// 现在改成查连接池：有活跃 WebSocket 的保持不动并计数，没有的直接置离线。
pub async fn refresh_devices(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<i64>>, AppError> {
    let timestamp = now();
    let connected: Vec<String> = state
        .device_connections
        .read()
        .await
        .keys()
        .cloned()
        .collect();
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, device_id FROM devices WHERE status <> '离线'")
            .fetch_all(&state.db)
            .await?;

    let mut alive = 0_i64;
    for (id, code) in rows {
        if connected.iter().any(|item| item == &code) {
            alive += 1;
            continue;
        }
        sqlx::query(
            "UPDATE devices SET status = '离线', body_online = 0,
                teleop_holder = NULL, teleop_at = NULL, teleop_force = 0,
                updated_at = ?1 WHERE id = ?2",
        )
        .bind(&timestamp)
        .bind(id)
        .execute(&state.db)
        .await?;
    }

    Ok(ApiResponse::ok(alive))
}

pub async fn send_command(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(payload): Json<CommandRequest>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let device = fetch_device(&state, id).await?;
    auth::ensure_device_access(&state, &claims, &device.device_id).await?;
    let result = push_command(
        &state,
        &device,
        &payload.command,
        None,
        Some(&claims.username),
    )
    .await?;
    Ok(ApiResponse::ok(result))
}

/// 重新定位：载图并重定位（`4030 op=1`），用于「机器狗不在图上」的恢复。
///
/// 地图解析顺序：请求 `mapId` → 设备上报的当前地图 → 园区地图基线。
/// 位姿缺省时下零四元数，由 SLAM 读上一份位姿文件做完整重定位。
pub async fn locate_device(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
    Json(payload): Json<LocateRequest>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let device = fetch_device(&state, id).await?;
    auth::ensure_device_access(&state, &claims, &device.device_id).await?;
    let pose = payload.pose().map_err(AppError::BadRequest)?;

    let requested = payload
        .map_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let map_id = match (requested, device.active_map_id.clone(), device.park_id) {
        (Some(map_id), _, _) => Some(map_id),
        (None, Some(active), _) => Some(active),
        (None, None, Some(park_id)) => sqlx::query_scalar::<_, Option<String>>(
            "SELECT map_version_id FROM parks WHERE id = ?1",
        )
        .bind(park_id)
        .fetch_optional(&state.db)
        .await?
        .flatten(),
        _ => None,
    };
    let Some(map_id) = map_id else {
        return Err(AppError::BadRequest(
            "设备尚未上报当前地图，且所属园区没有地图基线；请显式指定 mapId".to_string(),
        ));
    };

    // 本体目录名必须与业务 ID 区分：目录名由建图会话决定，客户端会用 4013 核对
    let map_dir = crate::mapdir::map_dir(&state.db, &map_id).await?;
    let result = push_command(
        &state,
        &device,
        "重新定位",
        Some(json!({ "mapId": map_id, "mapDir": map_dir, "pose": pose })),
        Some(&claims.username),
    )
    .await?;
    // 设备离线时显式报错：重定位必须在机器人本身上执行，不能记成「已下发」
    if result.get("wsDelivered").and_then(Value::as_bool) == Some(false) {
        return Err(AppError::BadRequest(format!(
            "设备 {} 未在线，重新定位指令未送达",
            device.device_id
        )));
    }
    Ok(ApiResponse::ok(result))
}

/// 设备事件流（最近 N 条）。
///
/// 客户端把「指令被本地门禁拒绝」这类事写成 `event_type=error` 的事件，而
/// `POST /devices/{id}/action` 仍然返回 200 + `wsDelivered=true`（命令确实送进了
/// 设备连接）。没有这个接口，控制台只能看到「点了没反应」，只能翻日志才知道原因。
pub async fn list_device_events(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Vec<crate::models::TaskEvent>>>, AppError> {
    let device = fetch_device(&state, id).await?;
    auth::ensure_device_access(&state, &claims, &device.device_id).await?;
    let events = sqlx::query_as::<_, crate::models::TaskEvent>(
        "SELECT e.id, e.event_type, e.title, e.description, d.device_id AS device_code,
            t.task_code, e.created_at
         FROM task_events e
         LEFT JOIN devices d ON d.id = e.device_id
         LEFT JOIN tasks t ON t.id = e.task_id
         WHERE e.device_id = ?1
         ORDER BY e.created_at DESC, e.id DESC LIMIT 30",
    )
    .bind(device.id)
    .fetch_all(&state.db)
    .await?;
    Ok(ApiResponse::ok(events))
}

/// 指令下发的唯一实现。
/// 网页端（`send_command`）与中台内部接口（`handlers::service`）都走这里，
/// 保证白名单、园区校验、状态流转、事件留痕、WS 下发只有一套语义。
///
/// `operator` 是中台账号名，仅用于事件描述，便于与中台 `dog_command_audit` 对账。
pub(crate) async fn push_command(
    state: &AppState,
    device: &Device,
    command: &str,
    extra_params: Option<Value>,
    operator: Option<&str>,
) -> Result<Value, AppError> {
    // 指令白名单：避免前端拼错或恶意字符串直接透传到客户端
    let command = super::teleop::normalize_command(command)?;
    // 未绑定园区的设备没有地图与路线归属，不允许启动巡检
    if device.park_id.is_none() && matches!(command, "开始巡检" | "继续任务") {
        return Err(AppError::BadRequest(
            "设备尚未绑定园区，无法执行任务指令".to_string(),
        ));
    }
    // 任务/运动类指令先过本体健康门禁；远程接管/释放/同步地图不涉及运动，放行。
    // 设备根本没有 WS 连接时保留原「未送达」语义，避免把“没连上”误报成“本体不健康”。
    let connected = gateway::is_device_connected(&state.device_connections, &device.device_id).await;
    if connected
        && matches!(
            command,
            "开始巡检" | "暂停任务" | "继续任务" | "召回充电" | "重新定位"
        )
    {
        super::teleop::ensure_ready(device)?;
    }
    // 接管中不启动/继续任务：本体在手动模式，5000 会回 13897 capability busy。
    if matches!(command, "开始巡检" | "继续任务")
        && device.teleop_holder.is_some()
        && !super::teleop::teleop_expired(device.teleop_at.as_deref())
    {
        return Err(AppError::BadRequest(format!(
            "设备 {} 正在远程接管中，请先释放接管再下发任务",
            device.device_id
        )));
    }
    // 「本体重连」是运维手段（本体链路/会话半死时重建），会重启设备客户端进程：
    // 带任务 / 接管 / 建图 / 模拟模式时一律拒，避免丢运行态。
    if command == "本体重连" {
        if !connected {
            return Err(AppError::BadRequest(
                "设备客户端未连接（不在线），无法下发重连指令".to_string(),
            ));
        }
        if device.run_mode.as_deref() == Some("sim") {
            return Err(AppError::BadRequest(
                "设备客户端处于模拟模式（未接本体），无需重连".to_string(),
            ));
        }
        if device.status == "执行任务" {
            return Err(AppError::BadRequest(format!(
                "设备 {} 正在执行任务，请先暂停/取消任务再重连",
                device.device_id
            )));
        }
        if device.teleop_holder.is_some() && !super::teleop::teleop_expired(device.teleop_at.as_deref())
        {
            return Err(AppError::BadRequest(format!(
                "设备 {} 正在远程接管中，请先释放接管再重连（重连会丢掉接管态）",
                device.device_id
            )));
        }
    }
    let id = device.id;
    let next_status = match command {
        "开始巡检" => Some("执行任务"),
        "暂停任务" | "释放接管" => Some("在线待命"),
        "继续任务" => Some("执行任务"),
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
    let description = match operator {
        Some(operator) => format!("当前设备状态：{}；操作人：{operator}", device.status),
        None => format!("当前设备状态：{}", device.status),
    };
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('command', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(format!("已向 {} 下发指令：{}", device.device_id, command))
    .bind(description)
    .bind(id)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;

    // 如果设备在线，通过 WebSocket 下发指令；`force` 只在「遥控器仍报 ctrl_source=3」
    // 且本次是接管时才有意义：客户端需要它才能跳过自己的 ctrl_source 门禁发 1304。
    let force = command == "远程接管"
        && device.ctrl_source == 3
        && extra_params
            .as_ref()
            .and_then(|extra| extra.get("force"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let mut payload = extra_params.map_or_else(
        || json!({"dispatchedAt": timestamp, "operator": operator}),
        |extra| json!({"dispatchedAt": timestamp, "operator": operator, "params": extra}),
    );
    payload["force"] = json!(force);
    let cmd_sent = gateway::send_command_to_device(
        &state.device_connections,
        &device.device_id,
        command,
        Some(payload),
    )
    .await;

    if cmd_sent {
        info!(
            "command '{}' pushed via WebSocket to device {}",
            command, device.device_id
        );
    }

    // 接管/释放同时维护接管态，避免前端还要多调一次接口
    // holder 必须与 `/teleop/take` 使用同一标识，否则遥控会被判成「他人接管」
    let teleop_holder = match command {
        "远程接管" => {
            let holder = super::teleop::operator_id(device);
            sqlx::query(
                "UPDATE devices SET teleop_holder = ?1, teleop_at = ?2, teleop_force = ?3 \
                 WHERE id = ?4",
            )
            .bind(&holder)
            .bind(&timestamp)
            .bind(if force { 1 } else { 0 })
            .bind(id)
            .execute(&state.db)
            .await?;
            Some(holder)
        }
        "释放接管" => {
            sqlx::query(
                "UPDATE devices SET teleop_holder = NULL, teleop_at = NULL, teleop_force = 0 \
                 WHERE id = ?1",
            )
            .bind(id)
            .execute(&state.db)
            .await?;
            None
        }
        _ => device.teleop_holder.clone(),
    };

    Ok(json!({
        "deviceId": device.device_id,
        "command": command,
        "dispatchedAt": timestamp,
        "wsDelivered": cmd_sent,
        "teleopHolder": teleop_holder,
        "forced": force,
    }))
}

/// 把业务库设备同步到中台台账；失败只记日志，不阻断业务操作。
async fn sync_device_to_zentai(state: &AppState, device: &Device, claims: &Claims) {
    if !state.config.zentai_ready() {
        return;
    }
    let park = match device.park_id {
        Some(park_id) => sqlx::query_as::<_, (String, String)>(
            "SELECT park_code, park_name FROM parks WHERE id = ?1",
        )
        .bind(park_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten(),
        None => None,
    };
    let user_ids = claims
        .zentai_user_id
        .filter(|_| !claims.is_admin)
        .into_iter()
        .collect();
    let payload = zentai::DeviceUpsert {
        device_id: device.device_id.clone(),
        device_name: None,
        model: device.model.clone(),
        status: Some(device.status.clone()),
        firmware: device.firmware.clone(),
        device_ip: device.device_ip.clone(),
        park_code: park.as_ref().map(|(code, _)| code.clone()),
        park_name: park.as_ref().map(|(_, name)| name.clone()),
        remarks: None,
        user_ids,
    };
    zentai::try_upsert_device(&state.config, &payload).await;
}

async fn fetch_device(state: &AppState, id: i64) -> Result<Device, AppError> {
    let sql = format!("{DEVICE_SELECT} WHERE d.id = ?1");
    sqlx::query_as::<_, Device>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("设备不存在".to_string()))
}

/// 按设备编号（本体 SN）取设备，供中台内部接口使用。
pub(crate) async fn fetch_device_by_code(
    state: &AppState,
    device_code: &str,
) -> Result<Device, AppError> {
    let sql = format!("{DEVICE_SELECT} WHERE d.device_id = ?1");
    sqlx::query_as::<_, Device>(&sql)
        .bind(device_code.trim())
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| {
            AppError::NotFound(format!("设备 {} 不在业务平台台账中", device_code.trim()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unbound_devices_default_to_pending_status() {
        assert_eq!(default_status(None), DEVICE_STATUS_UNBOUND);
        assert_eq!(default_status(Some(3)), "在线待命");
    }
}
