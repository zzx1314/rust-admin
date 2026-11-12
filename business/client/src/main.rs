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
//! 内置默认值就是现场值：业务服务 `ws://192.168.41.246:3100`、本体 `192.168.234.234:10010`。
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

mod dispatch;
mod mapupload;
mod video;
mod zhpro;

use chrono::Local;
use dispatch::{TaskMode, parse_flight_plan, parse_map_binding};
use futures_util::{SinkExt, StreamExt};
use mapupload::{IngestConfig, UploadOutcome};
use rand::Rng;
use robodog_device_client::config::{CliArgs, Config, EnvConfig, print_help};
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
use video::VideoRelay;
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
    /// 前补光灯开关：1315 `fill_light.front`
    fill_light_front: Option<bool>,
}

/// 本体状态新鲜窗口：1315 是 1Hz，30s 收不到就认为本体链路已不可信
const BODY_STATUS_FRESH: Duration = Duration::from_secs(30);

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
            estop_software: None,
            fill_light_front: None,
        }
    }

    /// 本体链路是否在线：以 1315 心跳新鲜度为准（TCP 还连着但本体不出数也算掉线）
    fn body_status_fresh(&self) -> bool {
        self.body_status_at
            .map(|at| at.elapsed() < BODY_STATUS_FRESH)
            .unwrap_or(false)
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
            self.battery = power.round() as i64;

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
}

type Shared<T> = Arc<Mutex<T>>;

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
        match run_client(&config, zhpro.clone()).await {
            Ok(()) => {
                info!("连接正常关闭");
                break;
            }
            Err(e) => {
                retry += 1;
                let delay = retry.min(30);
                warn!("业务连接断开 ({e})，{delay}s 后重试（第 {retry} 次）");
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

    let Some(host) = config.robot.as_deref() else {
        warn!(
            "未接入本体（robot.host 为空或为 sim），以模拟模式运行（任务只回传进度，不驱动本体）"
        );
        return Ok(None);
    };

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

async fn run_client(
    config: &Config,
    zhpro: Option<Arc<ZhProClient>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_url = format!("{}/ws/device", config.server_url.trim_end_matches('/'));
    info!("正在连接业务服务: {ws_url}");
    let (ws_stream, _) = connect_async(&ws_url).await?;
    let (mut write, mut read) = ws_stream.split();
    let (out_tx, mut out_rx) = mpsc::channel::<WsMessage>(128);

    // 实时视频回传：按需拉流（业务平台有人观看时才 video_start），与主链路互不阻塞
    let video = VideoRelay::spawn(Arc::new(config.clone()));

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

    // 2) 若接入了本体，启动 5009/1315/4001 等推送处理
    if let Some(ref client) = zhpro {
        spawn_zhpro_pump(
            client.clone(),
            out_tx.clone(),
            state.clone(),
            active.clone(),
            monitor.clone(),
            local_mapping.clone(),
            Arc::new(config.clone()),
        );
    }

    // 摇杆重发（50Hz）：浏览器 ~10Hz 的离散帧需要在本体侧连续化，否则看着像点动
    if let Some(ref client) = zhpro {
        teleop::spawn_republisher(client.clone(), teleop.clone());
    }

    // 3) 定时器
    let mut heartbeat_timer = interval(Duration::from_secs(config.heartbeat_interval.max(1)));
    let mut progress_timer = interval(Duration::from_secs(5));
    progress_timer.tick().await;
    let mut event_timer = interval(Duration::from_secs(120));
    // 摇杆看门狗：250ms 检查一次，超过阈值无新帧自动停车
    let mut teleop_timer = interval(Duration::from_millis(250));
    event_timer.tick().await;

    let local_runner: Shared<Option<Arc<RouteRunner>>> = Arc::new(Mutex::new(None));
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        tokio::select! {
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
                            &zhpro,
                            &local_runner,
                            &local_mapping,
                            &teleop,
                            &video,
                        ).await;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // 交给写循环回复 pong
                        let _ = data;
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("业务服务关闭了连接");
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error!("WebSocket 错误: {e}");
                        break;
                    }
                    None => {
                        info!("业务连接已断开");
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
                if let Some(task) = snapshot {
                    let mut payload = TaskProgressPayload {
                        task_code: task.task_code.clone(),
                        status: "执行中".to_string(),
                        progress: 0,
                        point_index: None,
                        message: None,
                        stage: task.stage.clone(),
                        robot_task_id: task.nav_task_id.clone(),
                        total_points: None,
                    };
                    // 智航导航进度按航点折算；模拟模式按时间推演
                    if let Some(runner) = local_runner.lock().await.clone() {
                        let (done, total) = runner.progress().await;
                        payload.progress = progress_pct(done, total);
                        payload.point_index = Some(done as i64);
                        payload.total_points = Some(total as i64);
                        payload.message = Some(format!("已完成 {done}/{total} 个航点"));
                    } else {
                        payload.status = "已暂停".to_string();
                        payload.message = Some("等待本体连接".to_string());
                    }
                    send(&out_tx, "task_progress", serde_json::to_value(payload).unwrap_or_default()).await;
                }
            }

            _ = teleop_timer.tick() => {
                if let Some(ref client) = zhpro {
                    teleop::tick_watchdog(client, &teleop).await;
                }
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
    Ok(())
}

fn progress_pct(done: usize, total: usize) -> i64 {
    if total == 0 {
        return 0;
    }
    ((done * 100) / total) as i64
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
fn spawn_zhpro_pump(
    client: Arc<ZhProClient>,
    out_tx: mpsc::Sender<WsMessage>,
    state: Shared<RobotState>,
    active: Shared<Option<ActiveTask>>,
    monitor: Arc<Mutex<TaskMonitor>>,
    local_mapping: Shared<Option<Arc<MappingSession>>>,
    config: Arc<Config>,
) {
    tokio::spawn(async move {
        let mut rx = client.subscribe();
        let dedup = Arc::new(Mutex::new(EventDedup::default()));
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
                    if accepted.reason != "heartbeat" {
                        debug!(
                            "5009 帧: reason={} revision={} task_state={} loc={}",
                            accepted.reason,
                            accepted.revision,
                            accepted.robot_task_state,
                            accepted.loc_status
                        );
                    }
                    let frame = accepted.clone();
                    handle_status_frame(&frame, &out_tx, &state, &active, &monitor).await;
                }
                T_ROBOT_STATUS => {
                    state.lock().await.apply_robot_status(&env.data);
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
                            let event = DeviceEventPayload {
                                event_type: if severity >= 1 { "fault" } else { "info" }
                                    .to_string(),
                                message: desc.to_string(),
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
    });
}

/// 把 5009 快照翻译成业务侧进度
async fn handle_status_frame(
    frame: &StatusFrame,
    out_tx: &mpsc::Sender<WsMessage>,
    state: &Shared<RobotState>,
    active: &Shared<Option<ActiveTask>>,
    monitor: &Arc<Mutex<TaskMonitor>>,
) {
    let Some(task) = frame.current.clone() else {
        return;
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

    {
        let mut guard = active.lock().await;
        if let Some(a) = guard.as_mut() {
            a.nav_task_id = Some(task.task_id.clone());
            a.stage = Some(task.stage.clone());
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
        message: Some(message),
        stage: Some(task.stage.clone()),
        robot_task_id: Some(task.task_id.clone()),
        total_points: None,
    };
    send(
        out_tx,
        "task_progress",
        serde_json::to_value(payload).unwrap_or_default(),
    )
    .await;

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

/// 平台下发的初始位姿：`initialPose: { x, y, heading }`（heading 单位度，与路线航点一致）
///
/// 缺字段或未下发时返回 `None`，交给 SLAM 读取上一份位姿文件做完整重定位。
fn parse_initial_pose(value: Option<&Value>) -> Option<Pose> {
    let pose = value?;
    let x = pose.get("x").and_then(Value::as_f64)?;
    let y = pose.get("y").and_then(Value::as_f64)?;
    let heading = pose
        .get("heading")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Some(Pose::new(x, y, heading.to_radians()))
}

/// 巡检起步前的定位守卫。
///
/// 已经定位好（`100`）直接放行，不动地图、不打断位姿；否则用 `4030 op=1`
/// 载图并重定位，等收敛后再交给本体导航，避免 `need_locate` 起步守卫静默失败。
async fn ensure_localized(
    client: &ZhProClient,
    map_path: &str,
    initial_pose: Option<Pose>,
) -> Result<i64, String> {
    match zhpro::mapping::latest_loc_status(client, Duration::from_secs(2)).await {
        Some(status) if zhpro::mapping::loc_status_ready(status) => return Ok(status),
        Some(status) => info!(
            "定位未就绪（{}，status={status}），载图重定位 map={map_path}",
            zhpro::mapping::describe_loc_status(status)
        ),
        None => warn!("2s 内未收到 5009 定位状态，直接尝试载图重定位 map={map_path}"),
    }
    zhpro::mapping::relocalize(client, map_path, initial_pose, LOCATE_TIMEOUT).await
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
    video: &VideoRelay,
) {
    match msg.msg_type.as_str() {
        "register_ack" => {
            if msg.payload["success"].as_bool().unwrap_or(false) {
                // 服务端在注册回执里下发设备绑定的园区，建图回传时用它作为归属
                let park_id = msg.payload["parkId"].as_i64();
                *config.bound_park_id.lock().await = park_id;
                match park_id {
                    Some(park_id) => info!("设备注册成功: {}（园区 {park_id}）", config.device_id),
                    None => warn!(
                        "设备注册成功: {}，但尚未绑定园区：请在控制台绑定，否则建图不会被回传",
                        config.device_id
                    ),
                }
            } else {
                error!(
                    "设备注册失败: {}",
                    msg.payload["message"].as_str().unwrap_or("未知错误")
                );
            }
        }

        "heartbeat_ack" => {}

        // 实时视频：控制台有人看监控时下发，这里起 ffmpeg 把 RTSP 转 FLV 回传
        "video_start" => {
            let url = msg.payload["url"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string();
            if url.is_empty() {
                warn!("收到 video_start 但没有拉流地址（设备管理里未填 video_url）");
            } else {
                info!("收到实时视频请求: {}", video::redact_url(&url));
                video.start(url).await;
            }
        }

        // 最后一个观看者离开：停掉 ffmpeg，不再占本体带宽
        "video_stop" => {
            video.stop().await;
        }

        // 事件回执：客户端发的 event 由服务端落库后回 ack，不需要额外处理
        "event_ack" => {}

        "park_binding" => {
            // 控制台绑定 / 解绑园区后实时下发，无需重连
            let park_id = msg.payload["parkId"].as_i64();
            *config.bound_park_id.lock().await = park_id;
            match park_id {
                Some(park_id) => info!(
                    "园区绑定已更新: parkId={park_id} {}",
                    msg.payload["parkName"].as_str().unwrap_or("")
                ),
                None => warn!("园区绑定已解除，建图将不再自动回传"),
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

            *active.lock().await = Some(ActiveTask {
                task_code: task_code.clone(),
                task_name: task_name.clone(),
                nav_task_id: None,
                stage: None,
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
                        return;
                    };
                    info!(
                        "航线计划: {} 个航点, 地图={} (路径 {}), 结束动作={}",
                        plan.waypoints.len(),
                        plan.map_id,
                        plan.map_path,
                        plan.end_task().as_str()
                    );
                    state.lock().await.active_map = Some(plan.map_id.clone());

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
                            *active.lock().await = None;
                            state.lock().await.current_task = None;
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
                        warn!("该地图未设置回充点，返充将以 0x3672 被拒；建议先下发 4018");
                    }
                    // 用本体上报的真实目录路径（L2/M1 地图根不同，不能靠平台拼）
                    plan.map_path = robot_map.path.clone();

                    // 起步前定位守卫：不在地图上 / 定位丢失时不发 5000，直接报可读异常
                    let map_path = plan.resolve_map_path(&config.map_root);
                    let initial_pose = parse_initial_pose(msg.payload.get("initialPose"));
                    match ensure_localized(&client, &map_path, initial_pose).await {
                        Ok(status) => {
                            info!("定位已就绪（status={status}），开始执行航线")
                        }
                        Err(e) => {
                            warn!("任务 {task_code} 定位未就绪，拒绝下发: {e}");
                            report_progress(
                                out_tx,
                                &task_code,
                                "异常",
                                0,
                                Some(format!(
                                    "定位未就绪：{e}。请远程接管把设备移回地图覆盖范围，或下发「重新定位」后重试"
                                )),
                                None,
                                None,
                                None,
                            )
                            .await;
                            return;
                        }
                    }

                    let runner = Arc::new(RouteRunner::new(client.clone()));
                    let total_points = plan.waypoints.len() as i64;
                    // 预检已把定位锁好：首航点不必再让本体重载地图
                    runner.mark_pre_located();
                    runner.prepare(plan).await;
                    *local_runner.lock().await = Some(runner.clone());

                    let out = out_tx.clone();
                    let code = task_code.clone();
                    let state = state.clone();
                    let active = active.clone();
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
            let Some(client) = zhpro.clone() else {
                warn!("未连接本体，忽略远程指令");
                return;
            };
            // 遥控类指令（接管 / 释放 / 急停 / 本体动作）由 teleop 模块处理，
            // 其余是业务任务指令，仍走原来的路径
            let teleop_result =
                zhpro::teleop::handle_command(&client, teleop, command, Some(&msg.payload)).await;

            let result: Result<String, String> = match teleop_result {
                Some(result) => result,
                None => match command {
                    "暂停任务" => client
                        .call(T_TASK_PAUSE, json!({ "task_id": "current" }))
                        .await
                        .map(|_| "已请求暂停".to_string()),
                    "继续任务" => client
                        .call(T_TASK_RESUME, json!({ "task_id": "current" }))
                        .await
                        .map(|_| "已请求继续".to_string()),
                    "召回充电" => {
                        let map_id = state.lock().await.active_map.clone().unwrap_or_default();
                        client
                            .call(T_RETURN_TO_CHARGE, json!({ "map_id": map_id }))
                            .await
                            .map(|_| "已请求返回充电点".to_string())
                    }
                    "同步地图" => sync_maps_from_robot(config, &client, out_tx).await,
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

        "teleop" => {
            let Some(client) = zhpro.clone() else {
                return;
            };
            if let Err(e) = zhpro::teleop::handle_teleop(&client, teleop, &msg.payload).await {
                warn!("遥控摇杆处理失败: {e}");
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
        message,
        stage,
        robot_task_id,
        total_points,
    };
    send(
        out_tx,
        "task_progress",
        serde_json::to_value(payload).unwrap_or_default(),
    )
    .await;
}

async fn send(tx: &mpsc::Sender<WsMessage>, msg_type: &str, payload: Value) {
    let msg = WsMessage {
        msg_type: msg_type.to_string(),
        ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        payload,
    };
    let _ = tx.send(msg).await;
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
    let park_id = match config.park_id {
        Some(park_id) => Some(park_id),
        // 未显式指定时，使用注册回执里的园区绑定
        None => *config.bound_park_id.lock().await,
    };
    let Some(park_id) = park_id else {
        return UploadOutcome::Skipped(
            "设备尚未绑定园区（请在控制台设备管理里绑定，或使用 --park-id）".to_string(),
        );
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
        park_id: Some(park_id),
        map_name: config.ingest_map_name.clone(),
        bind: true,
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

/// 中台设备页「同步地图」：4012 拉取地图列表，只下载业务库缺失的地图并回传。
async fn sync_maps_from_robot(
    config: &Config,
    client: &Arc<ZhProClient>,
    out_tx: &mpsc::Sender<WsMessage>,
) -> Result<String, String> {
    let park_id = match config.park_id {
        Some(park_id) => park_id,
        None => (*config.bound_park_id.lock().await).ok_or_else(|| {
            "设备尚未绑定园区，无法确定地图归属；请先在控制台绑定园区".to_string()
        })?,
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
    info!("中台触发手动同步：本体共有 {} 张地图", maps.len());

    let ingest = IngestConfig {
        api_base: config.resolve_api_base(),
        username: config.api_username.clone(),
        password: config.api_password().to_string(),
        device_id: config.device_id.clone(),
        park_id: Some(park_id),
        map_name: None,
        bind: false,
    };
    let report = mapupload::sync_robot_maps(&http, &ingest, &file_base, park_id, &maps).await;
    let summary = report.summary();
    let event = DeviceEventPayload {
        event_type: if report.failed > 0 { "error" } else { "info" }.to_string(),
        message: summary.clone(),
        data: Some(json!({
            "total": report.total,
            "uploaded": report.uploaded,
            "skipped": report.skipped,
            "failed": report.failed,
            "mode": "manual",
        })),
    };
    send(
        out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
    if report.failed > 0 {
        Err(summary)
    } else {
        Ok(summary)
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
