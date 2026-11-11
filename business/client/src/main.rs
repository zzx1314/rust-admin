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
//! ```bash
//! # 仅连业务服务（未接本体时以模拟模式运行，便于前端联调）
//! cargo run -- --device-id RD-001
//!
//! # 同时连接智航 Pro 本体，执行真实建图与航线任务
//! cargo run -- --device-id RD-001 --robot 192.168.168.100
//!
//! # 只有本体、没有业务服务时，本地跑一次建图
//! cargo run -- --robot 192.168.168.100 --map-only --map-name A区新图
//! ```

mod dispatch;
mod mapupload;
mod zhpro;

use chrono::Local;
use dispatch::{TaskMode, parse_flight_plan, parse_map_binding};
use futures_util::{SinkExt, StreamExt};
use mapupload::{IngestConfig, UploadOutcome};
use rand::Rng;
use robodog_device_client::config::{CliArgs, Config, EnvConfig, print_help};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
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
    signal: i64,
    temperature: f64,
    position: String,
    odometer: f64,
    runtime_minutes: i64,
    current_task: Option<String>,
    firmware: String,
    /// 定位状态（智航 loc_state.status）
    #[serde(skip_serializing_if = "Option::is_none")]
    loc_status: Option<i64>,
    /// 机器人当前生效地图
    #[serde(skip_serializing_if = "Option::is_none")]
    map_id: Option<String>,
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
    signal: i64,
    temperature: f64,
    position: String,
    runtime_minutes: i64,
    odometer: f64,
    current_task: Option<String>,
    firmware: String,
    loc_status: i64,
    loc_fresh: bool,
    active_map: Option<String>,
    /// 本体在线（未接本体时为 false）
    body_online: bool,
}

impl RobotState {
    fn new(config: &Config) -> Self {
        Self {
            battery: config.battery,
            signal: config.signal,
            temperature: 32.0,
            position: config.position.clone(),
            runtime_minutes: 0,
            odometer: 0.0,
            current_task: None,
            firmware: config.firmware.clone(),
            loc_status: 255,
            loc_fresh: false,
            active_map: None,
            body_online: false,
        }
    }

    /// 处理 1315 本体状态上报
    fn apply_robot_status(&mut self, data: &Value) {
        self.body_online = true;
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
            let t1 = battery
                .get("temperature1")
                .and_then(Value::as_f64)
                .unwrap_or(self.temperature);
            self.temperature = t1;
        }
        if let Some(mile) = data.get("mile_data").and_then(Value::as_f64) {
            self.odometer = mile;
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
        self.signal = (self.signal + rng.gen_range(-1..=1)).clamp(0, 5);
        self.temperature = (self.temperature + rng.gen_range(-0.1..0.3)).clamp(20.0, 65.0);
        self.runtime_minutes += 1;
    }

    fn heartbeat(&self) -> HeartbeatPayload {
        HeartbeatPayload {
            battery: self.battery,
            signal: self.signal,
            temperature: f64::trunc(self.temperature * 10.0) / 10.0,
            position: self.position.clone(),
            odometer: f64::trunc(self.odometer * 10.0) / 10.0,
            runtime_minutes: self.runtime_minutes,
            current_task: self.current_task.clone(),
            firmware: self.firmware.clone(),
            loc_status: Some(self.loc_status),
            map_id: self.active_map.clone(),
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

    // 先连本体：本体不可用时业务侧仍可联调（模拟执行）
    let zhpro = match &config.robot {
        Some(host) => match ZhProClient::connect(host).await {
            Ok(client) => {
                let session = client.session().await;
                info!(
                    "本体连接就绪: client_id={}, 型号={}",
                    session.client_id, session.device_type
                );
                Some(Arc::new(client))
            }
            Err(e) => {
                error!("连接智航本体失败: {e}；转为模拟模式");
                None
            }
        },
        None => {
            warn!("未指定 --robot，以模拟模式运行（任务只回传进度，不驱动本体）");
            None
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

    // 单写循环：所有发送集中在一处，避免并发写锁
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let text = serde_json::to_string(&msg).unwrap_or_default();
            if write.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    // 1) 注册
    send(
        &out_tx,
        "register",
        json!({ "deviceId": config.device_id, "secret": config.device_secret }),
    )
    .await;

    let state = Arc::new(Mutex::new(RobotState::new(config)));
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
                if !guard.body_online {
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
                if guard.body_online && rand::thread_rng().gen_bool(0.3) {
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
                            send(
                                &out_tx,
                                "event",
                                serde_json::to_value(event).unwrap_or_default(),
                            )
                            .await;
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
                        send(
                            &out_tx,
                            "event",
                            serde_json::to_value(event).unwrap_or_default(),
                        )
                        .await;
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
                    send(
                        &out_tx,
                        "event",
                        serde_json::to_value(event).unwrap_or_default(),
                    )
                    .await;
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

/// 航线下发前的定位就绪检查：读取最近一帧 5009 的 loc_state
async fn current_localization(zhpro: &Option<Arc<ZhProClient>>) -> Option<String> {
    let client = zhpro.as_ref()?;
    let mut rx = client.subscribe();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Ok(Ok(env)) = tokio::time::timeout(remaining, rx.recv()).await else {
            break;
        };
        if env.head.msg_type == T_TASK_STATUS
            && let Some(frame) = StatusFrame::parse(&env.data)
        {
            return Some(format!(
                "{}（status={}, fresh={}）",
                zhpro::mapping::describe_loc_status(frame.loc_status),
                frame.loc_status,
                frame.loc_fresh
            ));
        }
    }
    None
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
                    match client
                        .call(T_RETURN_TO_CHARGE, json!({ "map_id": binding.map_id }))
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
                    let Some(plan) = parse_flight_plan(&msg.payload, &config.map_root) else {
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

                    // 前置校验：地图必须存在于本体、且已设回充点（返充任务依赖）
                    let map_dir = zhpro::mapping::to_map_dir_name(&plan.map_id);
                    match MapManager::new(client.clone()).find(&map_dir).await {
                        Ok(Some(map)) => {
                            info!(
                                "机器人地图就绪: {} 路网={} 回充点={}",
                                map.map_id, map.route_exist, map.charge_point_exist
                            );
                            if plan.end_task() == zhpro::route::EndTask::ReturnToCharge
                                && !map.charge_point_exist
                            {
                                warn!("该地图未设置回充点，返充将以 0x3672 被拒；建议先下发 4018");
                            }
                        }
                        Ok(None) => warn!(
                            "机器人上没有地图 {map_dir}，导航任务可能因载图失败而终止；请先执行建图并同步地图"
                        ),
                        Err(e) => warn!("获取机器人地图列表失败: {e}"),
                    }

                    if let Some(loc) = current_localization(zhpro).await {
                        info!("机器人定位状态: {}", loc);
                    }

                    let runner = Arc::new(RouteRunner::new(client.clone()));
                    let total_points = plan.waypoints.len() as i64;
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
    let package = match mapupload::fetch_map_package(&http, &file_base, &map_dir).await {
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
