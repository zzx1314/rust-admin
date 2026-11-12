//! 远程接管、本体命令（1302）与遥控摇杆（1304）。
//!
//! 业务侧下发两类遥控消息：
//!
//! - `command`：离散动作。`payload.command` 是中文字面量（与前端按钮一致），
//!   `payload.params.action` 是机器码，两者任一能映射到 [`BodyCommand`] 即可。
//! - `teleop`：连续摇杆。`payload` 是 `{forward, strafe, turn, speed}`（归一化
//!   `[-1, 1]` + 速度档），映射到 1304 的 `lx / ly / rx / ry`。
//!
//! 协议对齐（智航 Pro v1.0.3，见 `business/docs/zhihang-pro-api-v1.0.3.txt`）：
//!
//! - `1302 data.cmd` 是 **uint32 枚举**（站立 10 / 卧倒 12 / 紧急停止 0 …），
//!   不是字符串；现场固件**不回 `1303`**，只能单向下发，执行效果看 1315；
//! - `1303` / `1311` 用 `status_code` + `message` 判受理，不是 `code` / `msg`；
//! - `1304 data` 字段名是 `lx / ly / rx / ry / turn / high_low`，值域 `[-1, 1]`，
//!   而且**没有对应响应类型**（文档里不存在 1305），只能单向发送。
//!
//! 安全约束（与业务侧 `handlers/teleop.rs` 对齐）：
//!
//! 1. 摇杆和动作都必须先接管（拿到 RoamerX 控制权），只有急停放行。
//! 2. 摇杆有看门狗：超过 [`TELEOP_WATCHDOG`] 没收到新帧就自动补一帧停车，
//!    防止网络中断后机器狗按最后的摇杆值一直走。
//! 3. 停止指令（三轴全 0）不要求接管态，任何时候都要能停。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::zhpro::client::ZhProClient;
use crate::zhpro::protocol::{T_COMMAND, T_REMOTE_CONTROL};

/// 摇杆看门狗：多久没收到新帧就自动停车
pub const TELEOP_WATCHDOG: Duration = Duration::from_secs(1);

/// 本体动作（1302 `cmd` 枚举，码值取自协议文档）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyCommand {
    /// 紧急停止
    EmergencyStop,
    /// 解除紧急停止
    ClearEmergencyStop,
    /// 站立
    StandUp,
    /// 卧倒
    LieDown,
    /// 前补光灯开
    LightOn,
    /// 前补光灯关
    LightOff,
}

impl BodyCommand {
    /// 1302 `data.cmd`
    pub fn code(self) -> u32 {
        match self {
            Self::EmergencyStop => 0,
            Self::ClearEmergencyStop => 1,
            Self::StandUp => 10,
            Self::LieDown => 12,
            Self::LightOn => 102,
            Self::LightOff => 104,
        }
    }

    /// 业务侧机器码（`payload.params.action`）
    pub fn action(self) -> &'static str {
        match self {
            Self::EmergencyStop => "emergency_stop",
            Self::ClearEmergencyStop => "clear_emergency_stop",
            Self::StandUp => "stand_up",
            Self::LieDown => "lie_down",
            Self::LightOn => "light_on",
            Self::LightOff => "light_off",
        }
    }

    /// 业务侧中文文案（`payload.command`）
    pub fn label(self) -> &'static str {
        match self {
            Self::EmergencyStop => "急停",
            Self::ClearEmergencyStop => "解除急停",
            Self::StandUp => "站立",
            Self::LieDown => "卧倒",
            Self::LightOn => "开灯",
            Self::LightOff => "关灯",
        }
    }

    /// 由机器码或中文文案解析，两者都支持（业务侧两种形态都下发过）
    pub fn parse(code_or_label: &str) -> Option<Self> {
        [
            Self::EmergencyStop,
            Self::ClearEmergencyStop,
            Self::StandUp,
            Self::LieDown,
            Self::LightOn,
            Self::LightOff,
        ]
        .into_iter()
        .find(|cmd| cmd.action() == code_or_label || cmd.label() == code_or_label)
    }
}

/// 摇杆三轴 + 速度档位（业务侧归一化值）
#[derive(Debug, Clone, Copy, Default)]
pub struct TeleopAxes {
    pub forward: f64,
    pub strafe: f64,
    pub turn: f64,
    pub speed: f64,
}

impl TeleopAxes {
    pub fn from_payload(payload: &Value) -> Self {
        Self {
            forward: payload["forward"].as_f64().unwrap_or(0.0).clamp(-1.0, 1.0),
            strafe: payload["strafe"].as_f64().unwrap_or(0.0).clamp(-1.0, 1.0),
            turn: payload["turn"].as_f64().unwrap_or(0.0).clamp(-1.0, 1.0),
            speed: payload["speed"].as_f64().unwrap_or(1.0).clamp(0.1, 1.0),
        }
    }

    pub fn is_stop(&self) -> bool {
        self.forward == 0.0 && self.strafe == 0.0 && self.turn == 0.0
    }

    /// 转成 1304 载荷。
    ///
    /// 协议字段（全部 `[-1, +1]` 归一化，本体侧自行换算）：
    /// - `lx` 左右移动，**正数左移** → 业务侧 `strafe` 正数是右移，取反；
    /// - `ly` 前后移动，正数前进；
    /// - `rx` 旋转，**正数左旋** → 业务侧 `turn` 正数是右转，取反；
    /// - `ry` 俯仰（运动模式下无作用）；
    /// - `turn` / `high_low` 原地转身与身形，本平台不用，固定 `none`。
    ///
    /// 速度档位仍在此处做二次缩放（`0.1 ~ 1.0`），保证结果不超出值域。
    pub fn to_payload(self) -> Value {
        let v = self.speed;
        json!({
            "lx": round3(-self.strafe * v),
            "ly": round3(self.forward * v),
            "rx": round3(-self.turn * v),
            "ry": 0.0,
            "turn": "none",
            "high_low": "none",
        })
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// 接管态 + 摇杆看门狗
#[derive(Default)]
pub struct TeleopState {
    /// 是否已通过 1204 从本体拿到控制权
    engaged: bool,
    /// 最后一次收到摇杆帧的时间
    last_frame: Option<Instant>,
    /// 已下发过的非零摇杆，看门狗只在有运动时补停车
    moving: bool,
}

pub type SharedTeleop = Arc<Mutex<TeleopState>>;

/// 处理业务侧 `command` 消息里的遥控相关指令
///
/// 返回 `Ok(Some(note))` 表示已处理，`Ok(None)` 表示不是遥控指令（交给上层原有逻辑）。
pub async fn handle_command(
    client: &ZhProClient,
    teleop: &SharedTeleop,
    command: &str,
    params: Option<&Value>,
) -> Option<Result<String, String>> {
    // 业务侧消息形态：{ command, params: { action }, issuedAt }
    // `action` 优先（机器码稳定），取不到再退回中文文案
    let payload_action = params.and_then(|p| {
        p["params"]["action"]
            .as_str()
            .or_else(|| p["action"].as_str())
    });
    let body = payload_action
        .and_then(BodyCommand::parse)
        .or_else(|| BodyCommand::parse(command));

    match command {
        "远程接管" | "开始巡检" => Some(engage(client, teleop).await),
        "释放接管" => Some(disengage(client, teleop).await),
        other => match body {
            Some(BodyCommand::EmergencyStop) => Some(emergency_stop(client, teleop).await),
            Some(cmd) => Some(send_body_command(client, teleop, cmd, other).await),
            None => None,
        },
    }
}

/// 处理 `teleop` 摇杆帧
pub async fn handle_teleop(
    client: &ZhProClient,
    teleop: &SharedTeleop,
    payload: &Value,
) -> Result<(), String> {
    let axes = TeleopAxes::from_payload(payload);
    let mut guard = teleop.lock().await;

    // 停止指令始终放行，运动指令必须已接管
    if !axes.is_stop() && !guard.engaged {
        client.take_control().await?;
        guard.engaged = true;
        info!("摇杆首帧自动接管 RoamerX 控制权");
    }

    client
        .send(T_REMOTE_CONTROL, axes.to_payload())
        .await
        .map_err(|e| format!("摇杆下发失败: {e}"))?;

    guard.last_frame = Some(Instant::now());
    guard.moving = !axes.is_stop();
    Ok(())
}

/// 取得控制权
async fn engage(client: &ZhProClient, teleop: &SharedTeleop) -> Result<String, String> {
    let mut guard = teleop.lock().await;
    if guard.engaged {
        return Ok("已处于接管态".to_string());
    }
    client.take_control().await?;
    guard.engaged = true;
    info!("已接管本体控制权");
    Ok("已接管本体控制权".to_string())
}

/// 释放控制权：先补一帧停车，再交还
async fn disengage(client: &ZhProClient, teleop: &SharedTeleop) -> Result<String, String> {
    let mut guard = teleop.lock().await;
    if !guard.engaged {
        return Ok("当前未接管".to_string());
    }
    // 先停车，避免摇杆还压着就交还控制权
    let _ = client
        .send(T_REMOTE_CONTROL, TeleopAxes::default().to_payload())
        .await;
    guard.engaged = false;
    guard.moving = false;
    guard.last_frame = None;
    // 1304 没有「释放」语义，交还靠 1204 的释放或超时；这里只清本地标志
    info!("已释放本体控制权");
    Ok("已释放接管".to_string())
}

/// 急停：不要求接管态，直接发停车 + 急停命令
async fn emergency_stop(client: &ZhProClient, teleop: &SharedTeleop) -> Result<String, String> {
    // 停车帧先到，再发急停命令，避免命令先到而车还在动
    let _ = client
        .send(T_REMOTE_CONTROL, TeleopAxes::default().to_payload())
        .await;
    {
        let mut guard = teleop.lock().await;
        guard.moving = false;
        guard.last_frame = None;
    }
    send_cmd(client, BodyCommand::EmergencyStop).await?;
    warn!("已下发急停");
    Ok("急停已下发".to_string())
}

/// 下发本体动作（1302）
async fn send_body_command(
    client: &ZhProClient,
    teleop: &SharedTeleop,
    cmd: BodyCommand,
    label: &str,
) -> Result<String, String> {
    if !teleop.lock().await.engaged {
        return Err(format!("{label} 需要先接管"));
    }
    send_cmd(client, cmd).await?;
    info!("本体动作已下发: {}({})", cmd.label(), cmd.code());
    Ok(format!("{label} 已下发"))
}

/// 1302 单条命令
///
/// **现场固件不回 `1303`**（实测：合法 `cmd=102` 与非法 `cmd=999` 都没应答，只有 1315 状态流），
/// 所以只能单向下发。能不能执行看 1315 的 `motion_status` / `fill_light` /
/// `estop` 变化，在控制台日志里可以直接看到。
async fn send_cmd(client: &ZhProClient, cmd: BodyCommand) -> Result<(), String> {
    client
        .send(T_COMMAND, json!({ "cmd": cmd.code() }))
        .await
        .map_err(|e| format!("{} 下发失败: {e}", cmd.label()))?;
    info!(
        "已下发 1302 {}（cmd={}），本固件不回 1303，效果以 1315 状态为准",
        cmd.label(),
        cmd.code()
    );
    Ok(())
}

/// 摇杆看门狗：由主循环定时调用，超过 [`TELEOP_WATCHDOG`] 无新帧则补停车
pub async fn tick_watchdog(client: &ZhProClient, teleop: &SharedTeleop) {
    let mut guard = teleop.lock().await;
    if !guard.moving {
        return;
    }
    let stale = guard
        .last_frame
        .map(|at| at.elapsed() > TELEOP_WATCHDOG)
        .unwrap_or(true);
    if !stale {
        return;
    }
    guard.moving = false;
    guard.last_frame = None;
    drop(guard);
    warn!(
        "摇杆超时（{}s 无新帧），自动停车",
        TELEOP_WATCHDOG.as_secs()
    );
    if let Err(e) = client
        .send(T_REMOTE_CONTROL, TeleopAxes::default().to_payload())
        .await
    {
        warn!("自动停车失败: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_axes_from_payload() {
        let payload = json!({ "forward": 5.0, "strafe": -3.0, "turn": 0.25, "speed": 0.5 });
        let axes = TeleopAxes::from_payload(&payload);
        assert_eq!(axes.forward, 1.0);
        assert_eq!(axes.strafe, -1.0);
        assert_eq!(axes.turn, 0.25);
        assert_eq!(axes.speed, 0.5);
    }

    #[test]
    fn speed_floor_and_default() {
        let slow = TeleopAxes::from_payload(&json!({ "speed": 0.0 }));
        assert_eq!(slow.speed, 0.1);
        let default = TeleopAxes::from_payload(&json!({ "forward": 1.0 }));
        assert_eq!(default.speed, 1.0);
    }

    #[test]
    fn stop_payload_is_all_zero() {
        let value = TeleopAxes::default().to_payload();
        assert_eq!(value["lx"], json!(0.0));
        assert_eq!(value["ly"], json!(0.0));
        assert_eq!(value["rx"], json!(0.0));
        assert_eq!(value["ry"], json!(0.0));
        assert_eq!(value["turn"], json!("none"));
        assert_eq!(value["high_low"], json!("none"));
    }

    /// 业务侧轴语义（前进/右移/右转为正）到协议轴语义（左移/左旋为正）的换算
    #[test]
    fn maps_axes_to_protocol_fields() {
        let value = TeleopAxes {
            forward: 1.0,
            strafe: 1.0,
            turn: 1.0,
            speed: 1.0,
        }
        .to_payload();
        assert_eq!(value["ly"], json!(1.0));
        assert_eq!(value["lx"], json!(-1.0));
        assert_eq!(value["rx"], json!(-1.0));

        let left = TeleopAxes {
            strafe: -0.5,
            turn: -0.5,
            speed: 1.0,
            ..Default::default()
        }
        .to_payload();
        assert_eq!(left["lx"], json!(0.5));
        assert_eq!(left["rx"], json!(0.5));
    }

    #[test]
    fn scales_by_speed_ratio() {
        let value = TeleopAxes {
            forward: 0.8,
            strafe: 0.0,
            turn: 0.0,
            speed: 0.5,
        }
        .to_payload();
        assert_eq!(value["ly"], json!(0.4));
    }

    #[test]
    fn detects_stop_frame() {
        assert!(TeleopAxes::default().is_stop());
        assert!(
            !TeleopAxes {
                turn: 0.01,
                ..Default::default()
            }
            .is_stop()
        );
    }

    /// 1302 的 cmd 必须是文档里的 uint32 枚举，发字符串本体不认
    #[test]
    fn body_command_codes_match_protocol() {
        assert_eq!(BodyCommand::StandUp.code(), 10);
        assert_eq!(BodyCommand::LieDown.code(), 12);
        assert_eq!(BodyCommand::EmergencyStop.code(), 0);
        assert_eq!(BodyCommand::ClearEmergencyStop.code(), 1);
        assert_eq!(BodyCommand::LightOn.code(), 102);
        assert_eq!(BodyCommand::LightOff.code(), 104);
    }

    #[test]
    fn parses_action_and_label() {
        assert_eq!(BodyCommand::parse("stand_up"), Some(BodyCommand::StandUp));
        assert_eq!(BodyCommand::parse("站立"), Some(BodyCommand::StandUp));
        assert_eq!(BodyCommand::parse("卧倒"), Some(BodyCommand::LieDown));
        assert_eq!(BodyCommand::parse("急停"), Some(BodyCommand::EmergencyStop));
        // 协议里没有「位姿复位」，不能静默映射成别的动作
        assert_eq!(BodyCommand::parse("reset_pose"), None);
        assert_eq!(BodyCommand::parse("位姿复位"), None);
    }
}
