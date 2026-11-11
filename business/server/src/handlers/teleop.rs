//! 远程接管与遥控
//!
//! 遥控直接驱动机器狗，误操作代价高，所以这里有一道**接管态**闸门：
//!
//! ```text
//! 无人接管 ──take-over──► 已接管（只允许同一个 holder 发遥控）
//!     ▲                        │
//!     └──release / 超时 / 断连──┘
//! ```
//!
//! 例外是**急停**：任何状态下都放行，不受接管态约束。
//!
//! 接管态落库（`devices.teleop_holder` / `teleop_at`），不用内存，
//! 这样多实例部署或服务重启后不会留下「幽灵接管」。

use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::gateway;
use crate::models::{Device, TeleopRequest};
use axum::Json;
use axum::extract::{Path, State};
use serde_json::json;
use tracing::{info, warn};

/// 接管超时（秒）。超过这个时间没有任何遥控指令，接管自动失效。
const TELEOP_IDLE_TIMEOUT_SECS: i64 = 120;

/// 本体动作指令（走智航 1302）
///
/// `id` 是下发给本体的动作码，`label` 用于审计日志。
const BODY_ACTIONS: [(&str, &str); 6] = [
    ("stand_up", "站立"),
    ("lie_down", "卧倒"),
    ("emergency_stop", "急停"),
    ("light_on", "开灯"),
    ("light_off", "关灯"),
    ("reset_pose", "位姿复位"),
];

/// 允许通过 `/command` 下发的业务指令
const BUSINESS_COMMANDS: [&str; 7] = [
    "开始巡检",
    "暂停任务",
    "继续任务",
    "召回充电",
    "同步地图",
    "远程接管",
    "释放接管",
];

fn invalid(message: impl Into<String>) -> AppError {
    AppError::BadRequest(message.into())
}

/// 接管人标识
///
/// 鉴权中间件没有把用户名透传到 handler，这里退化为设备维度标识，
/// 但必须和 `/devices/{id}/command` 的「远程接管」写成同一个值，
/// 否则两边会互相判定为「已被他人接管」。
pub(crate) fn operator_id(device: &Device) -> String {
    format!("operator@{}", device.device_id)
}

/// 解析当前登录人，作为接管 holder
async fn current_operator(state: &AppState, device: &Device) -> String {
    // 业务服务的鉴权中间件已把 token 校验前置，这里取不到用户名时退化为设备维度标识，
    // 保证「同一台设备同一时间只有一个接管者」这条不变量成立。
    let _ = state;
    operator_id(device)
}

/// 校验接管态，返回 Err 表示不允许下发遥控
fn ensure_holding(device: &Device, operator: &str) -> Result<(), AppError> {
    if device.status == "离线" {
        return Err(invalid("设备离线，无法远程控制"));
    }
    let Some(holder) = device.teleop_holder.as_deref() else {
        return Err(invalid("尚未接管，请先点击「远程接管」"));
    };
    if holder != operator {
        return Err(invalid(format!("设备已被 {holder} 接管，请先释放")));
    }
    if is_teleop_expired(device) {
        return Err(invalid("接管已超时，请重新接管"));
    }
    Ok(())
}

/// 判断接管是否已超时（`teleop_at` 距今超过阈值）
fn is_teleop_expired(device: &Device) -> bool {
    let Some(at) = device.teleop_at.as_deref() else {
        return true;
    };
    match parse_time(at) {
        Some(at_secs) => now_secs() - at_secs > TELEOP_IDLE_TIMEOUT_SECS,
        // 时间解析不了就当作已超时，宁可让用户重新接管也不放开控制
        None => true,
    }
}

fn now_secs() -> i64 {
    // 统一走 db::now() 的字符串形态，避免两套时间来源
    parse_time(&now()).unwrap_or(0)
}

/// 解析 `YYYY-MM-DD HH:MM:SS` 形态的时间戳为秒
fn parse_time(text: &str) -> Option<i64> {
    use chrono::{NaiveDateTime, TimeZone, Utc};
    let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").ok()?;
    let local = chrono::Local.from_local_datetime(&naive).single()?;
    Some(local.with_timezone(&Utc).timestamp())
}

async fn load_device(state: &AppState, id: i64) -> Result<Device, AppError> {
    let sql = format!("{} WHERE d.id = ?1", super::devices::DEVICE_SELECT);
    sqlx::query_as::<_, Device>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("设备不存在".to_string()))
}

async fn write_teleop(state: &AppState, id: i64, holder: Option<&str>) -> Result<(), AppError> {
    let stamp = now();
    sqlx::query("UPDATE devices SET teleop_holder = ?1, teleop_at = ?2 WHERE id = ?3")
        .bind(holder)
        .bind(holder.map(|_| stamp.clone()))
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(())
}

async fn log_event(
    state: &AppState,
    device: &Device,
    title: String,
    description: String,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('teleop', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(title)
    .bind(description)
    .bind(device.id)
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(())
}

/// POST /api/devices/{id}/teleop/take —— 接管
pub async fn take_over(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let device = load_device(&state, id).await?;
    if device.status == "离线" {
        return Err(invalid("设备离线，无法接管"));
    }
    let operator = current_operator(&state, &device).await;

    if let Some(holder) = device.teleop_holder.as_deref()
        && holder != operator
        && !is_teleop_expired(&device)
    {
        return Err(invalid(format!("设备已被 {holder} 接管")));
    }

    write_teleop(&state, id, Some(&operator)).await?;
    log_event(
        &state,
        &device,
        format!("{} 接管 {}", operator, device.device_id),
        "远程接管已建立，可下发摇杆与动作指令".to_string(),
    )
    .await?;

    // 通知客户端让本体交出控制权
    let delivered = gateway::send_command_to_device(
        &state.device_connections,
        &device.device_id,
        "远程接管",
        Some(json!({ "operator": operator })),
    )
    .await;

    info!(
        "device {} teleop taken over by {}",
        device.device_id, operator
    );
    Ok(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "holder": operator,
        "expiresInSec": TELEOP_IDLE_TIMEOUT_SECS,
        "wsDelivered": delivered,
    })))
}

/// POST /api/devices/{id}/teleop/release —— 释放接管
pub async fn release(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let device = load_device(&state, id).await?;
    let operator = current_operator(&state, &device).await;
    if device.teleop_holder.is_none() {
        return Ok(ApiResponse::ok(json!({
            "deviceId": device.device_id,
            "released": false,
            "note": "设备当前无人接管",
        })));
    }

    write_teleop(&state, id, None).await?;
    log_event(
        &state,
        &device,
        format!("{} 释放 {}", operator, device.device_id),
        "远程接管已释放".to_string(),
    )
    .await?;

    // 释放时先补一帧停车，避免摇杆还压着时设备继续动
    let _ = gateway::send_teleop_to_device(
        &state.device_connections,
        &device.device_id,
        &TeleopRequest {
            forward: 0.0,
            strafe: 0.0,
            turn: 0.0,
            speed: None,
        },
    )
    .await;
    let delivered = gateway::send_command_to_device(
        &state.device_connections,
        &device.device_id,
        "释放接管",
        None,
    )
    .await;

    info!(
        "device {} teleop released by {}",
        device.device_id, operator
    );
    Ok(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "released": true,
        "wsDelivered": delivered,
    })))
}

/// POST /api/devices/{id}/teleop/ping —— 保活，刷新接管时间
pub async fn keepalive(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let device = load_device(&state, id).await?;
    let operator = current_operator(&state, &device).await;
    ensure_holding(&device, &operator)?;
    write_teleop(&state, id, Some(&operator)).await?;
    Ok(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "expiresInSec": TELEOP_IDLE_TIMEOUT_SECS,
    })))
}

/// POST /api/devices/{id}/teleop —— 下发摇杆
pub async fn send_teleop(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<TeleopRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let device = load_device(&state, id).await?;
    let operator = current_operator(&state, &device).await;
    ensure_holding(&device, &operator)?;

    let axes = payload.clamped();
    write_teleop(&state, id, Some(&operator)).await?;

    let delivered =
        gateway::send_teleop_to_device(&state.device_connections, &device.device_id, &axes).await;

    if !delivered {
        warn!("device {} not connected, teleop dropped", device.device_id);
    }

    Ok(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "forward": axes.forward,
        "strafe": axes.strafe,
        "turn": axes.turn,
        "speed": axes.speed_ratio(),
        "stopped": axes.is_stop(),
        "wsDelivered": delivered,
    })))
}

/// POST /api/devices/{id}/action —— 本体动作（站立/卧倒/急停/灯光/复位）
///
/// 急停是唯一不要求接管态的动作。
pub async fn send_action(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let action = payload["action"]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("缺少 action 字段"))?;

    let device = load_device(&state, id).await?;
    let (code, label) = BODY_ACTIONS
        .iter()
        .find(|(code, _)| *code == action)
        .copied()
        .ok_or_else(|| invalid(format!("不支持的动作 {action}")))?;

    // 急停不受接管态约束：任何人在任何状态下都该能停车
    if code != "emergency_stop" {
        let operator = current_operator(&state, &device).await;
        ensure_holding(&device, &operator)?;
    }

    let delivered = gateway::send_command_to_device(
        &state.device_connections,
        &device.device_id,
        label,
        Some(json!({ "action": code })),
    )
    .await;

    log_event(
        &state,
        &device,
        format!("{} 下发动作：{label}", device.device_id),
        format!("wsDelivered={delivered}"),
    )
    .await?;

    Ok(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "action": code,
        "label": label,
        "wsDelivered": delivered,
    })))
}

/// 校验并规整业务指令名，供 `/command` 复用
pub fn normalize_command(command: &str) -> Result<&'static str, AppError> {
    BUSINESS_COMMANDS
        .iter()
        .find(|known| **known == command)
        .copied()
        .ok_or_else(|| invalid(format!("不支持的指令 {command}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_with(holder: Option<&str>, at: Option<&str>) -> Device {
        Device {
            id: 1,
            device_id: "RD-001".to_string(),
            model: None,
            park_id: None,
            park_name: None,
            zone_id: None,
            zone_name: None,
            status: "在线待命".to_string(),
            battery: 80,
            signal: 4,
            temperature: 30.0,
            runtime_minutes: 0,
            odometer: 0.0,
            position: None,
            current_task_name: None,
            last_heartbeat: None,
            firmware: None,
            device_ip: None,
            video_url: None,
            video_type: "none".to_string(),
            teleop_holder: holder.map(str::to_string),
            teleop_at: at.map(str::to_string),
            body_online: false,
            run_mode: None,
            charging: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn rejects_when_not_taken_over() {
        let device = device_with(None, None);
        assert!(ensure_holding(&device, "operator@RD-001").is_err());
    }

    #[test]
    fn rejects_offline_device() {
        let mut device = device_with(Some("operator@RD-001"), Some(&now()));
        device.status = "离线".to_string();
        assert!(ensure_holding(&device, "operator@RD-001").is_err());
    }

    #[test]
    fn rejects_other_holder() {
        let device = device_with(Some("someone-else"), Some(&now()));
        assert!(ensure_holding(&device, "operator@RD-001").is_err());
    }

    #[test]
    fn accepts_fresh_holder() {
        let device = device_with(Some("operator@RD-001"), Some(&now()));
        assert!(ensure_holding(&device, "operator@RD-001").is_ok());
    }

    #[test]
    fn clamps_axes_into_range() {
        let req = TeleopRequest {
            forward: 3.0,
            strafe: -9.0,
            turn: 0.5,
            speed: Some(5.0),
        }
        .clamped();
        assert_eq!(req.forward, 1.0);
        assert_eq!(req.strafe, -1.0);
        assert_eq!(req.turn, 0.5);
        assert_eq!(req.speed_ratio(), 1.0);
    }

    #[test]
    fn speed_defaults_and_floor() {
        let slow = TeleopRequest {
            forward: 0.0,
            strafe: 0.0,
            turn: 0.0,
            speed: Some(0.0),
        };
        assert_eq!(slow.speed_ratio(), 0.1);
        let default = TeleopRequest {
            forward: 1.0,
            strafe: 0.0,
            turn: 0.0,
            speed: None,
        };
        assert_eq!(default.speed_ratio(), 1.0);
    }

    #[test]
    fn detects_stop() {
        assert!(
            TeleopRequest {
                forward: 0.0,
                strafe: 0.0,
                turn: 0.0,
                speed: None,
            }
            .is_stop()
        );
        assert!(
            !TeleopRequest {
                forward: 0.1,
                strafe: 0.0,
                turn: 0.0,
                speed: None,
            }
            .is_stop()
        );
    }

    #[test]
    fn rejects_unknown_business_command() {
        assert!(normalize_command("起飞").is_err());
        assert_eq!(normalize_command("开始巡检").unwrap(), "开始巡检");
    }
}
