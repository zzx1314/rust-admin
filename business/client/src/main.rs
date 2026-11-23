//! robodog-device-client
//!
//! 四足机器人（设备端）运行时：把业务服务下发的任务，翻译成智航 Pro 本体协议并执行。
//!
//! ```text
//!   业务服务 (robodog-ops-server)                 机器人本体 (智航 Pro)
//!   ws://server:3100/ws/device                   ws://192.168.168.100:10010/control
//!        │  task_dispatch / task_control              │  5000 建图/导航、5014 返充
//!        │  ← task_progress / event / heartbeat        │  5009 状态、4001 保存结果、4013 地图列表
//!        ▼                                             ▼
//!   ┌───────────────────────────────────────────────────────┐
//!   │  设备客户端：任务编排（建图 / 航线 / 回充）+ 进度回传   │
//!   └───────────────────────────────────────────────────────┘
//! ```
//!
//! # 运行
//!
//! 内置默认值就是正式环境值：业务服务 `ws://39.107.55.13:3100`、本体 `192.168.234.234:10010`。
//! **设备编号不用配**：启动时自动读本体 SN 分区 `/dev/mmcblk0p5`
//! （等价 `hexdump -C /dev/mmcblk0p5 | sed -n '3s/.*|\(.*\)|.*/\1/p' | tr -d ' '`）。
//!
//! ```bash
//! # 现场：连业务服务 + 机器狗本体
//! cargo run --
//!
//! # 纯模拟模式（不接本体，便于前端联调）
//! cargo run -- --robot sim
//!
//! # 只有本体、没有业务服务时，本地跑一次建图
//! cargo run -- --robot 192.168.234.234 --map-only --map-name A区新图
//! ```

mod agent_relay;
mod ai_patrol;
mod dispatch;
mod lights;
mod mapupload;
mod media_sync;
/// 联调用「假本体」：`--robot mock` 时在进程内起一个智航 Pro 网关模拟器
mod mock_body;
mod zhpro;

use agent_relay::AgentRelay;
use ai_patrol::{
    AiPatrol, AiPoint, AiStart, ai_point_at, is_arrival_stage, is_point_stage, parse_ai_points,
};
use chrono::Local;
use dispatch::{TaskMode, parse_flight_plan, parse_map_binding};
use futures_util::{SinkExt, StreamExt};
use lights::{LightPoint, PointLights, light_point_at, parse_light_points};
use mapupload::{IngestConfig, UploadOutcome};
use rand::Rng;
use robodog_device_client::config::{CliArgs, Config, EnvConfig, RoadNetworkMode, print_help};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use tokio::time::interval;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};
use zhpro::client::ZhProClient;
use zhpro::mapping::{MapManager, MappingSaved, MappingSession};
use zhpro::protocol::*;
use zhpro::route::{Control, RouteRunner};
use zhpro::status::{StatusFrame, TaskMonitor, TaskSnapshot};
use zhpro::teleop::{self, SharedTeleop, TeleopState};

// ─── 业务服务协议（与 server/src/gateway_types.rs 一致） ────────

/// WebSocket 消息信封
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WsMessage {
    msg_type: String,
    ts: String,
    payload: Value,
}

/// 心跳载荷
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HeartbeatPayload {
    battery: i64,
    temperature: f64,
    position: String,
    odometer: f64,
    runtime_minutes: i64,
    current_task: Option<String>,
    firmware: String,
    /// 本体型号（智航 1201 `type`），未知时不报
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    /// 本体链路是否在线：false 时电量/温度等数值不可信（本服务据此不再当真实数据展示）
    body_online: bool,
    /// 客户端工作模式：robot（接本体）/ sim（纯模拟）
    mode: String,
    /// 充电状态：本体 1315 `charging_pile_connect` / 电池 `power_supply_status`
    #[serde(skip_serializing_if = "Option::is_none")]
    charging: Option<bool>,
    /// 本体工作模式（1315 `mode` / MachineStatus，如 `remote`、`auto`）
    #[serde(skip_serializing_if = "Option::is_none")]
    robot_mode: Option<String>,
    /// 定位状态（智航 loc_state.status）
    #[serde(skip_serializing_if = "Option::is_none")]
    loc_status: Option<i64>,
    /// 机器人当前生效地图
    #[serde(skip_serializing_if = "Option::is_none")]
    map_id: Option<String>,
    /// 1315 `ctrl_source`：0 未知 / 1 APP / 2 SDK / 3 其他（含硬件遥控器）
    ///
    /// 平台据此判断「到底谁在控制」：拿到 1204 但 `ctrl_source=3` 说明遥控器还压着。
    #[serde(skip_serializing_if = "Option::is_none")]
    ctrl_source: Option<i64>,
    /// 1315 `motion_status`（stand_up / lie_down / walk …）：卧倒时摇杆走不动
    #[serde(skip_serializing_if = "Option::is_none")]
    motion_status: Option<String>,
    /// 1315 `estop.software`：软件急停是否按下；平台据此禁止遥控/任务
    #[serde(skip_serializing_if = "Option::is_none")]
    estop_software: Option<bool>,
    /// 1207 模块可用性：本体 SDK 节点是否就绪；false 时所有运动/导航指令都会被拒
    #[serde(skip_serializing_if = "Option::is_none")]
    sdk_node_available: Option<bool>,
}

/// 任务进度上报
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskProgressPayload {
    task_code: String,
    status: String,
    progress: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    point_index: Option<i64>,
    /// 当前执行航点的 **0 基下标**（5009 的 `detail_index`）。
    ///
    /// 必须和 `point_index` 分开：`point_index` 是「已完成航点数」（1 基计数），
    /// 定时摘要里它等于「当前点 + 1」，平台拿它开 AI 窗口会查到下一个航点。
    #[serde(skip_serializing_if = "Option::is_none")]
    waypoint_index: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    /// 智航 5009 的阶段串，便于前端展示执行细节
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
    /// 智航侧任务 ID（5001 返回），便于与机器人日志对账
    #[serde(skip_serializing_if = "Option::is_none")]
    robot_task_id: Option<String>,
    /// 航点总数，前端据此展示「已完成 / 总数」
    #[serde(skip_serializing_if = "Option::is_none")]
    total_points: Option<i64>,
}

/// 设备事件
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceEventPayload {
    event_type: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

// ─── 机器人运行时状态 ──────────────────────────────────────────

/// 设备运行时状态：数值优先取本体 1315 上报，否则按任务推演
struct RobotState {
    battery: i64,
    /// 双电池里**较低**的那一节（`power1/power2` 取 min）——平台展示用 max，
    /// 但低电保护/巡检阈值必须按较低的那节算（ALG 就是按这个值判低电的）
    battery_min: i64,
    temperature: f64,
    position: String,
    runtime_minutes: i64,
    odometer: f64,
    current_task: Option<String>,
    firmware: String,
    /// 本体型号（智航 1201 `type`），未知时为空
    model: Option<String>,
    loc_status: i64,
    loc_fresh: bool,
    /// 1315 `ctrl_source`：谁在控制本体（遥控器 / APP / SDK）
    ctrl_source: Option<i64>,
    active_map: Option<String>,
    /// 最近一次收到本体 1315 的时刻：超过 `BODY_STATUS_FRESH` 视为本体掉线
    body_status_at: Option<Instant>,
    /// 工作模式：robot（接本体）/ sim（纯模拟）
    mode: &'static str,
    /// 充电状态：来自 1315（充电桩接入 / 电池电源状态），模拟模式恒 false
    charging: bool,
    /// 本体工作模式：1315 `mode`（协议里的 MachineStatus），如 remote / auto
    robot_mode: Option<String>,
    /// 运动状态：1315 `motion_status`（stand_up / balance_stand / lie_down …）
    ///
    /// 固件不回 `1303`，所以站立 / 卧倒是否生效只能看这个字段的变化
    motion_status: Option<String>,
    /// 软件急停是否处于按下态：1315 `estop.software`
    estop_software: Option<bool>,
    /// 1207 `modules[]` 里的 `sdk_node` 是否可用；false 时本体不会执行运动/导航
    sdk_node_available: Option<bool>,
    /// 前补光灯开关：1315 `fill_light.front`
    fill_light_front: Option<bool>,
    /// 最近一次「摇杆被门禁拒绝」的原因与上报时刻，用于限频（否则 10Hz 摇杆会刷爆事件流）
    teleop_block_notice: Option<(String, Instant)>,
    /// 强制接管标记：平台二次确认后忽略 `ctrl_source = 3`（遥控器占用）这一条门禁
    ///
    /// 现场：遥控器关了但本体还在报 `ctrl_source = 3`，常规门禁会把摇杆全部拦掉。
    /// 这个开关只放行 ctrl_source 一条，其余（离线 / sdk_node / 急停 / 卧倒）照旧。
    teleop_force: bool,
}

/// 本体状态新鲜窗口：1315 是 1Hz，30s 收不到就认为本体链路已不可信
const BODY_STATUS_FRESH: Duration = Duration::from_secs(30);

/// 「摇杆被门禁拒绝」上报平台的限频窗口（原因相同则最多这么久一条）
const TELEOP_BLOCK_NOTICE_INTERVAL: Duration = Duration::from_secs(30);

/// 本体断连后的**进程内**重连参数
///
/// 现场「本体重启」要 40~60s 才重新监听 10010，所以窗口给到 2 分钟；超过窗口
/// 就不再自己试，退出进程交给 systemd（`Restart=always`）作最后兜底。
const BODY_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const BODY_RECONNECT_WINDOW: Duration = Duration::from_secs(120);
const BODY_RECONNECT_MIN_BACKOFF: Duration = Duration::from_secs(1);
const BODY_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(10);

/// 链路「刚挂上就被踢」的抖动保护窗口
///
/// 重连成功但与本体只活了不到这个时间（网关还留着旧会话、重复 client 竞争）时，
/// 下一轮重连先等够窗口再动手，避免无间隔连打网关。
const BODY_RECONNECT_SETTLE: Duration = Duration::from_secs(5);

/// 下一次重连退避：翻倍并封顶（1s → 2s → 4s → 8s → 10s…）
/// 一次连接「活过这么久」就认为平台侧是好的（重启 / 发布 / 抖动），
/// 下次断开的退避从 1s 重新开始，别一路飙到 30s。
const HEALTHY_CONNECTION: Duration = Duration::from_secs(30);

/// 业务连接重连次数：连接稳定存活过 [`HEALTHY_CONNECTION`] 就重新从 1 开始退避。
fn next_retry(previous: u64, elapsed: Duration) -> u64 {
    if elapsed >= HEALTHY_CONNECTION {
        1
    } else {
        previous + 1
    }
}

fn next_body_backoff(current: Duration) -> Duration {
    (current * 2).min(BODY_RECONNECT_MAX_BACKOFF)
}

impl RobotState {
    fn new(config: &Config) -> Self {
        Self {
            battery: config.battery,
            temperature: 32.0,
            position: config.position.clone(),
            runtime_minutes: 0,
            odometer: 0.0,
            current_task: None,
            firmware: config.firmware.clone(),
            model: None,
            loc_status: 255,
            loc_fresh: false,
            ctrl_source: None,
            active_map: None,
            body_status_at: None,
            mode: "sim",
            charging: false,
            robot_mode: None,
            motion_status: None,
            battery_min: 100,
            estop_software: None,
            sdk_node_available: None,
            fill_light_front: None,
            teleop_block_notice: None,
            teleop_force: false,
        }
    }

    /// 本体链路是否在线：以 1315 心跳新鲜度为准（TCP 还连着但本体不出数也算掉线）
    fn body_status_fresh(&self) -> bool {
        self.body_status_at
            .map(|at| at.elapsed() < BODY_STATUS_FRESH)
            .unwrap_or(false)
    }

    /// 运动/任务前置条件（对齐官方 App 的 ready 门禁）。
    ///
    /// 官方 App 在 `sdk_node` 未就绪时显示“设备初始化中”，在 `ctrl_source`
    /// 被外部程序/遥控器抢占时暂停遥控，在急停未解除时不允许运动；这里把
    /// 同样的判断收敛到客户端，避免“平台显示已下发、本体实际拒收”。
    fn motion_block_reason(&self, require_posture: bool) -> Option<String> {
        self.motion_block_reason_with(require_posture, false)
    }

    /// 同上，但可选择忽略 `ctrl_source = 3` 这一条
    ///
    /// `allow_ctrl_preempt` 只给强制接管后的手动动作与摇杆用：遥控器断电后本体
    /// 仍可能报 `ctrl_source = 3`，不放行就永远接不回来。任务/导航类指令不进这个开关。
    fn motion_block_reason_with(
        &self,
        require_posture: bool,
        allow_ctrl_preempt: bool,
    ) -> Option<String> {
        if self.body_status_at.is_none() {
            return Some("尚未收到本体 1315 状态（设备初始化中），请稍后重试".to_string());
        }
        if !self.body_status_fresh() {
            return Some("本体 1315 状态超过 30s 未更新（本体离线或算法节点未启动）".to_string());
        }
        if self.sdk_node_available == Some(false) {
            return Some(
                "本体 SDK 节点未就绪（sdk_node=false），请等待初始化或重启本体算法节点".to_string(),
            );
        }
        if self.estop_software == Some(true) {
            return Some("本体软件急停已触发，请先点「解除急停」".to_string());
        }
        if self.ctrl_source == Some(3) && !allow_ctrl_preempt {
            return Some(
                "本体控制权不在平台（ctrl_source=3）：可能是物理遥控器 / 第三方 App，\
                 也可能是本体内部残留任务占着导航能力；请先结束外部控制或重启本体算法服务"
                    .to_string(),
            );
        }
        if require_posture && matches!(self.motion_status.as_deref(), Some("lie_down")) {
            return Some("本体当前为卧倒姿态，请先点「站立」再执行".to_string());
        }
        None
    }

    /// 处理 1315 `kRobotStatus` 本体状态上报。
    ///
    /// 现场实测字段（`data`）：`battery{power1,power2,present1,present2,temperature1,
    /// temperature2,voltage1,current1,power_supply_status1...}`、`temp`、`mile_data`、
    /// `charging_pile_connect`、`mode`、`motion_status`、`speed{}`、`estop{}` 等。
    /// 电量 / 温度 / 里程 / 充电状态全部只从这里取，不做任何推断或随机漂移。
    fn apply_robot_status(&mut self, data: &Value) {
        self.body_status_at = Some(Instant::now());

        if let Some(battery) = data.get("battery") {
            // 双电池取较高的那一节作为展示电量
            let p1 = battery.get("power1").and_then(Value::as_f64).unwrap_or(0.0);
            let p2 = battery.get("power2").and_then(Value::as_f64).unwrap_or(0.0);
            let present2 = battery
                .get("present2")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let power = if present2 { p1.max(p2) } else { p1 };
            let power_min = if present2 { p1.min(p2) } else { p1 };
            self.battery = power.round() as i64;
            self.battery_min = power_min.round() as i64;
            if present2 && (p1 - p2).abs() >= 5.0 {
                // 平台上展示的是较高那节，本体 ALG 用的是可用电量（较低那节），差值大时
                // 会出现「平台 52%、ALG 23%」这种误判，巡检查电量阈值时要用 battery_min
                warn!(
                    "两节电池差异较大：power1={p1:.0}% power2={p2:.0}%，平台展示取较高值 {}%，\
                     按可用电量 {}% 判断低电保护",
                    self.battery, self.battery_min
                );
            }

            // 充电判定：充电桩接入，或电池电源状态为 charging(1) / full(4)。
            // power_supply_status 用的是标准枚举：0=unknown 1=charging 2=discharging
            // 3=not_charging 4=full（现场实测放电时是 2），所以只认 1 和 4。
            let pile = data
                .get("charging_pile_connect")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let supply = [1_i64, 4].iter().any(|status| {
                [
                    battery.get("power_supply_status1"),
                    battery.get("power_supply_status2"),
                ]
                .iter()
                .any(|value| value.and_then(Value::as_i64) == Some(*status))
            });
            self.charging = pile || supply;

            // 温度优先取机体 `temp`，缺省（0）时退回电池温度
            let body_temp = data.get("temp").and_then(Value::as_f64).unwrap_or(0.0);
            let battery_temp = ["temperature1", "temperature2"]
                .iter()
                .filter_map(|key| battery.get(*key).and_then(Value::as_f64))
                .find(|value| *value > 0.0)
                .unwrap_or(0.0);
            let temperature = if body_temp > 0.0 {
                body_temp
            } else {
                battery_temp
            };
            if temperature > 0.0 {
                self.temperature = temperature;
            }
        }

        // 本体工作模式：协议里叫 MachineStatus，网关以字符串 `mode` 下发
        // （现场实测 `remote`）；个别固件给数值枚举，原样保留成字符串由前端展示
        if let Some(mode) = data
            .get("mode")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                data.get("machine_status")
                    .or_else(|| data.get("machineStatus"))
                    .and_then(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .or_else(|| value.as_i64().map(|code| code.to_string()))
                    })
            })
        {
            self.robot_mode = Some(mode);
        }

        // 控制源：0 未知 / 1 APP / 2 SDK / 3 其他（硬件遥控器走这条）
        if let Some(source) = data.get("ctrl_source").and_then(Value::as_i64)
            && self.ctrl_source != Some(source)
        {
            info!("本体控制源变化: ctrl_source={source}");
            self.ctrl_source = Some(source);
        }

        if let Some(mile) = data.get("mile_data").and_then(Value::as_f64) {
            self.odometer = mile;
        }

        // 运动状态 / 急停 / 补光灯：
        //
        // 固件不回 1303（实测），下发 1302 后只能靠这三个字段确认是否真的生效，
        // 所以变化时打一条 info，运维在 journalctl 里就能看到「站立命令到底动没动」。
        if let Some(status) = data
            .get("motion_status")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        {
            if self.motion_status.as_deref() != Some(status.as_str()) {
                info!("本体运动状态: {status}");
            }
            self.motion_status = Some(status);
        }
        if let Some(estop) = data
            .get("estop")
            .and_then(|value| value.get("software"))
            .and_then(Value::as_bool)
        {
            if self.estop_software != Some(estop) {
                if estop {
                    warn!("本体软件急停按下中（estop.software=true）：运动指令会被拒");
                } else {
                    info!("本体软件急停已解除");
                }
            }
            self.estop_software = Some(estop);
        }
        if let Some(front) = data
            .get("fill_light")
            .and_then(|value| value.get("front"))
            .and_then(Value::as_bool)
        {
            if self.fill_light_front != Some(front) {
                info!("前补光灯: {}", if front { "开" } else { "关" });
            }
            self.fill_light_front = Some(front);
        }

        self.runtime_minutes += 1;
    }

    /// 处理 1401 实时位姿（bulkdata）
    fn apply_pose(&mut self, data: &Value) {
        let Some(pose) = data.get("pose") else {
            return;
        };
        let x = pose
            .get("position")
            .and_then(|p| p.get("x"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let y = pose
            .get("position")
            .and_then(|p| p.get("y"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        self.position = format!("{x:.2}, {y:.2}");
    }

    /// 模拟模式下的状态漂移
    fn tick(&mut self, running: bool) {
        let mut rng = rand::thread_rng();
        let drain = if running { 1 } else { 0 };
        self.battery = (self.battery - drain).clamp(0, 100);
        // 信号强度没有真实数据源：保持配置值，不再随机漂移（否则页面显示的是假数据）
        self.temperature = (self.temperature + rng.gen_range(-0.1..0.3)).clamp(20.0, 65.0);
        self.runtime_minutes += 1;
    }

    fn heartbeat(&self) -> HeartbeatPayload {
        HeartbeatPayload {
            battery: self.battery,
            temperature: f64::trunc(self.temperature * 10.0) / 10.0,
            position: self.position.clone(),
            odometer: f64::trunc(self.odometer * 10.0) / 10.0,
            runtime_minutes: self.runtime_minutes,
            current_task: self.current_task.clone(),
            firmware: self.firmware.clone(),
            model: self.model.clone(),
            body_online: self.body_status_fresh(),
            mode: self.mode.to_string(),
            charging: self.body_status_fresh().then_some(self.charging),
            robot_mode: self.robot_mode.clone(),
            loc_status: Some(self.loc_status),
            map_id: self.active_map.clone(),
            ctrl_source: self.ctrl_source,
            motion_status: self.motion_status.clone(),
            estop_software: self.estop_software,
            sdk_node_available: self.sdk_node_available,
        }
    }
}

/// 任务执行上下文：把业务 taskCode 与智航任务绑定
#[derive(Default, Clone)]
struct ActiveTask {
    task_code: String,
    task_name: String,
    nav_task_id: Option<String>,
    stage: Option<String>,
    /// 当前航点下标（5009 `detail_index`，0 基）：定时摘要里单独作为 `waypointIndex` 上报
    point_index: Option<i64>,
    /// 本条路线里带「AI 识别」动作的航点（到点时按 `detail_index` 查）
    ai_points: Vec<AiPoint>,
    /// 本条路线里带「开补光灯 / 开 LED」动作的航点（与 `ai_points` 分开：
    /// 有点只开灯不识别）
    light_points: Vec<LightPoint>,
    /// 计划级「仅夜间自动开灯」（`tasks.light_night_only` → 载荷 `lightNightOnly`）：
    /// 为 true 时把 `nightOnly` 交给板端按本地时间窗判，白天到达不开灯
    light_night_only: bool,
}

type Shared<T> = Arc<Mutex<T>>;

/// 复位本地任务上下文。
///
/// 任务没真正跑起来（航点缺失 / 地图不在本体 / 定位未就绪）时必须调这个：
/// 否则 5s 进度定时器会继续拿这条已失败的任务上报「已暂停 / 等待本体连接」，
/// 把刚发出去的「异常」覆盖掉，平台事件流也会被刷屏。
async fn clear_active_task(active: &Shared<Option<ActiveTask>>, state: &Shared<RobotState>) {
    *active.lock().await = None;
    state.lock().await.current_task = None;
}

/// 日志初始化：RUST_LOG > client.toml `[log].filter` > 默认
fn init_tracing(file_filter: Option<&str>) {
    let fallback = file_filter
        .map(str::to_string)
        .unwrap_or_else(|| "robodog_device_client=info".to_string());
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| fallback.into()),
        )
        .init();
}

#[tokio::main]
async fn main() {
    // 1) 命令行 → 配置文件 → 环境变量三层合并，先于日志初始化拿到 [log].filter
    let cli = match CliArgs::from_env() {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("{error}");
            print_help();
            std::process::exit(1);
        }
    };
    let print_config = cli.print_config;
    let config = match Config::load(cli, EnvConfig::from_env()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };

    init_tracing(config.log_filter.as_deref());

    if print_config {
        println!("{}", config.redacted_report());
        return;
    }

    info!(
        "启动设备客户端: device_id={}, 业务服务={}, 本体={}",
        config.device_id,
        config.server_url,
        config.robot.as_deref().unwrap_or("模拟模式")
    );
    debug!("生效配置（脱敏）:\n{}", config.redacted_report());

    // 先连本体：配了本体就必须连上，连不上直接退出交给 systemd 重启
    // （以前一次失败就永久降级为模拟模式，页面还在报电量，属于假在线）
    let zhpro = match connect_body(&config).await {
        Ok(zhpro) => zhpro,
        Err(error) => {
            // 退出而不是降级成模拟模式：systemd `Restart=always` 会带退避重启，
            // 避免用假电量/假信号冒充真机往业务平台上报
            error!("{error}；进程退出，等待 systemd 重启重试");
            std::process::exit(1);
        }
    };

    if config.map_only {
        let Some(zhpro) = zhpro else {
            eprintln!("--map-only 需要 --robot 参数");
            std::process::exit(1);
        };
        run_mapping_once(&config, zhpro).await;
        return;
    }

    let mut retry = 0_u64;
    loop {
        let started = std::time::Instant::now();
        match run_client(&config, zhpro.clone()).await {
            Ok(()) => {
                info!("收到中断信号，连接正常关闭，进程退出");
                break;
            }
            Err(e) => {
                // 平台重启 / 发布 / 网络抖动：进程不退，原地按 1s → 30s 退避重连。
                // 注意 `active` / `local_runner` 是 `run_client` 内的局部状态，
                // 重连后任务上下文会重建（服务端会把断连期间的任务判异常）。
                retry = next_retry(retry, started.elapsed());
                let delay = retry.min(30);
                warn!("业务连接断开 ({e})，{delay}s 后原地重连（第 {retry} 次）");
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
        }
    }
}

/// 连接智航本体。
///
/// - `robot.host` 为空（`--robot sim`）：显式模拟模式，返回 `None`；
/// - 配了本体：最多重试 `BODY_ATTEMPTS` 次（现场开机竞态：客户端比本体网络先起来），
///   仍失败则返回错误让进程退出 —— systemd `Restart=always` 会带退避重启，
///   绝不带着「模拟数据」冒充真机上报。
async fn connect_body(
    config: &Config,
) -> Result<Option<Arc<ZhProClient>>, Box<dyn std::error::Error + Send + Sync>> {
    const BODY_ATTEMPTS: u32 = 12;
    const BODY_RETRY_SECS: u64 = 5;

    let Some(raw_host) = config.robot.as_deref() else {
        warn!(
            "未接入本体（robot.host 为空或为 sim），以模拟模式运行（任务只回传进度，不驱动本体）"
        );
        return Ok(None);
    };

    // 联调用假本体：进程内起一个 mock 网关，再把地址换成它。
    // 后面走的仍是真机的双通道协议，所以 5000/5001/5009 全部是真实报文。
    let host = if mock_body::is_mock_alias(raw_host) {
        match mock_body::start_from_env().await {
            Ok(addr) => {
                info!("已启动模拟本体（mock）：{addr}（联调模式，非真机）");
                addr
            }
            Err(e) => return Err(format!("启动模拟本体失败: {e}").into()),
        }
    } else {
        raw_host.to_string()
    };
    let host = host.as_str();

    for attempt in 1..=BODY_ATTEMPTS {
        match ZhProClient::connect(host).await {
            Ok(client) => {
                let session = client.session().await;
                info!(
                    "本体连接就绪: client_id={}, 型号={}",
                    session.client_id, session.device_type
                );
                return Ok(Some(Arc::new(client)));
            }
            Err(error) if attempt == BODY_ATTEMPTS => {
                return Err(format!(
                    "连接智航本体 {host} 失败（已重试 {BODY_ATTEMPTS} 次，每次间隔 {BODY_RETRY_SECS}s）: {error}"
                )
                .into());
            }
            Err(error) => {
                warn!(
                    "连接智航本体失败（第 {attempt}/{BODY_ATTEMPTS} 次）: {error}，{BODY_RETRY_SECS}s 后重试"
                );
                tokio::time::sleep(Duration::from_secs(BODY_RETRY_SECS)).await;
            }
        }
    }
    unreachable!("循环必然在最后一次尝试时返回")
}

/// 只跑一次建图（无需业务服务），用于地图标定与联调
async fn run_mapping_once(config: &Config, zhpro: Arc<ZhProClient>) {
    let session = MappingSession::new(zhpro.clone());
    let kind = match config.map_kind.as_str() {
        "indoor" => zhpro::protocol::MappingType::Indoor,
        _ => zhpro::protocol::MappingType::Outdoor,
    };
    match session.start(&config.map_name, kind).await {
        Ok(task_id) => info!("建图已开始: task_id={task_id}，走完后按 Ctrl+C 停止并保存"),
        Err(e) => {
            error!("建图任务创建失败: {e}");
            return;
        }
    }

    let mut rx = zhpro.subscribe();
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        tokio::select! {
            _ = &mut ctrl_c => {
                info!("收到中断信号，停止建图并保存");
                match session.stop(Duration::from_secs(30)).await {
                    Some(saved) if saved.success => {
                        info!("地图保存成功: {}", saved.message);
                        if let Ok(maps) = MapManager::new(zhpro.clone()).list().await {
                            for map in maps {
                                info!(
                                    "地图: {} 路网={} 回充点={}",
                                    map.map_id, map.route_exist, map.charge_point_exist
                                );
                            }
                        }
                        // 手工建图同样支持回传中台
                        match upload_mapping_to_zentai(config, &saved).await {
                            UploadOutcome::Skipped(reason) => {
                                info!("未回传中台: {reason}");
                            }
                            UploadOutcome::Uploaded { map_id } => {
                                info!("已入库中台: {map_id}");
                            }
                            UploadOutcome::Failed { reason } => {
                                error!("回传中台失败: {reason}");
                            }
                        }
                    }
                    Some(saved) => error!("地图保存失败: code={} {}", saved.code, saved.message),
                    None => error!("未收到 4001 保存结果"),
                }
                return;
            }
            env = rx.recv() => {
                let Ok(env) = env else { continue };
                match env.head.msg_type {
                    T_MAPPING_TELEMETRY => {
                        session.on_telemetry(&env.data).await;
                        let tel = session.telemetry().await;
                        if let Some(pose) = tel.pose {
                            info!(
                                "建图位姿 ({:.2}, {:.2}) 里程 {:.1}m 剩余 {:.1}m",
                                pose.position.x, pose.position.y, tel.mileage, tel.remain_mileage
                            );
                        }
                    }
                    T_MAP_SAVED => {
                        if let Some(saved) = session.on_map_saved(&env.data).await {
                            info!("建图收尾: success={} {}", saved.success, saved.message);
                            return;
                        }
                    }
                    T_TASK_STATUS => {
                        if let Some(frame) = StatusFrame::parse(&env.data)
                            && let Some(task) = &frame.current
                            && task.task_type == "mapping"
                        {
                            info!("建图阶段: {} ({})", task.stage, task.lifecycle);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// 本体链路重建时需要重新挂到新任务上的共享句柄（都可 `Clone`）
struct BodyContext {
    config: Arc<Config>,
    ai: Arc<AiPatrol>,
    lights: Arc<PointLights>,
    out_tx: mpsc::Sender<WsMessage>,
    state: Shared<RobotState>,
    active: Shared<Option<ActiveTask>>,
    monitor: Arc<Mutex<TaskMonitor>>,
    local_runner: Shared<Option<Arc<RouteRunner>>>,
    local_mapping: Shared<Option<Arc<MappingSession>>>,
    teleop: SharedTeleop,
}

/// 本体链路的运行期句柄。
///
/// `control` / `bulkdata` 断开后**不退出进程**：在这里原地重建连接，而业务
/// WebSocket（注册、心跳、任务、事件）全程不断 —— 设备不会因为一次网线抖动
/// 就从平台上掉线重登记，恢复窗口从「systemd 重启 + 重新握手」的 5~15s 缩到 1~3s。
struct BodyLink {
    client: Option<Arc<ZhProClient>>,
    pump: Option<tokio::task::JoinHandle<()>>,
    republisher: Option<tokio::task::JoinHandle<()>>,
    /// 链路断开信号（任一通道读到 EOF 或写失败即置 `true`）
    down: Option<tokio::sync::watch::Receiver<bool>>,
    /// 本次链路挂上的时刻，用于识别「刚挂上就被踢」的抖动
    attached_at: Option<Instant>,
}

impl BodyLink {
    fn new(client: Option<Arc<ZhProClient>>) -> Self {
        Self {
            client,
            pump: None,
            republisher: None,
            down: None,
            attached_at: None,
        }
    }

    /// 挂上推送任务与链路监视（首次连接与每次重连各调一次）
    fn attach(&mut self, ctx: &BodyContext, client: Arc<ZhProClient>) {
        self.down = Some(client.body_link());
        self.pump = Some(spawn_zhpro_pump(
            client.clone(),
            ctx.out_tx.clone(),
            ctx.state.clone(),
            ctx.active.clone(),
            ctx.monitor.clone(),
            ctx.local_mapping.clone(),
            ctx.teleop.clone(),
            ctx.config.clone(),
            ctx.ai.clone(),
            ctx.lights.clone(),
        ));
        // 摇杆重发（50Hz）：浏览器 ~10Hz 的离散帧需要在本体侧连续化，否则看着像点动
        self.republisher = Some(teleop::spawn_republisher(
            client.clone(),
            ctx.teleop.clone(),
        ));
        self.attached_at = Some(Instant::now());
        self.client = Some(client);
    }

    /// 摘掉当前链路，返回被摘下的 client（模拟模式 / 已摘下时为 `None`）
    ///
    /// 两个后台任务必须显式 `abort`：它们各自持有旧 `Arc<ZhProClient>`，只换字段
    /// 不杀任务会留下继续往 `out_tx` 塞过期帧的僵尸读循环。
    fn detach(&mut self) -> Option<Arc<ZhProClient>> {
        if let Some(handle) = self.pump.take() {
            handle.abort();
        }
        if let Some(handle) = self.republisher.take() {
            handle.abort();
        }
        self.down = None;
        self.attached_at = None;
        self.client.take()
    }
}

/// 重连前的一次性清理，返回「抖动保护」需要先等的时长
///
/// 必须**快**：它跑在主 select 的关键路径上，期间心跳会暂停 —— 所以耗时的
/// 「建链 + 退避等待」全部交给后台任务 [`connect_body_with_backoff`]，
/// 由主循环在 select 里等结果。这样重连 2 分钟也不会把设备心跳打断
/// （否则服务端 `device_timeout_secs` 会先把设备判离线）。
async fn prepare_body_reconnect(ctx: &BodyContext, body: &mut BodyLink, trigger: &str) -> Duration {
    // 上一轮链路刚挂上就被踢（网关还留着旧会话 / 重复 client 竞争）：先等够窗口
    let settle = body
        .attached_at
        .map(|at| BODY_RECONNECT_SETTLE.saturating_sub(at.elapsed()))
        .unwrap_or_default();
    if !settle.is_zero() {
        warn!("本体链路刚建立就断开，{settle:?} 后再重连（抖动保护）");
    }

    let previous = body.detach();

    // 1) 接管态必然失效：`1204` 是会话级的，重建 control 通道就没了
    //    先补一帧停车，避免最后一帧非零摇杆留在本体侧
    let was_engaged = teleop::is_engaged(&ctx.teleop).await;
    match previous.as_ref() {
        Some(client) => teleop::reset_after_control_lost(client, &ctx.teleop).await,
        None => teleop::reset_local_only(&ctx.teleop).await,
    }
    // 2) 业务 WS 没断，服务端收不到断连回调：必须自己上报 holder 已清空，
    //    否则留下幽灵接管（页面显示接管中、摇杆被客户端门禁拒绝）。
    //    服务端只在 holder 非空时才动手，所以这里无条件发一帧是安全的。
    send(
        &ctx.out_tx,
        "teleop_state",
        json!({
            "holder": false,
            "reason": trigger,
            "wasEngaged": was_engaged,
        }),
    )
    .await;

    // 3) 运行态清理
    if let Some(task) = ctx.active.lock().await.clone() {
        report_progress(
            &ctx.out_tx,
            &task.task_code,
            "异常",
            0,
            Some(format!(
                "本体连接已断开（{trigger}）——客户端正在原地重连本体（不退出进程），请稍后重新下发任务"
            )),
            None,
            None,
            None,
        )
        .await;
    }
    *ctx.active.lock().await = None;
    *ctx.local_runner.lock().await = None;
    if ctx.local_mapping.lock().await.take().is_some() {
        warn!("建图会话随本体断连一起废弃（4001 保存结果不会再来）");
    }
    {
        let mut guard = ctx.state.lock().await;
        guard.current_task = None;
        // 门禁立刻回到「设备初始化中」：断连前的 1315 值不可再用于放行运动指令
        guard.body_status_at = None;
        guard.ctrl_source = None;
        guard.sdk_node_available = None;
        guard.estop_software = None;
        guard.motion_status = None;
    }

    settle
}

/// 后台退避重连：连上就返回新 client，窗口内连不上返回 `Err`
///
/// 与主循环解耦，所以期间心跳、业务指令、事件上报都照常走。
async fn connect_body_with_backoff(
    host: String,
    settle: Duration,
) -> Result<Arc<ZhProClient>, String> {
    if !settle.is_zero() {
        tokio::time::sleep(settle).await;
    }
    let started = Instant::now();
    let mut backoff = BODY_RECONNECT_MIN_BACKOFF;
    let mut attempt = 0_u32;
    let mut last_error = String::new();

    while started.elapsed() < BODY_RECONNECT_WINDOW {
        attempt += 1;
        match tokio::time::timeout(BODY_CONNECT_TIMEOUT, ZhProClient::connect(&host)).await {
            Ok(Ok(client)) => {
                info!(
                    "本体链路已重建（第 {attempt} 次尝试，耗时 {:?}）",
                    started.elapsed()
                );
                return Ok(Arc::new(client));
            }
            Ok(Err(error)) => last_error = error.to_string(),
            Err(_) => {
                last_error = format!("连接超时（>{}s）", BODY_CONNECT_TIMEOUT.as_secs());
            }
        }
        warn!(
            "本体重连失败（第 {attempt} 次）: {last_error}，{}s 后重试",
            backoff.as_secs()
        );
        tokio::time::sleep(backoff).await;
        backoff = next_body_backoff(backoff);
    }

    Err(format!(
        "本体在 {}s 内重连失败（{attempt} 次尝试）：{last_error}",
        BODY_RECONNECT_WINDOW.as_secs()
    ))
}

/// 启动后台重连：先摘链路 + 清运行态（快），再把耗时的建链丢给后台任务
async fn start_relink(
    ctx: &BodyContext,
    body: &mut BodyLink,
    relinking: &mut bool,
    relink_tx: &mpsc::Sender<Result<Arc<ZhProClient>, String>>,
    trigger: &str,
) {
    let settle = prepare_body_reconnect(ctx, body, trigger).await;
    let Some(host) = ctx.config.robot.clone() else {
        // 没有本体配置却收到断连信号（理论上不可达）：只能退回「退出交给 systemd」
        error!("未接入本体（robot.host 为空），无法原地重连");
        std::process::exit(1);
    };
    *relinking = true;
    let tx = relink_tx.clone();
    tokio::spawn(async move {
        let result = connect_body_with_backoff(host, settle).await;
        // 接收端已关闭（进程正在退出）时忽略
        let _ = tx.send(result).await;
    });
}

/// 重连成功：挂上推送任务，并把新会话的型号 / 工作模式同步给平台
async fn finish_relink(ctx: &BodyContext, body: &mut BodyLink, client: Arc<ZhProClient>) {
    let session = client.session().await;
    {
        // 新会话可能报回不同型号（换本体 / 刷固件）：心跳会把变化同步给中台台账
        let mut guard = ctx.state.lock().await;
        if !session.device_type.trim().is_empty() {
            guard.model = Some(session.device_type.clone());
        }
        guard.mode = "robot";
    }
    body.attach(ctx, client);
    info!(
        "本体链路已重建：client_id={}，型号={}（业务连接未中断）",
        session.client_id, session.device_type
    );
    let event = DeviceEventPayload {
        event_type: "info".to_string(),
        message: format!(
            "本体链路已重建（client_id={}），设备在线状态未受影响，需要重新「远程接管」才能遥控",
            session.client_id
        ),
        data: None,
    };
    send(
        &ctx.out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
}

async fn run_client(
    config: &Config,
    zhpro: Option<Arc<ZhProClient>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_url = format!("{}/ws/device", config.server_url.trim_end_matches('/'));
    info!("正在连接业务服务: {ws_url}");
    let (ws_stream, _) = connect_async(&ws_url).await?;
    let (mut write, mut read) = ws_stream.split();
    let (out_tx, mut out_rx) = mpsc::channel::<WsMessage>(128);

    // 板端 Agent 隧道：平台/浏览器都可能不在背包网段，页面要的模型目录、运行态、
    // AI 启停与检出框都借这条设备 WS 回到平台（见 `agent_relay` 模块）
    let relay = Arc::new(AgentRelay::new(
        config.ai_agent_url.clone(),
        config.ai_enabled,
    ));
    let relay_task = relay.enabled().then(|| {
        let relay = relay.clone();
        let out = out_tx.clone();
        tokio::spawn(async move { relay.forward_events(out).await })
    });
    // Agent 可能比客户端晚起来（板子重启顺序不固定）：没登记成功就每 15s 补一次
    let announce_task = relay.enabled().then(|| {
        let relay = relay.clone();
        let out = out_tx.clone();
        tokio::spawn(async move {
            loop {
                if relay.announce(&out).await {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        })
    });
    if relay.enabled() {
        info!(
            "板端 Agent 隧道已启用：{}（背包页的模型目录 / 运行态 / AI 启停可跨网段代调）",
            relay.agent()
        );
    }

    // 单写循环：所有发送集中在一处，避免并发写锁
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let text = serde_json::to_string(&msg).unwrap_or_default();
            if write.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    // 1) 注册：一并上报本体型号与固件版本，服务端落库后同步中台台账
    //    型号：本体 1201 `type` 优先，取不到用 `robot.model` / `--model` 兜底
    let body_model = match zhpro.as_ref() {
        Some(client) => client.session().await.device_type.trim().to_string(),
        None => String::new(),
    };
    let model = (!body_model.is_empty())
        .then_some(body_model)
        .or_else(|| config.model.clone());
    send(
        &out_tx,
        "register",
        json!({
            "deviceId": config.device_id,
            "secret": config.device_secret,
            "model": &model,
            "firmware": config.firmware,
        }),
    )
    .await;

    let state = {
        let mut initial = RobotState::new(config);
        initial.model = model;
        initial.mode = if zhpro.is_some() { "robot" } else { "sim" };
        Arc::new(Mutex::new(initial))
    };
    let active: Shared<Option<ActiveTask>> = Arc::new(Mutex::new(None));
    let monitor = Arc::new(Mutex::new(TaskMonitor::default()));
    let local_mapping: Shared<Option<Arc<MappingSession>>> = Arc::new(Mutex::new(None));
    let teleop: SharedTeleop = Arc::new(Mutex::new(TeleopState::default()));

    // 2) 本体推送任务（5009/1315/4001 等）与摇杆重发在 [`BodyLink::attach`] 里挂上，
    //    重连时用同一套句柄重新挂 —— 所以这里不再直接 spawn。

    // 3) 定时器
    let mut heartbeat_timer = interval(Duration::from_secs(config.heartbeat_interval.max(1)));
    let mut progress_timer = interval(Duration::from_secs(5));
    progress_timer.tick().await;
    let mut event_timer = interval(Duration::from_secs(120));
    // 摇杆看门狗：250ms 检查一次，超过阈值无新帧自动停车
    let mut teleop_timer = interval(Duration::from_millis(250));
    event_timer.tick().await;

    // 地图定时增量同步：只补平台没有的图，不覆盖已有记录。
    // 0 = 关闭（也可用 --no-map-sync 显式关）。
    let mut map_sync_timer = (config.map_sync_interval_secs > 0)
        .then(|| interval(Duration::from_secs(config.map_sync_interval_secs)));
    if let Some(timer) = map_sync_timer.as_mut() {
        // interval 的第一次 tick 立即就绪：先吃掉，让第一次同步在间隔之后
        timer.tick().await;
    }
    // 同一时间只跑一次：同步要拉几十兆地图包，并发会把本体文件服务压死
    let map_sync_running = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let local_runner: Shared<Option<Arc<RouteRunner>>> = Arc::new(Mutex::new(None));
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    // 航点 AI 识别：板端 Agent 与客户端同机（127.0.0.1 一定通），
    // 业务平台在公网够不到板端，所以由客户端本地代调 Agent（见 `ai_patrol` 模块）
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        config.ai_agent_url.clone(),
        config.ai_enabled,
        config.ai_jobs_dir.clone(),
    ));
    if config.ai_enabled {
        info!(
            "航点 AI 识别已启用：板端 Agent = {}（到点且有「AI 识别」动作时本地开识别）",
            ai.agent()
        );
    } else {
        info!("航点 AI 识别已关闭（--no-ai / ROBODOG_AI_ENABLED=0）");
    }

    // 航点灯光（补光灯 / LED）：与 AI 同一个板端 Agent、同一个总开关（`--no-ai`）。
    // 到点由 5009 `detail_index` 查路线动作，离开航点/任务结束关灯；夜间判定在板端。
    let lights = Arc::new(PointLights::new(
        config.ai_agent_url.clone(),
        config.ai_enabled,
    ));
    if config.ai_enabled {
        info!(
            "航点灯光已启用：板端 Agent = {}（航点动作 lightFill/lightLed，计划 lightNightOnly）",
            lights.agent()
        );
    }

    // 启动时兜一把残留：`detect_jobs` 下的计划目录本该在任务结束时删（`finish_plan`），
    // 但客户端被 kill / 断电时收尾没跑到，那些目录就永远留在板子上。这里只清「早就
    // 该走了」的（`STALE_PLAN_AGE`），不碰正在跑的和刚跑完的。
    {
        let ai = ai.clone();
        tokio::spawn(async move {
            ai.sweep_stale_plans().await;
        });
    }

    // 本体链路（可变）：断连后原地重建，业务 WebSocket 不受影响
    let body_ctx = BodyContext {
        config: Arc::new(config.clone()),
        ai: ai.clone(),
        lights: lights.clone(),
        out_tx: out_tx.clone(),
        state: state.clone(),
        active: active.clone(),
        monitor: monitor.clone(),
        local_runner: local_runner.clone(),
        local_mapping: local_mapping.clone(),
        teleop: teleop.clone(),
    };
    let mut body = BodyLink::new(zhpro);
    if let Some(client) = body.client.clone() {
        body.attach(&body_ctx, client);
    }
    // 手动「本体重连」指令 → 主循环（与自动重连同一条路径）
    let (body_reconnect_tx, mut body_reconnect_rx) = mpsc::channel::<()>(1);
    // 后台重连的结果回传：连上（新 client）或窗口内失败
    let (relink_tx, mut relink_rx) = mpsc::channel::<Result<Arc<ZhProClient>, String>>(1);
    let mut relinking = false;

    // 进度上报：内容没变就降到 [`PROGRESS_DEDUP_WINDOW`] 一次（服务端还会按「标题 + 描述」
    // 去重落事件）。**不能彻底停发**：服务端任务看门狗按 `tasks.updated_at` 判超时
    // （默认 120s），超时会直接把跑着的任务翻成「异常」。反过来，进度一直不动也会
    // 被「进度停滞」判据（`task_stuck_secs`）判异常，所以卡住时上报真实原因比刷
    // 「执行中」更有用。
    let mut progress_reporter = ProgressReporter::default();

    // 连接断开的原因：`None` = 主动退出（Ctrl-C）；`Some` = 平台侧断开 / 网络抖动，
    // 交给 `main` 的重连循环**原地重连**，不再让进程退出等 systemd 拉起
    // （以前一次平台重启就是「进程退出 → 5s 后 systemd 重启 → 再握手」，设备白掉线 10s+）。
    let mut close_reason: Option<String> = None;
    loop {
        // 每次迭代拷一份链路监视器：`wait_body_down` 只借用这个临时副本，
        // 分支体里才能安全地重建 `body`
        let mut probe = body.down.clone();
        tokio::select! {
            // 本体 control/bulkdata 断了：原地重连，不退出进程
            _ = wait_body_down(&mut probe) => {
                if relinking {
                    // 已在重连：结果由 `relink_rx` 分支处理，这里不能空转
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
                warn!("本体 control/bulkdata 通道已断开，开始原地重连（不退出进程）");
                start_relink(
                    &body_ctx,
                    &mut body,
                    &mut relinking,
                    &relink_tx,
                    "body-link-lost",
                )
                .await;
            }

            // 控制台「本体重连」：同一条重建路径，也不退出进程
            _ = body_reconnect_rx.recv() => {
                if relinking {
                    warn!("已在重连中，忽略重复的「本体重连」");
                    continue;
                }
                warn!("收到「本体重连」，开始原地重建本体链路");
                start_relink(&body_ctx, &mut body, &mut relinking, &relink_tx, "manual").await;
            }

            // 后台重连有结果了（心跳/指令这段时间一直照常跑）
            Some(result) = relink_rx.recv() => {
                relinking = false;
                match result {
                    Ok(client) => finish_relink(&body_ctx, &mut body, client).await,
                    Err(error) => {
                        error!("{error}；进程退出，等待 systemd 重启重建本体链路");
                        std::process::exit(1);
                    }
                }
            }

            _ = &mut ctrl_c => {
                info!("收到中断信号，退出");
                break;
            }

            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) else {
                            warn!("无法解析业务消息: {text}");
                            continue;
                        };
                        handle_server_message(
                            config,
                            ws_msg,
                            &out_tx,
                            &state,
                            &active,
                            &body.client,
                            &local_runner,
                            &local_mapping,
                            &teleop,
                            &body_reconnect_tx,
                            &ai,
                            &lights,
                            &relay,
                        ).await;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // 交给写循环回复 pong
                        let _ = data;
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("业务服务关闭了连接，准备原地重连");
                        close_reason = Some("业务服务关闭了连接".to_string());
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error!("WebSocket 错误: {e}，准备原地重连");
                        close_reason = Some(e.to_string());
                        break;
                    }
                    None => {
                        info!("业务连接已断开，准备原地重连");
                        close_reason = Some("业务连接已断开".to_string());
                        break;
                    }
                }
            }

            _ = heartbeat_timer.tick() => {
                let running = active.lock().await.is_some();
                let mut guard = state.lock().await;
                // 本体没在报状态（模拟模式 / 本体掉线）才走模拟推演
                if !guard.body_status_fresh() {
                    guard.tick(running);
                }
                let hb = guard.heartbeat();
                drop(guard);
                send(&out_tx, "heartbeat", serde_json::to_value(hb).unwrap_or_default()).await;
            }

            // 执行中定期回传进度（5009 推送是 500ms，这里合流成业务侧 5s 一次）
            _ = progress_timer.tick() => {
                let snapshot = active.lock().await.clone();
                let Some(task) = snapshot else { continue };
                let runner = local_runner.lock().await.clone();
                let mapping = local_mapping.lock().await.clone();
                if runner.is_none() && mapping.is_none() {
                    // 返充 / 出桩这类本体任务没有本地 runner：进度由 5009 快照驱动，
                    // 这里不重复上报，更不能拿「已暂停」去覆盖真实阶段。
                    if task.nav_task_id.is_some() {
                        continue;
                    }
                    // 既没有 runner 也没有本体任务 = 根本没有执行上下文（下发失败 / 客户端重启过），
                    // 继续上报只会每 5s 刷一条假「已暂停」，直接清掉本地占位。
                    warn!("任务 {} 本地没有执行上下文，停止进度上报", task.task_code);
                    clear_active_task(&active, &state).await;
                    continue;
                }
                // 航线任务的导航任务 ID 只在 RouteRunner 里，必须从这里取：
                // 服务端靠 `robot_task_id` 判断「暂停后能不能 resume」，缺了会被
                // 当成从未下发的任务重跑整条航线。
                let runner_nav_task = match &runner {
                    Some(runner) => runner.current_nav_task().await,
                    None => None,
                };
                let mut payload = TaskProgressPayload {
                    task_code: task.task_code.clone(),
                    status: "执行中".to_string(),
                    progress: 0,
                    point_index: None,
                    waypoint_index: task.point_index,
                    message: None,
                    stage: task.stage.clone(),
                    robot_task_id: runner_nav_task.or_else(|| task.nav_task_id.clone()),
                    total_points: None,
                };
                // 智航导航进度按航点折算；模拟模式按时间推演
                if let Some(runner) = runner {
                    let (done, total) = runner.progress().await;
                    payload.progress = progress_pct(done, total);
                    // `pointIndex` 是「已完成航点数」（1 基，前端显示「进度 n/N」）；
                    // 平台拿它查路线航点会查到下一个点，所以「站在哪个点」另有
                    // `waypointIndex`（上面已按 5009 的 detail_index 填好）。
                    payload.point_index = Some(done as i64);
                    payload.total_points = Some(total as i64);
                    payload.message = Some(format!("已完成 {done}/{total} 个航点"));
                    // 暂停时不能还报「执行中」，否则平台上的「已暂停」会被刷回去
                    if runner.is_paused() {
                        payload.status = "已暂停".to_string();
                        payload.message = Some("任务已暂停".to_string());
                    }
                }
                // 内容未变时降频，不再往事件流里刷重复进度（以前 5s 一条，暂停一小时几千条）；
                // 每次真正发出去都会把整份 payload 打到日志，方便对着平台上的读数排查
                progress_reporter.report(&out_tx, &payload, "定时").await;
            }

            _ = teleop_timer.tick() => {
                if let Some(ref client) = body.client {
                    teleop::tick_watchdog(client, &teleop).await;
                }
            }

            // 地图定时增量同步：只在完全空闲时跑（无任务 / 无建图 / 非重连中 /
            // 本体状态新鲜），并且抛到后台任务 —— 下载整包动辄几十秒，
            // 卡在主循环里会让心跳与 1315 处理停摆，反而把设备刷成离线。
            _ = async {
                match map_sync_timer.as_mut() {
                    Some(timer) => {
                        timer.tick().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {
                if relinking {
                    debug!("本体重连中，跳过本轮地图增量同步");
                    continue;
                }
                let Some(client) = body.client.clone() else {
                    continue;
                };
                if active.lock().await.is_some() || local_mapping.lock().await.is_some() {
                    debug!("设备正在执行任务/建图，跳过本轮地图增量同步");
                    continue;
                }
                if !state.lock().await.body_status_fresh() {
                    debug!("本体状态不新鲜，跳过本轮地图增量同步");
                    continue;
                }
                let running = map_sync_running.clone();
                if running.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    debug!("上一轮地图同步还没结束，跳过本轮");
                    continue;
                }
                // 本轮结束（含 panic）一定复位，否则一次意外就永久停掉轮询
                let guard = MapSyncRunning(running.clone());
                let cfg = body_ctx.config.clone();
                let out = out_tx.clone();
                tokio::spawn(async move {
                    let _guard = guard;
                    let result = sync_maps_from_robot(
                        &cfg,
                        &client,
                        &out,
                        None,
                        // 轮询永远只补缺，绝不覆盖平台已有地图
                        false,
                        // 没变化就不发事件，有新增/失败才发
                        false,
                    )
                    .await;
                    match result {
                        Ok(summary) => info!("定时地图同步结束: {summary}"),
                        Err(error) => warn!("定时地图同步有失败项: {error}"),
                    }
                });
            }

            _ = event_timer.tick() => {
                let guard = state.lock().await;
                if guard.body_status_fresh() && rand::thread_rng().gen_bool(0.3) {
                    let event = DeviceEventPayload {
                        event_type: "info".to_string(),
                        message: format!("机体温度 {:.1}℃", guard.temperature),
                        data: Some(json!({ "temperature": guard.temperature })),
                    };
                    drop(guard);
                    send(&out_tx, "event", serde_json::to_value(event).unwrap_or_default()).await;
                }
            }
        }
    }

    writer.abort();
    if let Some(task) = relay_task {
        task.abort();
    }
    if let Some(task) = announce_task {
        task.abort();
    }
    match close_reason {
        Some(reason) => Err(reason.into()),
        None => Ok(()),
    }
}

/// 等本体链路断开（永不返回 = 没有本体连接 / 尚未断开）
///
/// `watch::Receiver::changed()` 是取消安全的，可以反复放进 `select!` 里轮询。
async fn wait_body_down(link: &mut Option<tokio::sync::watch::Receiver<bool>>) {
    let Some(rx) = link.as_mut() else {
        std::future::pending::<()>().await;
        return;
    };
    if *rx.borrow_and_update() {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            // 发送端被丢弃：视为链路已不可用
            return;
        }
        if *rx.borrow_and_update() {
            return;
        }
    }
}

fn progress_pct(done: usize, total: usize) -> i64 {
    if total == 0 {
        return 0;
    }
    ((done * 100) / total) as i64
}

/// 地图同步运行标记的释放器：任务结束或 panic 时都会把「正在同步」复位。
struct MapSyncRunning(Arc<std::sync::atomic::AtomicBool>);

impl Drop for MapSyncRunning {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 本体故障 / 诊断 / 充电结果都是**周期性推送**（实测 4021 每 5s 一条、1317 每秒一条），
/// 内容不变时只上报一次，否则会在服务端刷出成百上千条 task_events / alerts。
#[derive(Default)]
struct EventDedup {
    seen: HashMap<String, Instant>,
}

impl EventDedup {
    /// 窗口期内出现过同样的 `event_type + message` 就丢弃
    fn accept(&mut self, event_type: &str, message: &str) -> bool {
        const WINDOW: Duration = Duration::from_secs(300);
        let now = Instant::now();
        self.seen.retain(|_, at| now.duration_since(*at) < WINDOW);
        let key = format!("{event_type}|{message}");
        if self.seen.contains_key(&key) {
            return false;
        }
        self.seen.insert(key, now);
        true
    }
}

/// 去重后上报一条设备事件
async fn send_deduped_event(
    out_tx: &mpsc::Sender<WsMessage>,
    dedup: &Arc<Mutex<EventDedup>>,
    event: DeviceEventPayload,
) {
    if !dedup.lock().await.accept(&event.event_type, &event.message) {
        debug!("跳过重复事件: {} / {}", event.event_type, event.message);
        return;
    }
    send(
        out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
}

/// 本体推送处理：5009 状态、1315 本体状态、1401 位姿、4001 建图收尾、4021 故障
///
/// 参数都是运行期共享句柄（含 `teleop`：控制权丢失时要在同一处复位接管态），
/// 拆成结构体只会多一层搬运，故保留平铺签名。
#[allow(clippy::too_many_arguments)]
fn spawn_zhpro_pump(
    client: Arc<ZhProClient>,
    out_tx: mpsc::Sender<WsMessage>,
    state: Shared<RobotState>,
    active: Shared<Option<ActiveTask>>,
    monitor: Arc<Mutex<TaskMonitor>>,
    local_mapping: Shared<Option<Arc<MappingSession>>>,
    teleop: SharedTeleop,
    config: Arc<Config>,
    ai: Arc<AiPatrol>,
    lights: Arc<PointLights>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = client.subscribe();
        let dedup = Arc::new(Mutex::new(EventDedup::default()));
        // 只在任务快照的 lifecycle/stage/error 变化时打一条，避免 5009 500ms 心跳刷屏
        let mut last_task_log: Option<String> = None;
        // 5009 驱动的任务进度（本体任务 / 返充出桩）：自己一套去重窗口，
        // 不能真按 500ms 原样上报，也不能傻到一帧不漏地刷日志
        let mut progress_reporter = ProgressReporter::default();
        while let Ok(env) = rx.recv().await {
            match env.head.msg_type {
                T_TASK_STATUS => {
                    let Some(frame) = StatusFrame::parse(&env.data) else {
                        continue;
                    };
                    {
                        let mut guard = state.lock().await;
                        guard.loc_status = frame.loc_status;
                        guard.loc_fresh = frame.loc_fresh;
                    }
                    // 同一 boot_id 内按 snapshot_seq 丢弃重复帧，revision 变化才意味着任务内容变化
                    let Some(accepted) = monitor.lock().await.accept(&frame) else {
                        continue;
                    };
                    if accepted.boot_changed {
                        // 算法节点重启（boot_id 变了）：上一轮的 5009 快照全部作废，
                        // 本地任务视图也要清空，否则界面/台账会拿着不存在的任务继续跑。
                        // 业务任务上下文（active / runner）不在这里清：终态由 runner 自己
                        // 的上报或看门狗收尾，清了反而永远报不上去。
                        warn!(
                            "5009 boot_id 变化（算法节点重启，boot={}），清空本地任务快照",
                            accepted.frame.boot_id
                        );
                        state.lock().await.current_task = None;
                        last_task_log = None;
                    }
                    if accepted.frame.reason != "heartbeat" {
                        debug!(
                            "5009 帧: reason={} revision={} task_state={} loc={}",
                            accepted.frame.reason,
                            accepted.frame.revision,
                            accepted.frame.robot_task_state,
                            accepted.frame.loc_status
                        );
                    }
                    // 把 current/pending/最近终态的关键字段留痕：5001 code=0 但任务
                    // 不执行时，只有 stage/error/stop_reason 能说明卡在哪一步。
                    let snapshot = accepted
                        .frame
                        .current
                        .as_ref()
                        .map(|task| (task, "current"))
                        .or_else(|| {
                            accepted
                                .frame
                                .pending
                                .as_ref()
                                .map(|task| (task, "pending"))
                        })
                        .or_else(|| {
                            accepted
                                .frame
                                .terminals
                                .first()
                                .map(|task| (task, "terminal"))
                        });
                    if let Some((task, source)) = snapshot {
                        let key = format!(
                            "{}|{}|{}|{}|{:?}|{:?}",
                            task.task_id,
                            task.lifecycle,
                            task.stage,
                            task.task_type,
                            task.error_message,
                            task.stop_reason
                        );
                        if last_task_log.as_deref() != Some(key.as_str()) {
                            // 本体的 5009 响应：只在 lifecycle/stage/error 变化时打一条，
                            // 所以放在 info —— 默认 `journalctl` 就能看到「本体回什么」。
                            // （原始帧本身是 500ms 一条，只在 debug/trace 或帧落盘里出现）
                            info!(
                                "5009 任务({source}): id={} name={} type={} lifecycle={} stage={}                                  progress={:?} point={:?}/{:?} error={:?} stop={:?}                                  task_state={} loc={}",
                                task.task_id,
                                task.name,
                                task.task_type,
                                task.lifecycle,
                                task.stage,
                                task.progress,
                                task.detail_index,
                                task.detail_total,
                                task.error_message,
                                task.stop_reason,
                                accepted.frame.robot_task_state,
                                accepted.frame.loc_status
                            );
                            last_task_log = Some(key);
                        }
                    } else {
                        last_task_log = None;
                    }
                    let frame = accepted.frame.clone();
                    handle_status_frame(
                        &frame,
                        &out_tx,
                        &state,
                        &active,
                        &monitor,
                        &mut progress_reporter,
                        &ai,
                        &lights,
                    )
                    .await;
                }
                T_ROBOT_STATUS => {
                    // 1315 里的运动状态 / 机器模式 / 控制源变化要留痕：
                    // 现场争议最多的两个问题是「它什么时候自己趴下的」和
                    // 「谁把控制权拿走了」，以前只能靠 journalctl 猜。
                    let mut notices: Vec<(String, Value)> = Vec::new();
                    let mut invalidate_control = false;
                    {
                        let mut guard = state.lock().await;
                        let before_motion = guard.motion_status.clone();
                        let before_mode = guard.robot_mode.clone();
                        let before_ctrl = guard.ctrl_source;
                        guard.apply_robot_status(&env.data);
                        if before_motion != guard.motion_status {
                            let from = before_motion.unwrap_or_else(|| "未知".to_string());
                            let to = guard.motion_status.clone().unwrap_or_default();
                            notices.push((
                                format!("本体运动状态变更: {from} → {to}"),
                                json!({ "from": from, "to": to }),
                            ));
                        }
                        if before_mode != guard.robot_mode {
                            let from = before_mode.unwrap_or_else(|| "未知".to_string());
                            let to = guard.robot_mode.clone().unwrap_or_default();
                            notices.push((
                                format!("本体机器模式变更: {from} → {to}"),
                                json!({ "from": from, "to": to }),
                            ));
                        }
                        if before_ctrl != guard.ctrl_source
                            && let Some(source) = guard.ctrl_source
                        {
                            // 控制权被外部遥控器/未知来源拿走时，本地的 1204
                            // 标记必须失效；否则下一次“远程接管”会误以为还持有
                            // RoamerX，跳过 1204，摇杆帧被网关静默丢弃。
                            invalidate_control = matches!(source, 0 | 3);
                            notices.push((
                                format!(
                                    "本体控制源变更: ctrl_source={source}\
                                     （0 未知 / 1 APP / 2 SDK / 3 遥控器）"
                                ),
                                json!({ "ctrlSource": source }),
                            ));
                        }
                    }
                    if invalidate_control {
                        client.invalidate_control().await;
                        // 1204 的本地标记失效时，`teleop` 的接管态与摇杆缓存必须一起清：
                        // 只清一半会让摇杆首帧跳过 1204，1304 在没有 owner 的情况下被网关
                        // 丢掉，现场表现就是「平台显示已接管、狗子不动且没有任何报错」。
                        teleop::reset_after_control_lost(&client, &teleop).await;
                    }
                    for (message, data) in notices {
                        let event = DeviceEventPayload {
                            event_type: "info".to_string(),
                            message,
                            data: Some(data),
                        };
                        send(
                            &out_tx,
                            "event",
                            serde_json::to_value(event).unwrap_or_default(),
                        )
                        .await;
                    }
                }
                T_MODULE_NOTIFY => {
                    // 1207 modules[]：官方 App 用 sdk_node 是否可用决定是否让
                    // 用户进入控制/任务；我们以前只打 DEBUG，导致 sdk_node=false
                    // 时仍然照发 1302/1304/5000。
                    if let Some(modules) = env.data.get("modules").and_then(Value::as_array) {
                        let mut available = None;
                        for module in modules {
                            if module.get("name").and_then(Value::as_str) == Some("sdk_node") {
                                available = module.get("available").and_then(Value::as_bool);
                            }
                        }
                        if let Some(available) = available {
                            let mut guard = state.lock().await;
                            if guard.sdk_node_available != Some(available) {
                                if available {
                                    info!("本体 SDK 节点已就绪（sdk_node=true）");
                                } else {
                                    warn!(
                                        "本体 SDK 节点未就绪（sdk_node=false）：运动/导航指令会被拒"
                                    );
                                }
                                guard.sdk_node_available = Some(available);
                            }
                        }
                    }
                }
                T_CURRENT_POSE => {
                    state.lock().await.apply_pose(&env.data);
                }
                T_MAP_SAVED => {
                    let ok = env.is_ok();
                    info!(
                        "收到 4001 地图保存结果: {} {}",
                        if ok { "成功" } else { "失败" },
                        env.message()
                    );

                    // 交给建图会话解析（会话只认第一次，避免重复收尾）
                    let session = local_mapping.lock().await.clone();
                    let Some(session) = session else {
                        // 不在建图流程中（例如运维手动建图），只记录事件
                        send_map_saved_event(&out_tx, ok, env.message(), None).await;
                        continue;
                    };
                    let Some(saved) = session.on_map_saved(&env.data).await else {
                        continue;
                    };

                    // 建图任务收尾：给业务侧一个明确的终态，而不是只发一条 event
                    let task_code = {
                        let guard = active.lock().await;
                        guard
                            .as_ref()
                            .map(|a| a.task_code.clone())
                            .unwrap_or_default()
                    };
                    let map_hint = saved.dir_name().or_else(|| saved.map_path.clone());

                    if saved.success {
                        // 成功：先把「已保存到本体」报上去，再尝试回传中台
                        report_progress(
                            &out_tx,
                            &task_code,
                            "执行中",
                            90,
                            Some(match &map_hint {
                                Some(name) => format!("地图已保存到本体: {name}"),
                                None => "地图已保存到本体".to_string(),
                            }),
                            Some("mapping.saved".to_string()),
                            Some("mapping".to_string()),
                            None,
                        )
                        .await;

                        // 地图回传中台（A 方案）
                        let outcome = upload_mapping_to_zentai(&config, &saved).await;
                        let (status, progress, message) = match outcome {
                            UploadOutcome::Skipped(reason) => {
                                ("已完成", 100, format!("建图完成（未回传中台：{reason}）"))
                            }
                            UploadOutcome::Uploaded { map_id } => {
                                ("已完成", 100, format!("建图完成，地图已入库中台: {map_id}"))
                            }
                            UploadOutcome::Failed { reason } => {
                                ("异常", 90, format!("建图完成，但回传中台失败: {reason}"))
                            }
                        };
                        report_progress(
                            &out_tx,
                            &task_code,
                            status,
                            progress,
                            Some(message.clone()),
                            Some("mapping.done".to_string()),
                            Some("mapping".to_string()),
                            None,
                        )
                        .await;
                        send_map_saved_event(&out_tx, true, &saved.message, map_hint.clone()).await;
                        info!("建图任务 {task_code} 收尾: {message}");
                        *local_mapping.lock().await = None;
                        *active.lock().await = None;
                        state.lock().await.current_task = None;
                    } else {
                        let message = format!("建图保存失败: {}", saved.message);
                        report_progress(
                            &out_tx,
                            &task_code,
                            "异常",
                            0,
                            Some(message.clone()),
                            Some("mapping.failed".to_string()),
                            Some("mapping".to_string()),
                            None,
                        )
                        .await;
                        send_map_saved_event(&out_tx, false, &saved.message, None).await;
                        *local_mapping.lock().await = None;
                        *active.lock().await = None;
                        state.lock().await.current_task = None;
                    }
                }
                T_FAULT_CODE => {
                    if let Some(items) = env.data.get("items").and_then(Value::as_array) {
                        for item in items {
                            let desc = item
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("未知故障");
                            let severity =
                                item.get("severity").and_then(Value::as_i64).unwrap_or(0);
                            // 码值必须落到事件文案里：平台 task_events 只存 description，
                            // 没有 code/severity 时没法对着本体文档 / 厂家排查。
                            let message = match item.get("code").and_then(Value::as_i64) {
                                Some(code) => format!("{desc}（code={code}, 级别={severity}）"),
                                None => desc.to_string(),
                            };
                            let event = DeviceEventPayload {
                                event_type: if severity >= 1 { "fault" } else { "info" }
                                    .to_string(),
                                message,
                                data: Some(item.clone()),
                            };
                            send_deduped_event(&out_tx, &dedup, event).await;
                        }
                    }
                }
                T_FAULT_STATUS => {
                    // 1317 只上报最严重的一条（level>=2 才算故障）
                    let worst =
                        env.data
                            .get("faults")
                            .and_then(Value::as_array)
                            .and_then(|faults| {
                                faults.iter().max_by_key(|f| {
                                    f.get("level").and_then(Value::as_i64).unwrap_or(0)
                                })
                            });
                    if let Some(worst) = worst
                        && worst.get("level").and_then(Value::as_i64).unwrap_or(0) >= 2
                    {
                        let event = DeviceEventPayload {
                            event_type: "fault".to_string(),
                            message: format!(
                                "{} / {}",
                                worst
                                    .get("module")
                                    .and_then(Value::as_str)
                                    .unwrap_or("body"),
                                worst
                                    .get("fault")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown")
                            ),
                            data: Some(worst.clone()),
                        };
                        send_deduped_event(&out_tx, &dedup, event).await;
                    }
                }
                T_CHARGE_RESULT => {
                    let status = env.data.get("status").and_then(Value::as_i64).unwrap_or(0);
                    let label = match status {
                        1 => "回充成功，已进入充电",
                        3 => "满电保持",
                        _ => "回充失败",
                    };
                    info!("5019 底层充电结果: {label}");
                    let event = DeviceEventPayload {
                        event_type: if status == 2 { "error" } else { "info" }.to_string(),
                        message: format!("{label}: {}", env.message()),
                        data: Some(env.data.clone()),
                    };
                    send_deduped_event(&out_tx, &dedup, event).await;
                }
                _ => {}
            }
        }
    })
}

/// 把 5009 快照翻译成业务侧进度
#[allow(clippy::too_many_arguments)]
async fn handle_status_frame(
    frame: &StatusFrame,
    out_tx: &mpsc::Sender<WsMessage>,
    state: &Shared<RobotState>,
    active: &Shared<Option<ActiveTask>>,
    monitor: &Arc<Mutex<TaskMonitor>>,
    reporter: &mut ProgressReporter,
    ai: &Arc<AiPatrol>,
    lights: &Arc<PointLights>,
) {
    // 文档 §2.6.10 第 5 条：即时终态可能已经在 `current_task` 里被回收，只留在
    // `recent_terminal_tasks` 里（最多 3 条 / 约 30s）。两处都要识别，否则
    // 「已建链但终态帧丢了/断开重连期间到终态」时业务侧永远收不到终态，
    // 只能等看门狗把一条成功的任务判成「异常」。
    let bound_id = {
        let guard = active.lock().await;
        guard.as_ref().and_then(|a| a.nav_task_id.clone())
    };
    let task = match frame.current.as_ref() {
        Some(task) => task.clone(),
        None => {
            // 没有 current_task 时只认「本端已绑定的那个 nav_task_id」的终态，
            // 别人的历史终态一律忽略。
            let Some(id) = bound_id.as_deref() else {
                return;
            };
            let Some(task) = frame.terminals.iter().find(|t| t.task_id == id) else {
                return;
            };
            task.clone()
        }
    };

    // 只跟随我们下发的那个任务
    let bound = {
        let guard = active.lock().await;
        match guard.as_ref() {
            Some(a) => match &a.nav_task_id {
                Some(id) => *id == task.task_id,
                // 尚未绑定（刚下发）时用类型相近判定
                None => a.task_code == task.name || task.task_type == "patrol",
            },
            None => false,
        }
    };
    if !bound {
        return;
    }

    // 终态上报幂等：同一条任务同一个 lifecycle 只报一次。
    // 后面每一帧心跳都会带同一个终态（current_task 一次 + recent_terminal_tasks 多次），
    // 不去重就是每 500ms 往业务库推一条「已完成」。
    if task.is_terminal() && !monitor.lock().await.take_terminal(&task) {
        return;
    }

    {
        let mut guard = active.lock().await;
        if let Some(a) = guard.as_mut() {
            a.nav_task_id = Some(task.task_id.clone());
            a.stage = Some(task.stage.clone());
            a.point_index = task.detail_index;
        }
    }

    // 阶段 → 业务状态
    let (status, message) = classify_stage(&task);
    let index = task.detail_index;
    let progress = task
        .progress
        .map(|p| p.round() as i64)
        .or_else(|| index.map(|i| (i + 1) * 10))
        .unwrap_or(0);

    let payload = TaskProgressPayload {
        task_code: {
            let guard = active.lock().await;
            guard
                .as_ref()
                .map(|a| a.task_code.clone())
                .unwrap_or_default()
        },
        status: status.to_string(),
        progress,
        point_index: index,
        waypoint_index: index,
        message: Some(message),
        stage: Some(task.stage.clone()),
        robot_task_id: Some(task.task_id.clone()),
        total_points: None,
    };
    // 每次真正发出去的 payload 都打到日志：现场排查「平台显示的状态与本体不一致」时，
    // 先看这里上报了什么，再去对上面那条 5009 快照行，比猜更直接
    reporter.report(out_tx, &payload, "5009").await;

    // 航点 AI 识别：走到带「AI 识别」动作的航点就本地开识别窗口，终态时关窗。
    // 与服务端版本同一条规则（`patrol.point_arrived` / `patrol.waiting` + 该点有 AI 动作），
    // 只是把「谁去调 Agent」从公网平台换成了同机的客户端。
    // 识别失败只打日志，绝不影响巡检本身。
    if task.is_terminal() {
        ai.stop("任务终态").await;
        // 灯光：跑完 / 失败 / 取消都要收——灯亮着没人管就是白耗电
        lights.leave("任务终态").await;
        // 任务跑完了（成功 / 失败 / 取消都一样）：整个计划的板端存证都没用了。
        // 只靠「推过图就删这个航点」（`ai_cleanup`）清不干净——数量任务的检出不建单
        // 不推群、一个都没命中的航点只留一份 result.json，它们等不到 `ai_cleanup`。
        let plan_id = active
            .lock()
            .await
            .as_ref()
            .map(|task| task.task_code.clone());
        if let Some(plan_id) = plan_id {
            ai.finish_plan(&plan_id, "任务终态").await;
        }
    } else if is_arrival_stage(&task.stage) {
        let current = active.lock().await.clone();
        if let (Some(current), Some(index)) = (current, index) {
            // 灯光先开：AI 要亮着灯看，顺序反了前几帧还是黑的（补光灯从写 GPIO
            // 到真正亮起来只要几十毫秒，但识别窗口开得比这快）。开灯放到后台——
            // `POST /api/lights` 里有几次 GPIO shell 调用，不能让 5009 处理链路等它
            match light_point_at(&current.light_points, index) {
                Some(point) => {
                    let lights = lights.clone();
                    let task_code = current.task_code.clone();
                    let point = point.clone();
                    let night_only = current.light_night_only;
                    tokio::spawn(async move {
                        lights.point_arrived(&task_code, &point, night_only).await;
                    });
                }
                // 这个点没配灯：上一个点的灯别继续亮着（狗已经走了）
                None => lights.leave("到达无灯光动作的航点").await,
            }
            match ai_point_at(&current.ai_points, index) {
                Some(point) => {
                    ai.point_arrived(out_tx, &current.task_code.clone(), point)
                        .await
                }
                // 这个点没有 AI 动作：上一个点的窗口别继续跑（画面已经变了）
                None => ai.stop("到达无 AI 动作的航点").await,
            }
        }
    } else if !is_point_stage(&task.stage) {
        // 狗已经离开航点（前往下一点 / 返航 / 别的任务阶段）：窗口时长跟着停留时间走，
        // 收口交给这里，别让相机把路过的东西当成这个点的目标
        ai.stop("已离开航点").await;
        // 灯同一个道理：录像/识别已经结束，补光灯没必要一路亮到下一个点
        lights.leave("已离开航点").await;
    }

    // 离散事件（去重后只报一次）
    if monitor.lock().await.take_event(&task) {
        let event = DeviceEventPayload {
            event_type: "progress".to_string(),
            message: format!("{}: {}", task.last_event_type, task.stage),
            data: Some(json!({
                "taskId": task.task_id,
                "stage": task.stage,
                "index": task.detail_index,
                "total": task.detail_total,
            })),
        };
        send(
            out_tx,
            "event",
            serde_json::to_value(event).unwrap_or_default(),
        )
        .await;
    }

    // 终态：清空占用
    if task.is_terminal() {
        if let Some(lo) = task.detail_index {
            let _ = lo;
        }
        let mut guard = active.lock().await;
        if let Some(a) = guard.as_ref() {
            info!(
                "任务 {} [{}] 进入终态: {}",
                a.task_code, a.task_name, task.lifecycle
            );
        }
        *guard = None;
        let mut st = state.lock().await;
        st.current_task = None;
    }
}

/// 定位收敛等待上限：设备被搬运后重定位通常 10~30s，超过就该判失败而不是干等
const LOCATE_TIMEOUT: Duration = Duration::from_secs(60);

/// 自动重定位最多试几次（含首次）
///
/// 现场 SLAM 有一定概率第一轮不收敛（贴着墙 / 特征太少），自动再试一次能消掉大部分
/// 「必须人工点重新定位」的情况；再多就只是把异常拖长，不如早点把原因报给运维。
const LOCATE_ATTEMPTS: u32 = 2;

/// 两次自动重定位之间的间隔：给本体把上一轮「重定位中」的现场清干净
const LOCATE_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// 进度内容没变时的上报窗口
///
/// 必须明显小于服务端 `task_timeout_secs`（默认 120s）：看门狗靠 `tasks.updated_at`
/// 判「执行中但不上报」，彻底停发会把正在跑的任务翻成异常。
const PROGRESS_DEDUP_WINDOW: Duration = Duration::from_secs(30);

/// `task_progress` 上报器：去重 + 打印整份载荷。
///
/// 两个消费路径（5s 定时器、5009 快照）内容不变时都会重复产生同一份 payload：
/// - 不能原样刷：暂停一小时能刷出几千条一样的进度，服务端事件流也会被冲掉；
/// - 也不能彻底不发：服务端看门狗按 `tasks.updated_at` 判 120s 超时。
///
/// 所以按「整份 JSON 不变就 [`PROGRESS_DEDUP_WINDOW`] 一次」处理，
/// 并且只要真的发出去就把载荷打到 `info`，不用开 debug 就能对账。
#[derive(Default)]
struct ProgressReporter {
    last: Option<(String, Instant)>,
}

impl ProgressReporter {
    async fn report(
        &mut self,
        out_tx: &mpsc::Sender<WsMessage>,
        payload: &TaskProgressPayload,
        source: &str,
    ) {
        let value = serde_json::to_value(payload).unwrap_or_default();
        let key = value.to_string();
        if let Some((previous, at)) = self.last.as_ref()
            && previous == &key
            && at.elapsed() < PROGRESS_DEDUP_WINDOW
        {
            debug!("task_progress({source}) 内容未变，不上报: {key}");
            return;
        }
        info!("→ 上报 task_progress({source}): {key}");
        self.last = Some((key, Instant::now()));
        send(out_tx, "task_progress", value).await;
    }
}

/// 平台下发的初始位姿：`initialPose: { x, y, heading }`（heading 单位度，与路线航点一致）
///
/// 缺字段或未下发时返回 `None`，交给 SLAM 读取上一份位姿文件做完整重定位。
fn parse_initial_pose(value: Option<&Value>) -> Option<Pose> {
    let pose = value?;
    let x = pose.get("x").and_then(Value::as_f64)?;
    let y = pose.get("y").and_then(Value::as_f64)?;
    let heading = pose.get("heading").and_then(Value::as_f64).unwrap_or(0.0);
    Some(Pose::new(x, y, heading.to_radians()))
}

/// 巡检起步前的定位守卫（**自动**：执行计划时不依赖人工点「重新定位」）。
///
/// 判据是「**这条路线绑的那张图** + 定位收敛到 `100`」，缺一项就当场 `4030 op=1` 载图
/// 重定位：
/// - 定位没收敛（`121/122/203…`）→ 重定位；
/// - 已收敛但载的不是这张图（`active_map` 对不上）→ 也重定位，否则本体会在错的地图
///   坐标系里跑完整条航线才报错；
/// - 已经是「这张图 + 100」→ 直接放行，**不重载**（重载会把位姿重置回上一份位姿文件）。
///
/// 重定位默认下发**零四元数**（[`Pose::unspecified`]），由 SLAM 读上一份位姿文件做完整
/// 重定位——现场绝大多数「设备没动过，只是没载图 / 定位丢了」靠这条自愈。平台显式下发了
/// `initialPose` 时以平台为准（设备被搬动过的场景）。
///
/// 不收敛会再自动试一次（SLAM 有一定概率第一轮不收敛）；`203/210` 是 SLAM 明确失败，
/// 不白等第二轮。
///
/// 返回 `(定位状态, 本次是否真的载图定位过)`。
async fn ensure_localized(
    client: &ZhProClient,
    state: &Shared<RobotState>,
    map_id: &str,
    map_path: &str,
    initial_pose: Option<Pose>,
) -> Result<(i64, bool), String> {
    // 4030 在 4000~5999 非白名单区间，必须先拿到 RoamerX 控制权，否则网关静默丢弃
    if let Err(e) = client.ensure_control().await {
        warn!("取 RoamerX 控制权失败（4030 可能被网关丢弃）: {e}");
    }
    // 先取快照再 await，避免把 state 的锁带过 await 点
    let active_map = state.lock().await.active_map.clone();
    let same_map = map_matches(active_map.as_deref(), map_id, map_path);
    match zhpro::mapping::latest_loc_status(client, Duration::from_secs(2)).await {
        Some(status) if zhpro::mapping::loc_status_ready(status) && same_map => {
            return Ok((status, false));
        }
        Some(status) if zhpro::mapping::loc_status_ready(status) => info!(
            "定位正常（status={status}）但本体载的不是本任务地图（当前 {}，需要 {}），\
             自动载图重定位 map={map_path}",
            active_map.as_deref().unwrap_or("<未知>"),
            if map_id.trim().is_empty() {
                map_path
            } else {
                map_id.trim()
            }
        ),
        Some(status) => info!(
            "定位未就绪（{}，status={status}），自动载图重定位 map={map_path}",
            zhpro::mapping::describe_loc_status(status)
        ),
        None => warn!("2s 内未收到 5009 定位状态，直接尝试自动载图重定位 map={map_path}"),
    }
    let mut last_error = String::new();
    for attempt in 1..=LOCATE_ATTEMPTS {
        if attempt > 1 {
            info!("自动重定位第 {attempt}/{LOCATE_ATTEMPTS} 次（上次失败：{last_error}）");
            tokio::time::sleep(LOCATE_RETRY_BACKOFF).await;
        }
        match zhpro::mapping::relocalize(client, map_path, initial_pose, LOCATE_TIMEOUT).await {
            Ok(status) => {
                // 只有真的载进去了才更新 active_map（它同时被心跳的 map_id、
                // 「召回充电」5014 的校验参数复用）
                let loaded = if map_id.trim().is_empty() {
                    map_path.to_string()
                } else {
                    map_id.trim().to_string()
                };
                state.lock().await.active_map = Some(loaded);
                return Ok((status, true));
            }
            // 203/210：SLAM 自己判了失败，再等一轮 60s 只是拖长异常
            Err(e) if zhpro::mapping::is_fatal_locate_error(&e) => return Err(e),
            Err(e) => {
                warn!("自动重定位第 {attempt}/{LOCATE_ATTEMPTS} 次失败: {e}");
                last_error = e;
            }
        }
    }
    Err(last_error)
}

/// `active_map` 是否就是本任务要用的地图。
///
/// `active_map` 绝大多数时候存的是**业务地图 ID**（`MAP-ZJ-01@v2.1`），但也可能被写成
/// 本体目录名，所以三种写法都认：业务 ID、`to_map_dir_name(业务 ID)`、`map_path` 最后一段。
fn map_matches(active_map: Option<&str>, map_id: &str, map_path: &str) -> bool {
    let Some(active) = active_map.map(str::trim).filter(|s| !s.is_empty()) else {
        return false;
    };
    let map_id = map_id.trim();
    let dir = zhpro::mapping::candidate_dir_name(map_path).unwrap_or_default();
    active == map_id
        || (!map_id.is_empty() && active == zhpro::mapping::to_map_dir_name(map_id))
        || (!dir.is_empty() && active == dir)
}

/// 5009 `stage` → 业务任务状态
fn classify_stage(task: &TaskSnapshot) -> (&'static str, String) {
    if task.is_terminal() {
        return match task.lifecycle.as_str() {
            "success" => ("已完成", "任务完成".to_string()),
            "cancelled" => (
                "已暂停",
                task.stop_reason
                    .clone()
                    .unwrap_or_else(|| "任务被取消".to_string()),
            ),
            _ => (
                "异常",
                task.error_message
                    .clone()
                    .or_else(|| task.stop_reason.clone())
                    .unwrap_or_else(|| "任务失败".to_string()),
            ),
        };
    }
    if task.lifecycle == "paused" {
        return ("已暂停", "任务已暂停".to_string());
    }
    let message = match task.stage.as_str() {
        "mapping.preparing" => "建图准备中".to_string(),
        "mapping.running" => "建图中".to_string(),
        "mapping.saving" => "地图保存中".to_string(),
        "navigation.localizing" => "定位中".to_string(),
        "navigation.navigating" | "patrol.navigating" => "前往航点".to_string(),
        "patrol.point_arrived" => "已到达航点".to_string(),
        "patrol.waiting" => "航点停留".to_string(),
        "patrol.photographing" => "拍照采集".to_string(),
        "patrol.point_completed" => "航点作业完成".to_string(),
        "patrol.returning" => "返航中".to_string(),
        "patrol.returning_to_charge" | "return_to_charge.navigating" => "返回充电点".to_string(),
        "patrol.recharging" | "return_to_charge.recharging" | "recharge.charging" => {
            "充电中".to_string()
        }
        other => other.to_string(),
    };
    ("执行中", message)
}

/// 处理业务服务下发消息
#[allow(clippy::too_many_arguments)]
async fn handle_server_message(
    config: &Config,
    msg: WsMessage,
    out_tx: &mpsc::Sender<WsMessage>,
    state: &Shared<RobotState>,
    active: &Shared<Option<ActiveTask>>,
    zhpro: &Option<Arc<ZhProClient>>,
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    local_mapping: &Shared<Option<Arc<MappingSession>>>,
    teleop: &SharedTeleop,
    body_reconnect: &mpsc::Sender<()>,
    ai: &Arc<AiPatrol>,
    lights: &Arc<PointLights>,
    relay: &Arc<AgentRelay>,
) {
    match msg.msg_type.as_str() {
        "register_ack" => {
            if msg.payload["success"].as_bool().unwrap_or(false) {
                // 服务端在注册回执里下发设备绑定的园区；没绑也能用，只是地图会先不归属园区
                let park_id = msg.payload["parkId"].as_i64();
                *config.bound_park_id.lock().await = park_id;
                match park_id {
                    Some(park_id) => info!("设备注册成功: {}（园区 {park_id}）", config.device_id),
                    None => info!(
                        "设备注册成功: {}（未绑定园区：地图会先入库为设备地图，创建园区时再挑图绑定）",
                        config.device_id
                    ),
                }
                // 注册成功后立刻打个招呼：本机 Agent 真的连得上，平台才会把背包页的
                // 请求（模型目录 / 运行态 / AI 启停）转到这条连接上
                relay.announce(out_tx).await;
            } else {
                error!(
                    "设备注册失败: {}",
                    msg.payload["message"].as_str().unwrap_or("未知错误")
                );
            }
        }

        // 心跳回执：服务端在这里同步「强制接管」标记（客户端重启后本地标记会丢，
        // 不同步的话摇杆又会被 `ctrl_source = 3` 门禁拦下）
        "heartbeat_ack" => {
            if let Some(force) = msg.payload["teleopForce"].as_bool() {
                let mut guard = state.lock().await;
                if guard.teleop_force != force {
                    guard.teleop_force = force;
                    if force {
                        warn!("平台处于强制接管态：暂时忽略 ctrl_source 抢占门禁");
                    } else {
                        info!("平台已退出强制接管态：恢复遥控器抢占检测");
                    }
                }
            }
        }

        // 航点 AI 识别：平台已经按路线里的动作算好场景/类别/阈值/窗口秒数，
        // 这里只管在本机启停板端 Agent 并回传检出（服务端够不到板端 Agent）
        "ai_start" => match AiStart::from_value(&msg.payload) {
            Some(start) => ai.start(out_tx, start).await,
            None => warn!("ai_start 载荷缺少 windowId，忽略: {}", msg.payload),
        },

        // 背包页的请求（模型目录 / 运行态 / AI 启停）：本机代调 Agent 后回 agent_result
        "agent_request" => relay.on_request(out_tx, &msg.payload).await,

        // 平台关窗（任务终态 / 暂停 / 取消）：窗口任务自己停 Agent 并回报 ai_end
        "ai_stop" => {
            let reason = msg.payload["reason"].as_str().unwrap_or("平台关闭窗口");
            ai.stop(reason).await;
        }

        // 证据图已经进了企业微信群：删掉板端这一轮的存证
        // （`detect_jobs/<计划>/pt<航点>/` 里的画框 JPG + result.json）
        "ai_cleanup" => {
            let plan_id = msg.payload["planId"].as_str().unwrap_or("");
            let point_index = msg.payload["pointIndex"].as_i64().unwrap_or(-1);
            let reason = msg.payload["reason"].as_str().unwrap_or("平台已推送证据图");
            ai.cleanup_evidence(plan_id, point_index, reason).await;
        }

        // 事件回执：客户端发的 event 由服务端落库后回 ack，不需要额外处理
        "event_ack" => {}

        // 指令结果回执：业务平台确认收到 `command_result`，不需要额外处理
        "command_result_ack" => {}

        // 进度上报的逐条回执：客户端已经在本地合流上报，不需要逐条处理，
        // 也不能当「未处理的消息类型」打 WARN（任务执行时每 5s 一条会刷爆日志）
        "task_progress_ack" => {}

        "park_binding" => {
            // 控制台绑定 / 解绑园区后实时下发，无需重连
            let park_id = msg.payload["parkId"].as_i64();
            *config.bound_park_id.lock().await = park_id;
            match park_id {
                Some(park_id) => info!(
                    "园区绑定已更新: parkId={park_id} {}",
                    msg.payload["parkName"].as_str().unwrap_or("")
                ),
                None => warn!("园区绑定已解除，地图会先入库为设备地图，创建园区时再挑图绑定"),
            }
        }

        "task_dispatch" => {
            let task_code = msg.payload["taskCode"].as_str().unwrap_or("").to_string();
            let task_name = msg.payload["taskName"]
                .as_str()
                .unwrap_or("未命名任务")
                .to_string();
            let task_type = msg.payload["taskType"].as_str().unwrap_or("巡检");
            let mode = TaskMode::parse(task_type);
            info!("收到任务: [{task_code}] {task_name} 类型={task_type} 模式={mode:?}");
            // 排障最有用的一行：把业务侧下发的整份任务载荷原样打出来（含 routeData.points /
            // map / initialPose）。整条链路的信息默认可见：服务端 `task_dispatch 下发载荷`
            // → 客户端这行 → 客户端的 `5000` 完整帧 → 本体的 `5001` / `5009`。
            // （模拟模式下同样会打，用来核对 map / routeData 是否与平台一致）
            info!("task_dispatch 载荷: {}", msg.payload);

            // 本条路线里带「AI 识别」动作的航点：到点由 5009 的 detail_index 查表触发
            let ai_points = parse_ai_points(&msg.payload["routeData"]);
            if !ai_points.is_empty() {
                info!(
                    "任务 {task_code} 含 {} 个 AI 识别航点: {:?}",
                    ai_points.len(),
                    ai_points
                        .iter()
                        .map(|point| (point.index, point.name.as_str()))
                        .collect::<Vec<_>>()
                );
            }
            // 航点灯光（与 AI 动作分开：有点只开灯不识别）+ 计划级「仅夜间自动开灯」
            let light_points = parse_light_points(&msg.payload["routeData"]);
            if !light_points.is_empty() {
                info!(
                    "任务 {task_code} 含 {} 个灯光航点: {:?}",
                    light_points.len(),
                    light_points
                        .iter()
                        .map(|point| (point.index, point.fill, point.led))
                        .collect::<Vec<_>>()
                );
            }
            // 老平台不发 `lightNightOnly`：按「到点就开」跑（历史行为=不开灯）
            let light_night_only = msg.payload["lightNightOnly"].as_bool().unwrap_or(false);
            *active.lock().await = Some(ActiveTask {
                task_code: task_code.clone(),
                task_name: task_name.clone(),
                nav_task_id: None,
                stage: None,
                point_index: None,
                ai_points,
                light_points,
                light_night_only,
            });
            state.lock().await.current_task = Some(task_name.clone());

            let Some(client) = zhpro.clone() else {
                warn!("未连接本体，任务仅记录不执行");
                report_progress(
                    out_tx,
                    &task_code,
                    "异常",
                    0,
                    Some("未连接机器人本体".to_string()),
                    None,
                    None,
                    None,
                )
                .await;
                return;
            };

            // 官方 App 在 sdk_node 未就绪 / 急停未解除 / 控制权被外部抢占 /
            // 未站立时不会创建任务；这里在发 5000 之前统一拦掉，避免
            // “5000 受理但本体不执行，最后等 600s 超时”。
            //
            // 客户端刚启动/刚重连时，平台会立刻补发 pending 任务，而第一条
            // 1315 可能还没到；先等最多 3s，避免把“初始化中”误判成“离线”。
            if !state.lock().await.body_status_fresh() {
                for _ in 0..15 {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    if state.lock().await.body_status_fresh() {
                        break;
                    }
                }
            }
            let task_block = state.lock().await.motion_block_reason(true);
            if let Some(reason) = task_block {
                warn!("任务 {task_code} 拒绝下发: {reason}");
                report_progress(
                    out_tx,
                    &task_code,
                    "异常",
                    0,
                    Some(reason),
                    None,
                    None,
                    None,
                )
                .await;
                clear_active_task(active, state).await;
                return;
            }

            match mode {
                TaskMode::Mapping => {
                    let session = Arc::new(MappingSession::new(client.clone()));
                    *local_mapping.lock().await = Some(session.clone());
                    let kind = match config.map_kind.as_str() {
                        "indoor" => zhpro::protocol::MappingType::Indoor,
                        _ => zhpro::protocol::MappingType::Outdoor,
                    };
                    match session.start(&task_name, kind).await {
                        Ok(task_id) => {
                            report_progress(
                                out_tx,
                                &task_code,
                                "执行中",
                                0,
                                Some("建图任务已下发".to_string()),
                                Some("mapping.preparing".to_string()),
                                Some(task_id),
                                None,
                            )
                            .await;
                        }
                        Err(e) => {
                            error!("建图任务下发失败: {e}");
                            report_progress(
                                out_tx,
                                &task_code,
                                "异常",
                                0,
                                Some(format!("建图任务下发失败: {e}")),
                                None,
                                None,
                                None,
                            )
                            .await;
                            // 建图没起来：别留着「正在建图」的占位，否则进度定时器会拿它刷假状态
                            *local_mapping.lock().await = None;
                            clear_active_task(active, state).await;
                        }
                    }
                }

                TaskMode::ReturnToCharge => {
                    let binding = parse_map_binding(&msg.payload);
                    // 5014 的 `map_id` 是本体目录名（4013 `path` 最后一段），不是业务 ID
                    let manager = MapManager::new(client.clone());
                    let robot_map = match manager
                        .resolve(&[&binding.map_dir, &binding.map_path, &binding.map_id])
                        .await
                    {
                        Ok(map) => map,
                        Err(e) => {
                            warn!("任务 {task_code} 的地图不在本体上，拒绝返充: {e}");
                            report_progress(
                                out_tx,
                                &task_code,
                                "异常",
                                0,
                                Some(e),
                                None,
                                None,
                                None,
                            )
                            .await;
                            clear_active_task(active, state).await;
                            return;
                        }
                    };
                    match client
                        .call(T_RETURN_TO_CHARGE, json!({ "map_id": robot_map.map_id }))
                        .await
                    {
                        Ok(data) => {
                            let robot_task = data
                                .get("task_id")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            if let Some(a) = active.lock().await.as_mut() {
                                a.nav_task_id = robot_task.clone();
                            }
                            report_progress(
                                out_tx,
                                &task_code,
                                "执行中",
                                0,
                                Some("返回充电点任务已受理".to_string()),
                                Some("return_to_charge.preparing".to_string()),
                                robot_task,
                                None,
                            )
                            .await;
                        }
                        Err(e) => {
                            report_progress(
                                out_tx,
                                &task_code,
                                "异常",
                                0,
                                Some(format!("返回充电点失败: {e}")),
                                None,
                                None,
                                None,
                            )
                            .await;
                        }
                    }
                }

                TaskMode::Recharge | TaskMode::Undock => {
                    let kind = if mode == TaskMode::Recharge {
                        TaskKind::Recharge
                    } else {
                        TaskKind::Undock
                    };
                    let data = json!({
                        "kind": kind.code(),
                        "name": task_name,
                        "schedule": 0,
                        "execute_at": now_ms().to_string(),
                        "priority": 0,
                        "record": false,
                        "params": Value::Null
                    });
                    match client.call(T_TASK_CREATE, data).await {
                        Ok(reply) => {
                            let robot_task = reply
                                .get("task_id")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            if let Some(a) = active.lock().await.as_mut() {
                                a.nav_task_id = robot_task.clone();
                            }
                            report_progress(
                                out_tx,
                                &task_code,
                                "执行中",
                                0,
                                Some(format!("{}任务已受理", kind.as_str())),
                                None,
                                robot_task,
                                None,
                            )
                            .await;
                        }
                        Err(e) => {
                            report_progress(
                                out_tx,
                                &task_code,
                                "异常",
                                0,
                                Some(format!("任务下发失败: {e}")),
                                None,
                                None,
                                None,
                            )
                            .await;
                        }
                    }
                }

                TaskMode::Route => {
                    let Some(mut plan) = parse_flight_plan(&msg.payload, &config.map_root) else {
                        warn!("任务 {task_code} 缺少可执行的航点数据");
                        report_progress(
                            out_tx,
                            &task_code,
                            "异常",
                            0,
                            Some("任务缺少航点数据（routeData.points 为空）".to_string()),
                            None,
                            None,
                            None,
                        )
                        .await;
                        clear_active_task(active, state).await;
                        return;
                    };
                    info!(
                        "航线计划: {} 个航点, 地图={} (路径 {}), 结束动作={}",
                        plan.waypoints.len(),
                        plan.map_id,
                        plan.map_path,
                        plan.end_task_label()
                    );
                    // 注意：**不要**在这里把 `active_map` 改成计划里的地图。那个字段表示
                    // 「本体当前真的载着哪张图」，是定位守卫判断「要不要重新载图」的依据；
                    // 提前写进去会让守卫误以为已载好，从而跳过自动重定位。
                    // 定位成功后由 `ensure_localized` 写回。

                    // 上一轮遗留的本体任务必须先收尾：本体对「已有一个未结束任务」回
                    // 13897 resource conflict (capability busy)，而任务被暂停（5004）后
                    // 不会自动结束——平台直接重新下发时第一个 5000 必然被拒。
                    if let Some(previous) = local_runner.lock().await.take() {
                        let had_robot_task = previous.current_nav_task().await.is_some();
                        match previous.control(Control::Cancel).await {
                            Ok(()) => {
                                if had_robot_task {
                                    info!(
                                        "任务 {task_code}：已 5002 取消上一轮本体导航任务，等本体释放导航能力"
                                    );
                                    tokio::time::sleep(Duration::from_secs(2)).await;
                                }
                            }
                            Err(e) => warn!("取消上一轮本体导航任务失败: {e}"),
                        }
                    }

                    // 前置校验：业务 `mapId` 不是本体目录名，必须用 4013 列表解析出真实目录。
                    // 对不上就不发 5000——否则本体只会回 13898 missing map.yaml。
                    let manager = MapManager::new(client.clone());
                    let robot_map = match manager.resolve(&[&plan.map_path, &plan.map_id]).await {
                        Ok(map) => map,
                        Err(e) => {
                            warn!("任务 {task_code} 的地图不在本体上，拒绝下发: {e}");
                            report_progress(
                                out_tx,
                                &task_code,
                                "异常",
                                0,
                                Some(e),
                                None,
                                None,
                                None,
                            )
                            .await;
                            clear_active_task(active, state).await;
                            return;
                        }
                    };
                    info!(
                        "机器人地图就绪: {} 目录={} 路网={} 回充点={}",
                        plan.map_id,
                        robot_map.map_id,
                        robot_map.route_exist,
                        robot_map.charge_point_exist
                    );
                    if plan.end_task() == zhpro::route::EndTask::ReturnToCharge
                        && !robot_map.charge_point_exist
                    {
                        // 官方 App 在 charge_point_exist=false 时直接置灰返充入口；
                        // 这里提前失败，避免跑完整条航线才在 5014 收到 0x3672。
                        warn!("任务 {task_code} 拒绝下发：地图未设置回充点");
                        report_progress(
                            out_tx,
                            &task_code,
                            "异常",
                            0,
                            Some(
                                "该地图未设置回充点（charge_point_exist=false），无法返航充电；\
                                 请先在本体设置回充点（4018）或把结束动作改为「原地」"
                                    .to_string(),
                            ),
                            None,
                            None,
                            None,
                        )
                        .await;
                        clear_active_task(active, state).await;
                        return;
                    }
                    // 用本体上报的真实目录路径（L2/M1 地图根不同，不能靠平台拼）
                    plan.map_path = robot_map.path.clone();

                    // 对齐官方 cruise_mode：只有整条路线都走「沿道路」且本体地图
                    // 有 map.geojson 时才用路网（map_type=3）；否则一律 2D 栅格（1）。
                    // 不能只看服务端 mapType：服务端可能按 road_count 给了 3，
                    // 但这条路线实际是自由导航。`--road-network on/off` 可强制。
                    let target_map_type = config
                        .road_network
                        .map_type(plan.road_network, robot_map.route_exist);
                    info!(
                        "地图类型：本体能力 map_type={target_map_type}（导航枚举 1/3）→ 巡检 \
                         5000 kind=4 会发 {}（0=2D 栅格 / 1=路网）（road_network 策略={}，\
                         航线全沿道路={}，本体 map.geojson={}）",
                        if target_map_type == 3 { 1 } else { 0 },
                        config.road_network.as_str(),
                        plan.road_network,
                        robot_map.route_exist
                    );
                    if config.road_network == RoadNetworkMode::Auto
                        && plan.road_network
                        && !robot_map.route_exist
                        && target_map_type == 1
                    {
                        warn!("航线请求路网导航，但本体地图没有 map.geojson，回退 2D 栅格");
                    }
                    if plan.map_type != target_map_type {
                        info!(
                            "5000 地图类型: 服务端下发 {} → 实际按本体能力用 {target_map_type}",
                            plan.map_type
                        );
                        plan.map_type = target_map_type;
                    }

                    // 官方链路是「1204 取控制权 → 1302 cmd=62 切导航模式 → 选图/定位 → 建任务」。
                    // 巡检（kind=4）由本体自己走 `load_localization_map` + `capture_origin_pose`，
                    // 这里保留一次起步前定位守卫：地图不在本体上 / 载的不是这张图 / 定位丢失时
                    // **自动**载图重定位，无需人工点「重新定位」。
                    if let Err(e) = zhpro::teleop::enter_navigation_mode(&client).await {
                        warn!("任务 {task_code} 切导航模式失败（继续尝试，但本体可能拒任务）: {e}");
                    }
                    let map_path = plan.resolve_map_path(&config.map_root);
                    let initial_pose = parse_initial_pose(msg.payload.get("initialPose"));
                    match ensure_localized(&client, state, &plan.map_id, &map_path, initial_pose)
                        .await
                    {
                        Ok((status, loaded)) => info!(
                            "定位已就绪（status={status}，本次{}载图重定位），开始下发巡检任务\
                             （5000 kind=4：航点与到点动作由本体 nav_paths 里的路径执行）",
                            if loaded { "" } else { "未" }
                        ),
                        Err(e) => {
                            warn!("任务 {task_code} 定位未就绪，拒绝下发: {e}");
                            report_progress(
                                out_tx,
                                &task_code,
                                "异常",
                                0,
                                Some(format!(
                                    "定位未就绪：{e}（已自动重定位 {LOCATE_ATTEMPTS} 次）。\
                                     请远程接管把设备移回地图覆盖范围，或下发「重新定位」\
                                     并给出初始位姿后重试"
                                )),
                                None,
                                None,
                                None,
                            )
                            .await;
                            clear_active_task(active, state).await;
                            return;
                        }
                    }

                    // 软件急停 / sdk_node / ctrl_source / 卧倒姿态已由上面的
                    // `motion_block_reason(true)` 统一拦过，这里不再重复检查
                    let runner = Arc::new(RouteRunner::new(client.clone()));
                    // 巡检下发前用它和 min_recharge_battery 做低电守卫（双电池取较低那节）；
                    // 先取出来再 await，避免把 state 的锁带过 await 点
                    let battery_percent = state.lock().await.battery_min;
                    runner.set_battery_percent(battery_percent).await;
                    runner
                        .set_patrol_options(&config.patrol_path_prefix, config.min_recharge_battery)
                        .await;
                    // 巡检的总点数 = 路径点数（本体按 nav_paths 里的点算，不做「闭环补点」）
                    let total_points = plan.waypoints.len() as i64;
                    runner.prepare(plan).await;
                    *local_runner.lock().await = Some(runner.clone());

                    let out = out_tx.clone();
                    let code = task_code.clone();
                    let state = state.clone();
                    let active = active.clone();
                    let runner_slot = local_runner.clone();
                    let ai = ai.clone();
                    let lights = lights.clone();
                    tokio::spawn(async move {
                        let outcome = runner.run().await;
                        let (status, message) = match outcome {
                            Ok(o) if o.success => (
                                "已完成",
                                format!(
                                    "{}（{}/{} 航点）",
                                    o.message, o.completed_points, o.total_points
                                ),
                            ),
                            Ok(o) => ("异常", format!("{}（{}）", o.message, o.skipped)),
                            Err(e) => ("异常", format!("执行失败: {e}")),
                        };
                        report_progress(
                            &out,
                            &code,
                            status,
                            100,
                            Some(message),
                            None,
                            None,
                            Some(total_points),
                        )
                        .await;
                        // 本地跑的路子同样要在任务收尾时清掉整个计划的板端存证
                        ai.finish_plan(&code, "本地任务结束").await;
                        // 本地跑不一定会走到 5009 的「离开航点」，这里直接收灯
                        lights.leave("本地任务结束").await;
                        // 收尾必须把槽位里的 runner 摘掉：业务侧的下发地图 / 继续任务等
                        // 门禁都看这个槽位，残留的 runner 会让设备在任务早已结束（甚至
                        // 取消）之后仍报「设备正在执行任务」（2026-11-19 现场事故）。
                        clear_local_runner(&runner_slot, &runner).await;
                        *active.lock().await = None;
                        state.lock().await.current_task = None;
                    });
                }
            }
        }

        "task_control" => {
            let action = msg.payload["action"].as_str().unwrap_or("unknown");
            let task_code = msg.payload["taskCode"].as_str().unwrap_or("");
            let reason = msg.payload["reason"].as_str().unwrap_or("");
            info!(
                "任务控制: {task_code} → {action}{}",
                if reason.is_empty() {
                    String::new()
                } else {
                    format!("（{reason}）")
                }
            );
            let control = match action {
                "pause" => Some(Control::Pause),
                "resume" => Some(Control::Resume),
                "cancel" => Some(Control::Cancel),
                _ => None,
            };
            let Some(control) = control else {
                warn!("未知控制指令: {action}");
                return;
            };
            // resume 只对「本地正在跑的任务」有意义：平台用「执行中」触发 resume 时，
            // 如果本地没有对应任务的执行上下文（客户端重启过 / 平台只发了 resume
            // 没发 task_dispatch），回一条假的「执行中」会让平台任务永久卡住。
            if action == "resume" {
                let has_context = active
                    .lock()
                    .await
                    .as_ref()
                    .map(|task| task.task_code == task_code)
                    .unwrap_or(false)
                    || local_runner.lock().await.is_some()
                    || local_mapping.lock().await.is_some();
                if !has_context {
                    warn!("任务 {task_code} 本地没有执行上下文，拒绝 resume");
                    report_progress(
                        out_tx,
                        task_code,
                        "异常",
                        0,
                        Some("本地没有该任务的执行上下文（任务未下发或客户端已重启），请从控制台重新下发".to_string()),
                        None,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            }
            if let Some(runner) = local_runner.lock().await.clone()
                && let Err(e) = runner.control(control).await
            {
                warn!("任务控制失败: {e}");
            }
            // 建图任务的取消走建图会话
            if action == "cancel" {
                if let Some(session) = local_mapping.lock().await.take() {
                    let saved = session.stop(Duration::from_secs(30)).await;
                    match saved {
                        Some(s) if s.success => {
                            info!("建图已停止并保存成功");
                        }
                        Some(s) => warn!("建图停止，保存失败: {}", s.message),
                        None => warn!("建图停止，未收到 4001"),
                    }
                }
                // 任务取消：识别窗口没有继续的意义（Agent 单路，别占着）
                ai.stop("任务已取消").await;
                // 灯光也是：取消后狗不再跑这条路线，灯必须当场收掉
                lights.leave("任务已取消").await;
                // 取消也是计划收尾：板端整个计划目录都不需要了（理由同终态那条）
                ai.finish_plan(task_code, "任务已取消").await;
                *active.lock().await = None;
                state.lock().await.current_task = None;
            }
            let status = match control {
                Control::Pause => "已暂停",
                Control::Resume => "执行中",
                Control::Cancel => "异常",
            };
            report_progress(
                out_tx,
                task_code,
                status,
                0,
                Some(format!("任务控制: {action}")),
                None,
                None,
                None,
            )
            .await;
        }

        "command" => {
            let command = msg.payload["command"].as_str().unwrap_or("unknown");
            info!("收到远程指令: {command}");

            // 「本体重连」：进程内重建 control/bulkdata，不退出进程、业务 WebSocket 不断。
            // 放在所有门禁之前：本体已经断链时（`zhpro` 为空）也要能触发重连。
            // 只在空闲时执行：带任务/建图/接管重建会丢运行态。
            if command == "本体重连" {
                let refused = {
                    let engaged = zhpro::teleop::is_engaged(teleop).await;
                    let busy_task = active.lock().await.is_some();
                    let busy_mapping = local_mapping.lock().await.is_some();
                    if engaged {
                        Some("当前处于远程接管中，请先「释放接管」")
                    } else if busy_task {
                        Some("有任务在执行，请先暂停/取消任务")
                    } else if busy_mapping {
                        Some("建图会话进行中，请先结束建图")
                    } else {
                        None
                    }
                };
                if let Some(reason) = refused {
                    warn!("「本体重连」被拒绝: {reason}");
                    let event = DeviceEventPayload {
                        event_type: "error".to_string(),
                        message: format!("重连本体被拒绝: {reason}"),
                        data: None,
                    };
                    send(
                        out_tx,
                        "event",
                        serde_json::to_value(event).unwrap_or_default(),
                    )
                    .await;
                    return;
                }

                // 先补一帧停车并清接管态/摇杆缓存，避免带着非零摇杆重建
                // （旧链已断时 `prepare_body_reconnect` 里还会再做一次，都是幂等的）
                if let Some(client) = zhpro.as_ref() {
                    zhpro::teleop::reset_after_control_lost(client, teleop).await;
                }
                let event = DeviceEventPayload {
                    event_type: "warn".to_string(),
                    message: "收到「本体重连」：正在原地重建到本体的连接（约 5~15 秒）。\
                              客户端不退出、平台连接保持，期间设备会短暂显示「设备初始化中」"
                        .to_string(),
                    data: None,
                };
                send(
                    out_tx,
                    "event",
                    serde_json::to_value(event).unwrap_or_default(),
                )
                .await;
                // 交给主循环：与「链路意外断开」走同一条重建路径（退避、接管态清理、
                // 任务置异常、窗口内失败后交 systemd 兜底）
                if body_reconnect.try_send(()).is_err() {
                    warn!("本体重连请求未入队（已有一次重连在进行中）");
                }
                return;
            }

            // 「同步媒体」只动板子自己的磁盘与上行网络，不碰本体：本体没连（关机 /
            // 断链）也应该能把已经采好的巡检图传回中台，所以放在本体门禁之前。
            if command == "同步媒体" {
                let outer = msg.payload.get("params");
                // 进度上报：一轮同步在 4G 上可能跑几分钟，中台设备页要能看到
                // 「已上传 8/24」而不是干等 final 的 command_result。
                let progress_tx = out_tx.clone();
                let progress_request_id = command_request_id(&msg);
                let progress = move |p: media_sync::SyncProgress| {
                    let Some(request_id) = progress_request_id.clone() else {
                        return;
                    };
                    let _ = progress_tx.try_send(WsMessage {
                        msg_type: "command_progress".to_string(),
                        ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                        payload: json!({
                            "requestId": request_id,
                            "command": "同步媒体",
                            "uploaded": p.uploaded,
                            "failed": p.failed,
                            "total": p.total,
                            "batchesDone": p.batches_done,
                            "batchesTotal": p.batches_total,
                        }),
                    });
                };
                let summary = media_sync::sync(outer, config, Some(&progress)).await;
                // 结构化结果比纯文案好判：中台要区分「目录已清空」与「还剩几张」，
                // 前端才能把「完成」和「部分完成 / 再点一次」分开显示。
                let result: Result<String, String> = match &summary {
                    Ok(summary) => Ok(summary.to_string()),
                    Err(error) => Err(error.clone()),
                };
                let extra = match &summary {
                    Ok(summary) => json!({
                        "drained": summary.drained(),
                        "scanned": summary.scanned,
                        "uploaded": summary.uploaded,
                        "existing": summary.existing,
                        "failed": summary.failed,
                        "deleted": summary.deleted,
                        "remaining": summary.remaining,
                    }),
                    Err(_) => json!({}),
                };
                send_command_result_with(out_tx, &msg, command, &result, extra).await;
                match result {
                    Ok(note) => info!("指令 {command} 执行: {note}"),
                    Err(error) => {
                        warn!("指令 {command} 执行失败: {error}");
                        let event = DeviceEventPayload {
                            event_type: "error".to_string(),
                            message: format!("指令 {command} 执行失败: {error}"),
                            data: None,
                        };
                        send(
                            out_tx,
                            "event",
                            serde_json::to_value(event).unwrap_or_default(),
                        )
                        .await;
                    }
                }
                return;
            }

            // 对齐官方 App 的 ready 门禁：恢复类动作（急停/解除急停/避障）
            // 永远放行；运动/任务类动作必须先过 sdk_node、急停、控制权、
            // 姿态检查，避免“平台显示已下发、本体直接拒收”。
            let action = msg.payload["params"]["action"]
                .as_str()
                .or_else(|| msg.payload["action"].as_str())
                .unwrap_or("");
            let recovery = matches!(command, "急停" | "解除急停" | "避障开启" | "避障关闭")
                || matches!(
                    action,
                    "emergency_stop" | "clear_emergency_stop" | "avoid_on" | "avoid_off"
                );
            let motion_command = matches!(
                command,
                "站立"
                    | "卧倒"
                    | "通用模式"
                    | "原地模式"
                    | "导航模式"
                    | "开始巡检"
                    | "暂停任务"
                    | "继续任务"
                    | "召回充电"
                    | "重新定位"
                    | "下发地图"
            ) || matches!(
                action,
                "stand_up" | "lie_down" | "generic_mode" | "in_place_mode" | "navigation_mode"
            );
            // 强制接管标记：遥控器关了但本体仍报 `ctrl_source = 3` 时，平台会带
            // `force=true` 下发「远程接管」。客户端据此只放行 ctrl_source 这一条
            // 门禁（见 [`RobotState::motion_block_reason_with`]），其余门禁不变。
            let force = msg.payload["force"]
                .as_bool()
                .or_else(|| msg.payload["params"]["force"].as_bool())
                .unwrap_or(false);
            match command {
                "远程接管" => {
                    state.lock().await.teleop_force = force;
                    if force {
                        warn!("平台强制接管：暂时忽略 ctrl_source 抢占门禁（请确认遥控器已断电）");
                    }
                }
                "释放接管" => state.lock().await.teleop_force = false,
                _ => {}
            }
            if !recovery && motion_command {
                let require_posture =
                    matches!(
                        command,
                        "通用模式"
                            | "原地模式"
                            | "导航模式"
                            | "开始巡检"
                            | "暂停任务"
                            | "继续任务"
                            | "召回充电"
                    ) || matches!(action, "generic_mode" | "in_place_mode" | "navigation_mode");
                // 强制接管只放行手动动作（站立/卧倒/模式切换），任务与导航类指令
                // 仍然要按 `ctrl_source = 3` 拒绝：本体在遥控器手里时导航能力不可用。
                let manual_action = matches!(
                    command,
                    "站立" | "卧倒" | "通用模式" | "原地模式" | "导航模式"
                ) || matches!(
                    action,
                    "stand_up" | "lie_down" | "generic_mode" | "in_place_mode" | "navigation_mode"
                );
                let command_block = {
                    let guard = state.lock().await;
                    let allow_preempt = guard.teleop_force && manual_action;
                    guard.motion_block_reason_with(require_posture, allow_preempt)
                };
                if let Some(reason) = command_block {
                    warn!("指令 {command} 被拒绝: {reason}");
                    // 门禁拒绝也必须回执，否则平台只能等到超时才知道没执行
                    send_command_result(out_tx, &msg, command, &Err(reason.clone())).await;
                    let event = DeviceEventPayload {
                        event_type: "error".to_string(),
                        message: format!("指令 {command} 被拒绝: {reason}"),
                        data: None,
                    };
                    send(
                        out_tx,
                        "event",
                        serde_json::to_value(event).unwrap_or_default(),
                    )
                    .await;
                    return;
                }
            }

            let Some(client) = zhpro.clone() else {
                warn!("未连接本体，忽略远程指令");
                send_command_result(
                    out_tx,
                    &msg,
                    command,
                    &Err("本体未连接（客户端正在重连或本体离线），指令未执行".to_string()),
                )
                .await;
                return;
            };
            // 遥控类指令（接管 / 释放 / 急停 / 本体动作）由 teleop 模块处理，
            // 其余是业务任务指令，仍走原来的路径
            let teleop_result =
                zhpro::teleop::handle_command(&client, teleop, command, Some(&msg.payload)).await;

            let result: Result<String, String> = match teleop_result {
                Some(result) => result,
                None => match command {
                    // 暂停 / 继续必须用**真实的**本体任务 ID：固件不认 `task_id: "current"`，
                    // 会回 13896 task not found（之前这两个按钮实际上什么都没做）。
                    "暂停任务" | "继续任务" => {
                        let control = if command == "暂停任务" {
                            Control::Pause
                        } else {
                            Control::Resume
                        };
                        let runner = local_runner.lock().await.clone();
                        match runner {
                            None => Err("本地没有正在执行的航线任务（可能未下发或客户端已重启）；\
                                 请在控制台重新下发任务"
                                .to_string()),
                            Some(runner) => match runner.control(control).await {
                                Ok(()) => Ok(if command == "暂停任务" {
                                    "已请求暂停".to_string()
                                } else {
                                    "已请求继续".to_string()
                                }),
                                Err(e) => Err(e),
                            },
                        }
                    }
                    "召回充电" => {
                        let map_id = state.lock().await.active_map.clone().unwrap_or_default();
                        // 5014 同样在非白名单区间，没控制权会被网关丢弃
                        if let Err(e) = client.ensure_control().await {
                            warn!("取 RoamerX 控制权失败（5014 可能被网关丢弃）: {e}");
                        }
                        client
                            .call(T_RETURN_TO_CHARGE, json!({ "map_id": map_id }))
                            .await
                            .map(|_| "已请求返回充电点".to_string())
                    }
                    "同步地图" => {
                        // 服务端在指令里带上了台账当前园区归属（`params.params.parkId`），
                        // 显式 null 表示设备确实没绑园区，不能拿本地缓存的旧值充数
                        let outer = msg.payload.get("params");
                        let params = outer.and_then(|p| p.get("params")).or(outer);
                        let park_hint = params
                            .and_then(|p| p.get("parkId"))
                            .map(|value| value.as_i64());
                        // 刷新语义由服务端 `mode` / `refresh` / `onlyMissing` 共同决定：
                        // 中台按钮（manual）要求全量刷新，auto / onlyMissing 只补缺。
                        let mode = params
                            .and_then(|p| p.get("mode"))
                            .and_then(Value::as_str)
                            .unwrap_or("manual");
                        let refresh = params
                            .and_then(|p| p.get("refresh"))
                            .and_then(Value::as_bool)
                            .unwrap_or_else(|| {
                                let only_missing = params
                                    .and_then(|p| p.get("onlyMissing"))
                                    .and_then(Value::as_bool);
                                only_missing.map(|only| !only).unwrap_or(mode != "auto")
                            });
                        sync_maps_from_robot(config, &client, out_tx, park_hint, refresh, true)
                            .await
                    }
                    "下发地图" => {
                        let result = push_map_to_body(
                            config,
                            &client,
                            state,
                            local_runner,
                            local_mapping,
                            &msg,
                        )
                        .await;
                        // 成功也要留一条设备事件：中台审计只知道「已下发」，
                        // 真正写本体 + 重新载图的结果要看这里。
                        if let Ok(note) = &result {
                            let event = DeviceEventPayload {
                                event_type: "info".to_string(),
                                message: note.clone(),
                                data: None,
                            };
                            send(
                                out_tx,
                                "event",
                                serde_json::to_value(event).unwrap_or_default(),
                            )
                            .await;
                        }
                        result
                    }
                    "重新定位" => {
                        // 服务端把指令附加参数包在 `params.params`（见 server 的 push_command），
                        // 这里兼容直接平铺在 `params` 上的写法
                        let outer = msg.payload.get("params");
                        let params = outer.and_then(|p| p.get("params")).or(outer);
                        let map_id = params
                            .and_then(|p| p.get("mapId"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or(state.lock().await.active_map.clone());
                        let pose = parse_initial_pose(params.and_then(|p| p.get("pose")));
                        match map_id {
                            None => Err(
                                "未指定 mapId，且机器人当前没有已载入地图；请在设备上指定地图后重试"
                                    .to_string(),
                            ),
                            Some(map_id) => {
                                let map_dir_hint = params
                                    .and_then(|p| p.get("mapDir"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                // 4030 的 map_path 必须是本体上真实存在的目录
                                let manager = MapManager::new(client.clone());
                                match manager.resolve(&[&map_dir_hint, &map_id]).await {
                                    Err(e) => Err(format!("重定位失败：{e}")),
                                    Ok(robot_map) => {
                                        let map_path = robot_map.path.clone();
                                        info!(
                                            "重新定位：map={map_id} 目录={} 路径={map_path} 初始位姿={}",
                                            robot_map.map_id,
                                            if pose.is_some() {
                                                "平台指定"
                                            } else {
                                                "用上次位姿文件"
                                            }
                                        );
                                        match zhpro::mapping::relocalize(
                                            &client,
                                            &map_path,
                                            pose,
                                            LOCATE_TIMEOUT,
                                        )
                                        .await
                                        {
                                            Ok(status) => {
                                                state.lock().await.active_map =
                                                    Some(map_id.clone());
                                                // 重定位完就切导航模式：否则后续 5000 会被回
                                                // 13897 capability busy（本体仍在手动模式）。
                                                if let Err(e) =
                                                    zhpro::teleop::enter_navigation_mode(&client)
                                                        .await
                                                {
                                                    warn!("重定位后切导航模式失败: {e}");
                                                }
                                                Ok(format!(
                                                    "重定位完成：{map_id}（status={status}）"
                                                ))
                                            }
                                            Err(e) => Err(format!("重定位失败：{e}")),
                                        }
                                    }
                                }
                            }
                        }
                    }
                    other => Err(format!("未知指令 {other}")),
                },
            };
            // 先把真实结果回报给业务平台（按 requestId 关联），再落本地日志/事件。
            send_command_result(out_tx, &msg, command, &result).await;
            match result {
                Ok(note) => info!("指令 {command} 执行: {note}"),
                Err(e) => {
                    warn!("指令 {command} 执行失败: {e}");
                    let event = DeviceEventPayload {
                        event_type: "error".to_string(),
                        message: format!("指令 {command} 执行失败: {e}"),
                        data: None,
                    };
                    send(
                        out_tx,
                        "event",
                        serde_json::to_value(event).unwrap_or_default(),
                    )
                    .await;
                }
            }
        }

        // 本体链路断开前客户端会上报一帧 `teleop_state`（holder=false），服务端回这个 ack；
        // 只是确认收到，不需要处理
        "teleop_state_ack" => {}

        "teleop" => {
            let Some(client) = zhpro.clone() else {
                return;
            };
            // 官方 App 的 JoystickOutboundGate：只有 ready + RoamerX 才放 1304。
            // 不满足时先补一帧停车，避免最后一帧非零摇杆继续生效。
            let teleop_block = {
                let guard = state.lock().await;
                let allow_preempt = guard.teleop_force;
                guard.motion_block_reason_with(true, allow_preempt)
            };
            if let Some(reason) = teleop_block {
                zhpro::teleop::send_stop(&client).await;
                notify_teleop_blocked(state, out_tx, &reason).await;
                return;
            }
            // 门禁通过：清掉上次拒绝原因，下一次拒绝立刻重新上报
            clear_teleop_blocked(state).await;
            if let Err(e) = zhpro::teleop::handle_teleop(&client, teleop, &msg.payload).await {
                warn!("遥控摇杆处理失败: {e}");
                notify_teleop_blocked(state, out_tx, &format!("接管失败：{e}")).await;
            }
        }

        "error" => {
            error!(
                "业务服务返回错误: {}",
                msg.payload["message"].as_str().unwrap_or("未知错误")
            );
        }

        // 服务端链路保活探针：WS 层已用 Ping/Pong 维持连接，这里忽略载荷
        "ping" => {}

        other => warn!("未处理的消息类型: {other}"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn report_progress(
    out_tx: &mpsc::Sender<WsMessage>,
    task_code: &str,
    status: &str,
    progress: i64,
    message: Option<String>,
    stage: Option<String>,
    robot_task_id: Option<String>,
    total_points: Option<i64>,
) {
    let payload = TaskProgressPayload {
        task_code: task_code.to_string(),
        status: status.to_string(),
        progress,
        point_index: None,
        waypoint_index: None,
        message,
        stage,
        robot_task_id,
        total_points,
    };
    // 一次性上报（定位未就绪 / 执行失败 / 航点跑完）：没有历史可比，直接发并打日志
    ProgressReporter::default()
        .report(out_tx, &payload, "事件")
        .await;
}

/// 摇杆未放行（门禁拒绝 / 接管失败）：按「原因变化或 30s」限频上报一条 `warn` 事件
///
/// 以前只打 `debug!`，平台侧完全看不到原因，现场只能看到「页面显示已接管、狗子不动」。
/// 用 `warn` 而不是 `error`：门禁拒绝不是本体故障，服务端不应因此生成告警工单。
async fn notify_teleop_blocked(
    state: &Shared<RobotState>,
    out_tx: &mpsc::Sender<WsMessage>,
    reason: &str,
) {
    let changed = {
        let mut guard = state.lock().await;
        let changed = guard
            .teleop_block_notice
            .as_ref()
            .map(|(last, at)| last != reason || at.elapsed() > TELEOP_BLOCK_NOTICE_INTERVAL)
            .unwrap_or(true);
        if changed {
            guard.teleop_block_notice = Some((reason.to_string(), Instant::now()));
        }
        changed
    };
    if !changed {
        debug!("摇杆未放行（同原因已在窗口内上报）: {reason}");
        return;
    }
    warn!("摇杆未放行: {reason}");
    let event = DeviceEventPayload {
        event_type: "warn".to_string(),
        message: format!("摇杆被拒绝: {reason}"),
        data: Some(json!({ "blocked": true })),
    };
    send(
        out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
}

/// 摇杆重新放行：清掉上次的拒绝原因
async fn clear_teleop_blocked(state: &Shared<RobotState>) {
    state.lock().await.teleop_block_notice = None;
}

async fn send(tx: &mpsc::Sender<WsMessage>, msg_type: &str, payload: Value) {
    let msg = WsMessage {
        msg_type: msg_type.to_string(),
        ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        payload,
    };
    let _ = tx.send(msg).await;
}

/// 从平台下发指令里取 `requestId`。
///
/// 两种形态都要认：
/// - 「下发地图」这类由业务平台自己组参数：`params.requestId`；
/// - 中台转发过来的普通指令走 `push_command`，业务平台会把中台参数**再包一层**：
///   `params.params.requestId`（与 `media_sync::parse_params` 兼容的两层一致）。
///   少了这一层，`同步媒体` 的进度/结论回执会因为取不到 requestId 直接不发，
///   中台审计就一直停在「已下发」。
fn command_request_id(msg: &WsMessage) -> Option<String> {
    let params = msg.payload.get("params");
    params
        .and_then(|params| params.get("params"))
        .and_then(|params| params.get("requestId"))
        .or_else(|| params.and_then(|params| params.get("requestId")))
        .or_else(|| msg.payload.get("requestId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// 回报远程指令的**真实执行结果**。
///
/// 平台只看 `wsDelivered` 无法区分「指令进了设备连接」和「设备真的执行成功」；
/// 业务平台按 `requestId` 等待这条回执，用它决定地图下发成功还是失败。
async fn send_command_result(
    out_tx: &mpsc::Sender<WsMessage>,
    msg: &WsMessage,
    command: &str,
    result: &Result<String, String>,
) {
    send_command_result_with(out_tx, msg, command, result, json!({})).await;
}

/// 同上，带结构化细节（响应目录是否已清空等）。
async fn send_command_result_with(
    out_tx: &mpsc::Sender<WsMessage>,
    msg: &WsMessage,
    command: &str,
    result: &Result<String, String>,
    extra: Value,
) {
    let Some(request_id) = command_request_id(msg) else {
        return;
    };
    let (ok, message) = match result {
        Ok(note) => (true, note.clone()),
        Err(error) => (false, error.clone()),
    };
    let mut payload = json!({
        "requestId": request_id,
        "command": command,
        "ok": ok,
        "message": message,
    });
    if let (Some(payload), Value::Object(extra)) = (payload.as_object_mut(), extra) {
        for (key, value) in extra {
            payload.insert(key, value);
        }
    }
    send(out_tx, "command_result", payload).await;
}

/// 上报一条「地图保存」事件（信息流留痕，与任务终态分开）
async fn send_map_saved_event(
    out_tx: &mpsc::Sender<WsMessage>,
    ok: bool,
    message: impl AsRef<str>,
    map_path: Option<String>,
) {
    let message = message.as_ref();
    let event = DeviceEventPayload {
        event_type: if ok { "info" } else { "error" }.to_string(),
        message: format!(
            "地图保存{}: {}{}",
            if ok { "成功" } else { "失败" },
            message,
            map_path
                .as_deref()
                .map(|p| format!(" → {p}"))
                .unwrap_or_default()
        ),
        data: Some(json!({
            "mapPath": map_path,
            "success": ok,
        })),
    };
    send(
        out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
}

/// 建图完成后把地图回传中台（A 方案：本体取包 → 业务服务导流）
async fn upload_mapping_to_zentai(config: &Config, saved: &MappingSaved) -> UploadOutcome {
    if !config.upload_map {
        return UploadOutcome::Skipped("已用 --no-upload-map 关闭".to_string());
    }
    // 园区是可选的：地图只跟设备有关系，设备没绑园区也照样回传，
    // 设备绑定人之后创建园区时再从自己的设备地图里挑一张绑定。
    let park_id = match config.park_id {
        Some(park_id) => Some(park_id),
        // 未显式指定时，使用注册回执里的园区绑定
        None => *config.bound_park_id.lock().await,
    };
    let Some(map_dir) = saved.dir_name() else {
        return UploadOutcome::Failed {
            reason: "4001 未返回地图目录名".to_string(),
        };
    };

    let robot_host = config.robot.clone().unwrap_or_default();
    let file_base = mapupload::resolve_file_base(
        &robot_host,
        config.robot_file_base.as_deref(),
        config.robot_file_port,
    );
    let http = mapupload::build_http_client();

    info!("开始回传地图 {map_dir}，从 {file_base} 取包");
    let package =
        match mapupload::fetch_map_package(&http, &file_base, &map_dir, saved.map_path.as_deref())
            .await
        {
            Ok(bytes) => bytes,
            Err(e) => {
                return UploadOutcome::Failed {
                    reason: format!("{e}；本体若未开放文件服务，请用 --robot-file-base 指定"),
                };
            }
        };

    let ingest = IngestConfig {
        api_base: config.resolve_api_base(),
        username: config.api_username.clone(),
        password: config.api_password().to_string(),
        device_id: config.device_id.clone(),
        park_id,
        map_name: config.ingest_map_name.clone(),
        bind: park_id.is_some(),
        // 4001 自动回传只新增，永远不覆盖已经入库的地图
        refresh: false,
    };
    // 4001 偶尔只给目录名不给绝对路径；补一个与 4012 一致的文件服务路径，
    // 保证自动回传与手动同步使用同一个 sourceRef 去重。
    let mut saved = saved.clone();
    if saved.map_path.is_none() {
        saved.map_path = Some(format!(
            "{}/history_map/{}",
            config.map_root.trim_end_matches('/'),
            map_dir
        ));
    }
    mapupload::ingest_map(&http, &ingest, &saved, &package).await
}

/// 任务收尾时把 `local_runner` 槽位里的这一份摘掉（只摘自己）。
///
/// 用的是 `Arc::ptr_eq` 而不是无条件 `= None`：任务结束的瞬间平台可能已经下发了
/// 下一轮任务（槽位里换成了新 runner），无条件清空会误伤新任务。
async fn clear_local_runner(
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    finished: &Arc<RouteRunner>,
) {
    let mut slot = local_runner.lock().await;
    if slot
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, finished))
    {
        *slot = None;
    }
}

/// 「下发地图」的忙碌门禁：只有**真的在跑**的任务 / 建图才拒绝。
///
/// 不要退回去看 `local_runner.lock().await.is_some()`：槽位里可能残留着上一轮的
/// runner（任务取消 / 执行失败后没摘干净），那时平台侧任务早已是「异常/已完成」，
/// 但设备会永远回「设备正在执行任务，请先暂停/结束任务后再下发地图」。
/// 2026-11-19 现场 `0202222672400230` 连续 6 次下发地图失败就是踩在这里（13 号任务
/// 已于 16:13:48 取消，设备状态「在线待命」，下发地图仍被拒）。
async fn map_push_block(
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    local_mapping: &Shared<Option<Arc<MappingSession>>>,
) -> Option<String> {
    if local_runner
        .lock()
        .await
        .as_ref()
        .is_some_and(|runner| runner.is_running())
    {
        return Some("设备正在执行任务，请先暂停/结束任务后再下发地图".to_string());
    }
    if local_mapping.lock().await.is_some() {
        return Some("设备正在建图，请先结束建图后再下发地图".to_string());
    }
    None
}

/// 中台设备页「同步地图」：4012 拉取地图列表，只下载业务库缺失的地图并回传。
///
/// 不要求设备绑定园区：地图跟着设备走，园区在控制台创建时再挑图。
/// 中台编辑地图后的「下发地图」：拉业务平台最新包 → 写本体文件服务 → 重新载图。
///
/// 本体上没有「上传地图」协议，只能通过文件服务写文件；写之前先确认：
/// 1. 设备当前没有执行任务 / 建图（避免改图与导航互相打架）；
/// 2. 本体上确实存在该地图目录（不能凭名字造目录）；
/// 3. 写完 `map_edit/map.pgm` / `map_edit/map.yaml`（有编辑目录时）或根目录
///    `map.pgm` / `map.yaml`（老固件）后读回校验，内容一致才算成功；
/// 4. 最后 4030 重新载图，让导航立刻用上新栅格。
async fn push_map_to_body(
    config: &Config,
    client: &Arc<ZhProClient>,
    state: &Shared<RobotState>,
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    local_mapping: &Shared<Option<Arc<MappingSession>>>,
    msg: &WsMessage,
) -> Result<String, String> {
    if let Some(block) = map_push_block(local_runner, local_mapping).await {
        return Err(block);
    }

    let outer = msg.payload.get("params");
    let params = outer.and_then(|p| p.get("params")).or(outer);
    let map_id = params
        .and_then(|p| p.get("mapId"))
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_i64().map(|id| id.to_string()))
        })
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "下发地图缺少 mapId".to_string())?;
    let map_dir_hint = params
        .and_then(|p| p.get("mapDir"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let source_ref_hint = params
        .and_then(|p| p.get("sourceRef"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let dir_hint = if map_dir_hint.trim().is_empty() {
        source_ref_hint
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    } else {
        map_dir_hint
    };

    let http = mapupload::build_http_client();
    let ingest = IngestConfig {
        api_base: config.resolve_api_base(),
        username: config.api_username.clone(),
        password: config.api_password().to_string(),
        device_id: config.device_id.clone(),
        park_id: None,
        map_name: None,
        bind: false,
        // 写本体不经过 ingest，字段只为满足结构体
        refresh: false,
    };
    let package = mapupload::download_map_package(&http, &ingest, &map_id).await?;
    info!(
        "已下载业务平台最新地图包: map={map_id} {} 字节",
        package.len()
    );

    // 本体目录名以 4013 列表为准（平台给的目录名只作候选）
    let manager = MapManager::new(client.clone());
    let robot_map = manager
        .resolve(&[&dir_hint, &map_id])
        .await
        .map_err(|error| {
            format!("本体上没有对应地图目录（候选 {dir_hint} / {map_id}）：{error}")
        })?;

    let file_base = mapupload::resolve_file_base(
        config.robot.as_deref().unwrap_or_default(),
        config.robot_file_base.as_deref(),
        config.robot_file_port,
    );
    let summary =
        mapupload::push_map_to_robot(&http, &file_base, &robot_map.map_id, &package).await?;

    // 重新载图，让本体导航立刻使用新栅格
    let status = zhpro::mapping::relocalize(client, &robot_map.path, None, LOCATE_TIMEOUT)
        .await
        .map_err(|error| format!("{summary}；但重新载图失败：{error}"))?;
    state.lock().await.active_map = Some(map_id.clone());
    if let Err(error) = zhpro::teleop::enter_navigation_mode(client).await {
        warn!("下发地图后切导航模式失败: {error}");
    }
    Ok(format!(
        "{summary}；已重新载图加载最新地图（{map_id}，status={status}）"
    ))
}

/// 同步本体地图到平台。
///
/// - `refresh`：`true` = 中台/控制台显式「同步地图」，要求把本体最新图全部刷上去（内容未变的仍会跳过）；
///   `false` = 设备定时轮询，只补平台没有的图，**绝不覆盖已有记录**。
/// - `announce`：为 `false` 时不往事件流里发「无事可做」汇总，避免轮询每十分钟刷一条；
///   新增/更新/失败仍然会发，运维能看到实际动作。
async fn sync_maps_from_robot(
    config: &Config,
    client: &Arc<ZhProClient>,
    out_tx: &mpsc::Sender<WsMessage>,
    park_hint: Option<Option<i64>>,
    refresh: bool,
    announce: bool,
) -> Result<String, String> {
    // 服务端下发的园区优先；没有就用本地配置 / 注册回执里的绑定
    let park_id = match park_hint {
        Some(park_id) => park_id,
        None => match config.park_id {
            Some(park_id) => Some(park_id),
            None => *config.bound_park_id.lock().await,
        },
    };
    let robot_host = config.robot.clone().unwrap_or_default();
    let file_base = mapupload::resolve_file_base(
        &robot_host,
        config.robot_file_base.as_deref(),
        config.robot_file_port,
    );
    let http = mapupload::build_http_client();
    let maps = zhpro::mapping::MapManager::new(client.clone())
        .list()
        .await
        .map_err(|e| format!("4012 获取地图列表失败: {e}"))?;
    info!(
        "{}同步地图：本体共有 {} 张地图",
        if refresh {
            "中台触发全量刷新"
        } else {
            "定时增量"
        },
        maps.len()
    );

    let ingest = IngestConfig {
        api_base: config.resolve_api_base(),
        username: config.api_username.clone(),
        password: config.api_password().to_string(),
        device_id: config.device_id.clone(),
        park_id,
        map_name: None,
        bind: false,
        refresh,
    };
    let report = mapupload::sync_robot_maps(&http, &ingest, &file_base, &maps).await;
    let summary = report.summary();
    if announce || report.has_changes() {
        let event = DeviceEventPayload {
            event_type: if report.failed > 0 { "error" } else { "info" }.to_string(),
            message: summary.clone(),
            data: Some(json!({
                "total": report.total,
                "uploaded": report.uploaded,
                "skipped": report.skipped,
                "unchanged": report.unchanged,
                "failed": report.failed,
                "mode": if refresh { "refresh" } else { "incremental" },
            })),
        };
        send(
            out_tx,
            "event",
            serde_json::to_value(event).unwrap_or_default(),
        )
        .await;
    } else {
        debug!("定时同步无变化: {summary}");
    }
    if report.failed > 0 {
        Err(summary)
    } else {
        Ok(summary)
    }
}

#[cfg(test)]
mod body_reconnect_tests {
    use super::*;

    /// 业务连接重连计数：跑得久的连接断了 → 退避归 1；刚连上就断 → 继续累加
    #[test]
    fn retry_resets_after_a_healthy_connection() {
        use super::{HEALTHY_CONNECTION, next_retry};
        use std::time::Duration;
        assert_eq!(next_retry(0, Duration::from_secs(1)), 1);
        assert_eq!(next_retry(5, Duration::from_secs(2)), 6);
        assert_eq!(next_retry(9, HEALTHY_CONNECTION), 1);
        assert_eq!(next_retry(9, Duration::from_secs(600)), 1);
    }

    /// 退避序列必须是「翻倍 + 封顶」，否则重连会退化成忙等
    #[test]
    fn backoff_doubles_then_caps() {
        let mut backoff = BODY_RECONNECT_MIN_BACKOFF;
        let mut seen = vec![backoff];
        for _ in 0..6 {
            backoff = next_body_backoff(backoff);
            seen.push(backoff);
        }
        assert_eq!(
            seen,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                BODY_RECONNECT_MAX_BACKOFF,
                BODY_RECONNECT_MAX_BACKOFF,
                BODY_RECONNECT_MAX_BACKOFF,
            ]
        );
    }

    /// 120s 窗口按「已等退避 + 单次连接超时」估算能试几次：
    /// 至少要留出 3 次机会，否则本体重启（40~60s 不可用）撐不过来
    #[test]
    fn window_allows_at_least_three_attempts() {
        let mut elapsed = Duration::ZERO;
        let mut backoff = BODY_RECONNECT_MIN_BACKOFF;
        let mut attempts = 0;
        while elapsed < BODY_RECONNECT_WINDOW {
            attempts += 1;
            elapsed += BODY_CONNECT_TIMEOUT.min(Duration::from_secs(5));
            if elapsed >= BODY_RECONNECT_WINDOW {
                break;
            }
            elapsed += backoff;
            backoff = next_body_backoff(backoff);
        }
        assert!(attempts >= 3, "窗口内只够 {attempts} 次尝试");
    }
}

#[cfg(test)]
mod locate_guard_tests {
    use super::map_matches;

    const MAP_PATH: &str = "/ota/alg_data/map/history_map/MAP-ZJ-01_v2.1";

    /// 业务地图 ID / 本体目录名两种写法都算「同一张图」
    #[test]
    fn recognises_business_id_and_dir_name() {
        assert!(map_matches(
            Some("MAP-ZJ-01@v2.1"),
            "MAP-ZJ-01@v2.1",
            MAP_PATH
        ));
        assert!(map_matches(
            Some("MAP-ZJ-01_v2.1"),
            "MAP-ZJ-01@v2.1",
            MAP_PATH
        ));
        assert!(map_matches(
            Some("  MAP-ZJ-01@v2.1 "),
            "MAP-ZJ-01@v2.1",
            MAP_PATH
        ));
    }

    /// 对不上 / 没有记录 → 必须当作「载的不是这张图」，让定位守卫重新载图
    #[test]
    fn rejects_other_or_unknown_map() {
        assert!(!map_matches(
            Some("MAP-ZJ-02@v2.1"),
            "MAP-ZJ-01@v2.1",
            MAP_PATH
        ));
        assert!(!map_matches(None, "MAP-ZJ-01@v2.1", MAP_PATH));
        assert!(!map_matches(Some("  "), "MAP-ZJ-01@v2.1", MAP_PATH));
    }
}

#[cfg(test)]
mod event_dedup_tests {
    use super::EventDedup;

    #[test]
    fn repeated_fault_is_reported_once() {
        let mut dedup = EventDedup::default();
        assert!(dedup.accept("fault", "感知障碍物检测延迟"));
        // 本体 4021 周期性重推同一条故障：不再上报
        assert!(!dedup.accept("fault", "感知障碍物检测延迟"));
        // 换一条故障照常上报
        assert!(dedup.accept("fault", "电池通信异常"));
        // 事件类型不同不互相压制
        assert!(dedup.accept("info", "感知障碍物检测延迟"));
    }
}

#[cfg(test)]
mod progress_reporter_tests {
    use super::{ProgressReporter, TaskProgressPayload};

    fn payload(status: &str, stage: &str) -> TaskProgressPayload {
        TaskProgressPayload {
            task_code: "TK-1".to_string(),
            status: status.to_string(),
            progress: 40,
            point_index: Some(2),
            waypoint_index: Some(1),
            message: Some("已完成 2/5 个航点".to_string()),
            stage: Some(stage.to_string()),
            robot_task_id: Some("nav-1".to_string()),
            total_points: Some(5),
        }
    }

    /// 5009 是 500ms 一帧，同一份内容不能每次都往业务库推
    #[tokio::test]
    async fn identical_payload_is_suppressed() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut reporter = ProgressReporter::default();
        reporter
            .report(&tx, &payload("执行中", "navigation.navigating"), "测试")
            .await;
        reporter
            .report(&tx, &payload("执行中", "navigation.navigating"), "测试")
            .await;

        let first = rx.try_recv().expect("首次必须上报");
        assert_eq!(first.msg_type, "task_progress");
        assert!(rx.try_recv().is_err(), "内容完全相同的重复帧不该再上报");
    }

    /// 阶段一变（或终态）必须立刻下发，不能被 30s 窗口压住
    #[tokio::test]
    async fn changed_payload_is_sent_immediately() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut reporter = ProgressReporter::default();
        reporter
            .report(&tx, &payload("执行中", "navigation.navigating"), "测试")
            .await;
        reporter
            .report(&tx, &payload("已完成", "navigation.succeeded"), "测试")
            .await;
        reporter
            .report(&tx, &payload("执行中", "navigation.navigating"), "测试")
            .await;

        assert!(rx.try_recv().is_ok());
        let terminal = rx.try_recv().expect("终态必须立刻上报");
        assert_eq!(terminal.payload["status"], "已完成");
        // 窗口只比对「紧邻的上一份」：内容换回去也得发，不能把某个状态永久封死
        let back = rx.try_recv().expect("内容变化必须立刻上报");
        assert_eq!(back.payload["status"], "执行中");
        assert!(rx.try_recv().is_err());
    }
}

#[cfg(test)]
mod command_result_tests {
    use super::{WsMessage, command_request_id};
    use serde_json::json;

    fn message(payload: serde_json::Value) -> WsMessage {
        WsMessage {
            msg_type: "command".to_string(),
            ts: "2026-11-16 10:00:00".to_string(),
            payload,
        }
    }

    #[test]
    fn reads_request_id_from_nested_command_params() {
        let msg = message(json!({
            "command": "下发地图",
            "params": { "mapId": "ROS-MAP-1@v1.0", "requestId": "req-push-1" }
        }));
        assert_eq!(command_request_id(&msg).as_deref(), Some("req-push-1"));
    }

    #[test]
    fn reads_request_id_when_the_platform_wraps_params_twice() {
        // 中台 → 业务平台 push_command → 设备 WS：中台参数被包成 params.params
        let msg = message(json!({
            "command": "同步媒体",
            "params": {
                "dispatchedAt": "2026-11-20 17:21:38",
                "operator": "admin",
                "params": {
                    "pictureDir": "/media/media-data/picture",
                    "frames": 3,
                    "requestId": "cmd-9f2"
                }
            }
        }));
        assert_eq!(
            command_request_id(&msg).as_deref(),
            Some("cmd-9f2"),
            "同步媒体的进度/结论就是靠这个 requestId 回写到中台审计的"
        );
    }

    #[test]
    fn falls_back_to_top_level_request_id_and_rejects_blank() {
        let msg = message(json!({ "requestId": "req-2" }));
        assert_eq!(command_request_id(&msg).as_deref(), Some("req-2"));

        let blank = message(json!({ "params": { "requestId": "   " } }));
        assert!(command_request_id(&blank).is_none());
    }
}

#[cfg(test)]
mod map_push_guard_tests {
    use super::{clear_local_runner, map_push_block};
    use crate::mock_body;
    use crate::zhpro::client::ZhProClient;
    use crate::zhpro::mapping::MappingSession;
    use crate::zhpro::route::RouteRunner;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    type RunnerSlot = Arc<Mutex<Option<Arc<RouteRunner>>>>;
    type MappingSlot = Arc<Mutex<Option<Arc<MappingSession>>>>;

    async fn client() -> Arc<ZhProClient> {
        // 假本体（`--robot mock`）是仓库里唯一不依赖真机的 ZhProClient 来源
        let host = mock_body::start(0, Duration::from_millis(100), vec![], true)
            .await
            .expect("假本体启动失败");
        Arc::new(ZhProClient::connect(&host).await.expect("连接假本体失败"))
    }

    /// 回归 2026-11-19 现场：任务取消后 `local_runner` 槽位残留，
    /// 「下发地图」永久报「设备正在执行任务」（设备其实已经待命）
    #[tokio::test]
    async fn stale_runner_does_not_block_map_push() {
        let client = client().await;
        let slot: RunnerSlot = Arc::new(Mutex::new(Some(Arc::new(RouteRunner::new(client)))));
        let mapping: MappingSlot = Arc::new(Mutex::new(None));
        assert_eq!(map_push_block(&slot, &mapping).await, None);
    }

    /// 真在跑的任务仍然要挡住下发地图（否则改图与导航互相打架）
    #[tokio::test]
    async fn running_task_still_blocks_map_push() {
        let client = client().await;
        let runner = Arc::new(RouteRunner::new(client));
        runner.mark_running(true);
        let slot: RunnerSlot = Arc::new(Mutex::new(Some(runner)));
        let mapping: MappingSlot = Arc::new(Mutex::new(None));
        let blocked = map_push_block(&slot, &mapping).await;
        assert!(
            blocked
                .as_deref()
                .unwrap_or_default()
                .contains("正在执行任务"),
            "运行中的任务必须拒绝下发地图: {blocked:?}"
        );
    }

    /// `run()` 无论从哪条分支返回都要复位「执行中」标记
    #[tokio::test]
    async fn run_clears_running_flag_on_early_return() {
        let client = client().await;
        let runner = RouteRunner::new(client);
        assert!(runner.run().await.is_err(), "没有执行计划时必须报错");
        assert!(!runner.is_running(), "run() 退出后必须复位执行中标记");
    }

    /// 收尾只摘自己那一份：新任务已经写进槽位时不能被上一轮的收尾清空
    #[tokio::test]
    async fn clear_local_runner_only_removes_itself() {
        let client = client().await;
        let previous = Arc::new(RouteRunner::new(client.clone()));
        let current = Arc::new(RouteRunner::new(client));
        let slot: RunnerSlot = Arc::new(Mutex::new(Some(current.clone())));

        clear_local_runner(&slot, &previous).await;
        assert!(
            slot.lock().await.is_some(),
            "上一轮的收尾不能清掉新任务的 runner"
        );

        clear_local_runner(&slot, &current).await;
        assert!(slot.lock().await.is_none(), "自己收尾时必须摘掉槽位");
    }
}
