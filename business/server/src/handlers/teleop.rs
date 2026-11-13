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
/// `id` 是下发给本体的动作机码（不是 1302 的 `cmd` 码值，码值在设备客户端
/// `client/src/zhpro/teleop.rs::BodyCommand` 里按协议文档映射），`label` 用于审计
/// 日志与前端提示。
///
/// 协议 1302 的枚举里没有「位姿复位」，所以不再对外暴露这个动作
/// （以前下发的字符串 `reset_pose` 本体根本不认）。
const BODY_ACTIONS: [(&str, &str); 11] = [
    ("stand_up", "站立"),
    ("lie_down", "卧倒"),
    ("emergency_stop", "急停"),
    // 1302 cmd=1：清 `estop.software`。软件急停没解除时，本体会把运动/导航指令全部拒掉，
    // 而现场（X30-Pro）没有物理急停按钮，只能靠这条命令解除。
    ("clear_emergency_stop", "解除急停"),
    ("light_on", "开灯"),
    ("light_off", "关灯"),
    // 模式切换（1302 cmd 60/61/62）：1304 摇杆的 lx/ly 只在通用模式下生效，
    // 设备卡在原地模式时用它们手动纠正；接管时客户端也会自动切通用模式。
    ("generic_mode", "通用模式"),
    ("in_place_mode", "原地模式"),
    ("navigation_mode", "导航模式"),
    // 1302 cmd=110/111：避障开关。感知降级（12608 相机无图 / 13382 检测延迟）时，
    // 规划器会一直报「无有效绕障轨迹，终点附近有障碍物，等待中（13380）」
    ("avoid_on", "避障开启"),
    ("avoid_off", "避障关闭"),
];

/// 允许通过 `/command` 下发的业务指令
const BUSINESS_COMMANDS: [&str; 9] = [
    "开始巡检",
    "暂停任务",
    "继续任务",
    "召回充电",
    "同步地图",
    "重新定位",
    "远程接管",
    "释放接管",
    // 「本体重连」：让设备客户端退出并由 systemd 重建到本体的连接。
    // 它是运维手段（链路/会话半死时用），不改本体任何状态。
    "本体重连",
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

/// 本体健康门禁：与客户端 `RobotState::motion_block_reason` 对齐。
///
/// 纯模拟设备没有本体，跳过这层校验（前端联调用）。
///
/// `allow_ctrl_preempt` 专门对应「遥控器关了但本体还在报 `ctrl_source = 3`」：
/// 运维确认遥控器已断电后可强制接管（`teleop_force`），这里放行这一条门禁，
/// 其余（离线 / `sdk_node` / 急停）一条不少。
pub(crate) fn ensure_ready(device: &Device) -> Result<(), AppError> {
    ensure_ready_with(device, device.teleop_force)
}

fn ensure_ready_with(device: &Device, allow_ctrl_preempt: bool) -> Result<(), AppError> {
    if device.status == "离线" {
        return Err(invalid("设备离线，无法远程控制"));
    }
    if device.run_mode.as_deref() == Some("sim") {
        return Ok(());
    }
    if !device.body_online {
        return Err(invalid(
            "本体未连接（body_online=false，1315 状态超时），无法远程控制",
        ));
    }
    if device.sdk_node_available == Some(false) {
        return Err(invalid(
            "本体 SDK 节点未就绪（sdk_node=false），请等待初始化或重启本体算法节点",
        ));
    }
    if device.estop_software == Some(true) {
        return Err(invalid("本体软件急停已触发，请先点「解除急停」"));
    }
    if device.ctrl_source == 3 && !allow_ctrl_preempt {
        return Err(invalid(
            "本体控制权被其他遥控器/外部程序抢占，请断开外部遥控后重试\
             （确认遥控器已断电？可用「强制接管」）",
        ));
    }
    Ok(())
}

fn ensure_holding(device: &Device, operator: &str) -> Result<(), AppError> {
    ensure_ready(device)?;
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
///
/// 外部（如任务下发）也要判断接管态，不能让「接管中」的设备去跑自动任务。
pub(crate) fn teleop_expired(at: Option<&str>) -> bool {
    let Some(at) = at else {
        return true;
    };
    match parse_time(at) {
        Some(at_secs) => now_secs() - at_secs > TELEOP_IDLE_TIMEOUT_SECS,
        // 时间解析不了就当作已超时，宁可让用户重新接管也不放开控制
        None => true,
    }
}

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

async fn write_teleop(
    state: &AppState,
    id: i64,
    holder: Option<&str>,
    force: bool,
) -> Result<(), AppError> {
    let stamp = now();
    sqlx::query(
        "UPDATE devices SET teleop_holder = ?1, teleop_at = ?2, teleop_force = ?3 WHERE id = ?4",
    )
    .bind(holder)
    .bind(holder.map(|_| stamp.clone()))
    .bind(if holder.is_some() && force { 1 } else { 0 })
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
///
/// 可选 body `{"force": true}`：遥控器断电后本体仍报 `ctrl_source = 3` 时，
/// 运维二次确认后可强制接管（落库 `teleop_force`，并告知客户端跳过这条门禁）。
pub async fn take_over(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Option<Json<serde_json::Value>>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let device = load_device(&state, id).await?;
    let force = body
        .as_ref()
        .and_then(|Json(value)| value.get("force"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    // 接管同样要求本体健康：sdk_node 未就绪时拿到 1204 也动不了，
    // 提前给出可读原因，避免页面上“已接管但摇杆没反应”。
    ensure_ready_with(&device, device.teleop_force || force)?;
    let operator = current_operator(&state, &device).await;

    if let Some(holder) = device.teleop_holder.as_deref()
        && holder != operator
        && !is_teleop_expired(&device)
    {
        return Err(invalid(format!("设备已被 {holder} 接管")));
    }

    // 只在真的越过了 `ctrl_source = 3` 时才保留强制标记；常规路径必须清零，
    // 否则一次强制接管会让这台设备以后永久免疫遥控器抢占检测。
    let force = force && device.ctrl_source == 3;
    write_teleop(&state, id, Some(&operator), force).await?;
    log_event(
        &state,
        &device,
        format!("{} 接管 {}", operator, device.device_id),
        if force {
            format!(
                "远程接管已建立（强制接管：本体仍报 ctrl_source={}，已跳过遥控器抢占门禁）",
                device.ctrl_source
            )
        } else {
            "远程接管已建立，可下发摇杆与动作指令".to_string()
        },
    )
    .await?;

    // 通知客户端让本体交出控制权；`force` 让客户端也跳过 ctrl_source 门禁，
    // 否则 1304 摇杆会被客户端的门禁拦下（服务端放行也没用）。
    let delivered = gateway::send_command_to_device(
        &state.device_connections,
        &device.device_id,
        "远程接管",
        Some(json!({ "operator": operator, "force": force })),
    )
    .await;

    info!(
        "device {} teleop taken over by {} (force={force})",
        device.device_id, operator
    );
    Ok(ApiResponse::ok(json!({
        "deviceId": device.device_id,
        "holder": operator,
        "expiresInSec": TELEOP_IDLE_TIMEOUT_SECS,
        "wsDelivered": delivered,
        "forced": force,
        "ctrlSource": device.ctrl_source,
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

    write_teleop(&state, id, None, false).await?;
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
    // 保活只刷新时间，保留原有的强制接管标记（否则强制接管会在 2 分钟后自动失效）
    write_teleop(&state, id, Some(&operator), device.teleop_force).await?;
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
    write_teleop(&state, id, Some(&operator), device.teleop_force).await?;

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

    // 恢复类动作不受接管态约束：
    // - 急停：任何人在任何状态下都该能停车；
    // - 解除急停 / 避障开关：接管会把本体切到 1302 cmd=60 通用（手动）模式，
    //   而手动模式下 `5000` 会被回 13897，所以这几个按钮必须能在非接管态点。
    if !matches!(
        code,
        "emergency_stop" | "clear_emergency_stop" | "avoid_on" | "avoid_off"
    ) {
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
            teleop_force: false,
            body_online: true,
            run_mode: None,
            charging: false,
            robot_mode: None,
            loc_status: 255,
            active_map_id: None,
            ctrl_source: 0,
            motion_status: None,
            estop_software: None,
            sdk_node_available: None,
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
    fn rejects_when_body_offline() {
        let mut device = device_with(Some("operator@RD-001"), Some(&now()));
        device.body_online = false;
        assert!(ensure_holding(&device, "operator@RD-001").is_err());
    }

    #[test]
    fn rejects_when_sdk_node_not_ready() {
        let mut device = device_with(Some("operator@RD-001"), Some(&now()));
        device.sdk_node_available = Some(false);
        assert!(ensure_holding(&device, "operator@RD-001").is_err());
    }

    #[test]
    fn rejects_when_software_estop_active() {
        let mut device = device_with(Some("operator@RD-001"), Some(&now()));
        device.estop_software = Some(true);
        assert!(ensure_holding(&device, "operator@RD-001").is_err());
    }

    #[test]
    fn rejects_when_control_seized_by_remote() {
        let mut device = device_with(Some("operator@RD-001"), Some(&now()));
        device.ctrl_source = 3;
        assert!(ensure_holding(&device, "operator@RD-001").is_err());
    }

    /// 遥控器断电、本体还报 `ctrl_source = 3` 时，强制接管必须能放行摇杆
    #[test]
    fn force_takeover_bypasses_remote_preempt() {
        let mut device = device_with(Some("operator@RD-001"), Some(&now()));
        device.ctrl_source = 3;
        device.teleop_force = true;
        assert!(ensure_holding(&device, "operator@RD-001").is_ok());
    }

    /// 强制接管只能跳过 ctrl_source，其余健康门禁必须照旧生效
    #[test]
    fn force_takeover_still_checks_other_gates() {
        let mut device = device_with(Some("operator@RD-001"), Some(&now()));
        device.ctrl_source = 3;
        device.teleop_force = true;

        let mut estop = device.clone();
        estop.estop_software = Some(true);
        assert!(ensure_holding(&estop, "operator@RD-001").is_err());

        let mut offline = device.clone();
        offline.body_online = false;
        assert!(ensure_holding(&offline, "operator@RD-001").is_err());

        let mut no_sdk = device.clone();
        no_sdk.sdk_node_available = Some(false);
        assert!(ensure_holding(&no_sdk, "operator@RD-001").is_err());
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

    /// 协议 1302 里没有「位姿复位」，这个动作不应该再被受理
    #[test]
    fn drops_unsupported_body_action() {
        assert!(!BODY_ACTIONS.iter().any(|(code, _)| *code == "reset_pose"));
        assert!(BODY_ACTIONS.iter().any(|(code, _)| *code == "stand_up"));
    }
}
