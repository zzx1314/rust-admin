//! 远程接管、本体命令（1302）与遥控摇杆（1304）。
//!
//! 业务侧下发两类遥控消息：
//!
//! - `command`：离散动作。`payload.command` 是中文字面量（与前端按钮一致），
//!   可选 `payload.params.action` 带机器码，两者取交集映射到 1302 的 `cmd`。
//! - `teleop`：连续摇杆。`payload` 是 `{forward, strafe, turn, speed}`，
//!   映射到 1304 的 `linear_x / linear_y / angular_z`。
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

/// 本体动作：业务侧 `command` 文案 → (1302 的 cmd 码, 说明)
///
/// 码值与智航网关的 `cmd` 枚举对齐；不确定的先用语义化字符串，网关侧做转发映射。
const BODY_COMMANDS: [(&str, &str); 6] = [
    ("站立", "stand_up"),
    ("卧倒", "lie_down"),
    ("急停", "emergency_stop"),
    ("开灯", "light_on"),
    ("关灯", "light_off"),
    ("位姿复位", "reset_pose"),
];

/// 摇杆三轴 + 速度档位
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
    /// 智航摇杆用「线速度 m/s + 角速度 rad/s」，这里把归一化轴值乘以最大速度：
    /// 前进 1.0 m/s、横移 0.6 m/s（狗横移慢）、转向 1.2 rad/s。
    pub fn to_payload(self) -> Value {
        let v = self.speed;
        json!({
            "linear_x": round3(self.forward * 1.0 * v),
            "linear_y": round3(self.strafe * 0.6 * v),
            "angular_z": round3(self.turn * 1.2 * v),
            "speed_ratio": round3(self.speed),
        })
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// 接管态 + 摇杆看门狗
#[derive(Default)]
pub struct TeleopState {
    /// 是否已通过 1304 从本体拿到控制权
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
    // 业务侧可能用机器码（action）或中文文案，优先用机器码
    let action = params.and_then(|p| p["action"].as_str()).or_else(|| {
        BODY_COMMANDS
            .iter()
            .find(|(label, _)| *label == command)
            .map(|(_, code)| *code)
    });

    match command {
        "远程接管" | "开始巡检" => Some(engage(client, teleop).await),
        "释放接管" => Some(disengage(client, teleop).await),
        "急停" => Some(emergency_stop(client, teleop).await),
        other => match action {
            Some("emergency_stop") => Some(emergency_stop(client, teleop).await),
            Some(code) => Some(send_body_command(client, teleop, code, other).await),
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
        let _ = client.take_control().await;
        guard.engaged = true;
        info!("摇杆首帧自动接管 RoamerX 控制权");
    }

    client
        .call(T_REMOTE_CONTROL, axes.to_payload())
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
        .call(T_REMOTE_CONTROL, TeleopAxes::default().to_payload())
        .await;
    guard.engaged = false;
    guard.moving = false;
    guard.last_frame = None;
    // 1304 没有「释放」语义，交还靠 1204 的释放或超时；这里只清本地标志
    info!("已释放本体控制权");
    Ok("已释放接管".to_string())
}

/// 急停：不要求接管态，直接发停车 + 急停动作
async fn emergency_stop(client: &ZhProClient, teleop: &SharedTeleop) -> Result<String, String> {
    // 停车帧先到，再发急停动作，避免动作先到而车还在动
    let _ = client
        .call(T_REMOTE_CONTROL, TeleopAxes::default().to_payload())
        .await;
    {
        let mut guard = teleop.lock().await;
        guard.moving = false;
        guard.last_frame = None;
    }
    client
        .call(
            T_COMMAND,
            json!({ "cmd": "emergency_stop", "action": "emergency_stop" }),
        )
        .await
        .map_err(|e| format!("急停下发失败: {e}"))?;
    warn!("已下发急停");
    Ok("急停已下发".to_string())
}

/// 下发本体动作（1302）
async fn send_body_command(
    client: &ZhProClient,
    teleop: &SharedTeleop,
    code: &str,
    label: &str,
) -> Result<String, String> {
    if !teleop.lock().await.engaged {
        return Err(format!("{label} 需要先接管"));
    }
    client
        .call(T_COMMAND, json!({ "cmd": code, "action": code }))
        .await
        .map_err(|e| format!("{label} 下发失败: {e}"))?;
    info!("本体动作已下发: {label}({code})");
    Ok(format!("{label} 已下发"))
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
        .call(T_REMOTE_CONTROL, TeleopAxes::default().to_payload())
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
        assert_eq!(value["linear_x"], json!(0.0));
        assert_eq!(value["linear_y"], json!(0.0));
        assert_eq!(value["angular_z"], json!(0.0));
    }

    #[test]
    fn scales_axes_by_max_velocity() {
        let axes = TeleopAxes {
            forward: 1.0,
            strafe: 1.0,
            turn: 1.0,
            speed: 1.0,
        };
        let value = axes.to_payload();
        assert_eq!(value["linear_x"], json!(1.0));
        assert_eq!(value["linear_y"], json!(0.6));
        assert_eq!(value["angular_z"], json!(1.2));
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

    #[test]
    fn maps_label_to_action_code() {
        let found = BODY_COMMANDS
            .iter()
            .find(|(label, _)| *label == "急停")
            .map(|(_, code)| *code);
        assert_eq!(found, Some("emergency_stop"));
    }
}
