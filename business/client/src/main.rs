//! robodog-device-client
//!
//! 四足机器人（设备端）WebSocket 客户端模拟器。
//! 连接到业务服务端的设备网关（/ws/device），完成注册、心跳、任务接收与进度上报。
//!
//! # 使用
//!
//! ```bash
//! # 使用默认配置连接 RD-001
//! cargo run
//!
//! # 指定设备编号与服务端地址
//! cargo run -- --device-id RD-007 --server ws://127.0.0.1:3100
//!
//! # 查看完整参数
//! cargo run -- --help
//! ```

use chrono::Local;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::time::interval;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

// ─── 消息协议（与服务端 gateway_types.rs 一致） ─────────────────

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
}

/// 任务进度上报
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskProgressPayload {
    task_code: String,
    status: String,
    progress: i64,
    point_index: Option<i64>,
    message: Option<String>,
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

// ─── 命令行参数 ────────────────────────────────────────────────

#[derive(Debug)]
struct Config {
    server_url: String,
    device_id: String,
    device_secret: Option<String>,
    /// 心跳间隔（秒）
    heartbeat_interval: u64,
    /// 初始电量
    battery: i64,
    /// 初始信号强度
    signal: i64,
    /// 起始位置（经纬度或坐标字符串）
    position: String,
    firmware: String,
}

impl Config {
    fn from_args() -> Result<Self, pico_args::Error> {
        let mut args = pico_args::Arguments::from_env();

        let config = Config {
            server_url: args
                .opt_value_from_str("--server")?
                .unwrap_or_else(|| "ws://127.0.0.1:3100".to_string()),
            device_id: args
                .opt_value_from_str("--device-id")?
                .unwrap_or_else(|| "RD-001".to_string()),
            device_secret: args.opt_value_from_str("--secret")?,
            heartbeat_interval: args
                .opt_value_from_str("--heartbeat")?
                .unwrap_or(10),
            battery: args.opt_value_from_str("--battery")?.unwrap_or(86),
            signal: args.opt_value_from_str("--signal")?.unwrap_or(4),
            position: args
                .opt_value_from_str("--position")?
                .unwrap_or_else(|| "31.2014, 121.6023".to_string()),
            firmware: args
                .opt_value_from_str("--firmware")?
                .unwrap_or_else(|| "v2.6.1".to_string()),
        };

        if args.contains("--help") || args.contains("-h") {
            print_help();
            std::process::exit(0);
        }

        Ok(config)
    }
}

fn print_help() {
    eprintln!(
        "\
机器人 WebSocket 客户端模拟器

用法: robodog-device-client [选项]

选项:
  --server <URL>         服务端 WebSocket 地址 (默认: ws://127.0.0.1:3100)
  --device-id <ID>       设备编号 (默认: RD-001)
  --secret <SECRET>      设备密钥 (可选)
  --heartbeat <秒>       心跳间隔 (默认: 10)
  --battery <0-100>      初始电量 (默认: 86)
  --signal <0-5>         初始信号强度 (默认: 4)
  --position <坐标>      位置坐标 (默认: 31.2014, 121.6023)
  --firmware <版本>      固件版本 (默认: v2.6.1)
  -h, --help             显示帮助"
    );
}

// ─── 主逻辑 ────────────────────────────────────────────────────

/// 模拟设备运行时状态
struct DeviceSimulation {
    /// 当前电量（缓慢下降）
    battery: i64,
    /// 信号强度（随机波动）
    signal: i64,
    /// 设备温度（随运行时间上升）
    temperature: f64,
    /// 当前位置
    position: String,
    /// 累计运行时间（分钟）
    runtime_minutes: i64,
    /// 累计里程（米）
    odometer: f64,
    /// 当前正在执行的任务编号
    current_task: Option<String>,
    /// 固件版本
    firmware: String,
}

impl DeviceSimulation {
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
        }
    }

    /// 模拟状态变化
    fn tick(&mut self) {
        let mut rng = rand::thread_rng();

        // 电量：执行任务时消耗更快
        let drain = if self.current_task.is_some() { 2 } else { 1 };
        self.battery = (self.battery - drain).max(0).min(100);

        // 信号：轻微波动
        self.signal = (self.signal + rng.gen_range(-1..=1)).clamp(0, 5);

        // 温度：随运行缓慢上升，上限 65℃
        self.temperature = (self.temperature + rng.gen_range(-0.1..0.3)).clamp(20.0, 65.0);

        // 累计运行
        self.runtime_minutes += 1;

        // 里程：模拟移动
        self.odometer += rng.gen_range(0.0..2.0);

        // 位置：小幅移动（模拟 GPS 漂移）
        let coords: Vec<f64> = self
            .position
            .split(',')
            .filter_map(|s| s.trim().parse::<f64>().ok())
            .collect();
        if coords.len() == 2 {
            let lat = coords[0] + rng.gen_range(-0.0001..0.0001);
            let lng = coords[1] + rng.gen_range(-0.0001..0.0001);
            self.position = format!("{lat:.6}, {lng:.6}");
        }
    }

    /// 模拟任务执行进度
    fn simulate_task_progress(&mut self, task_code: &str) -> TaskProgressPayload {
        let mut rng = rand::thread_rng();
        TaskProgressPayload {
            task_code: task_code.to_string(),
            status: if rng.gen_bool(0.05) {
                // 5% 概率异常
                "异常".to_string()
            } else {
                "执行中".to_string()
            },
            progress: rng.gen_range(1..=5),
            point_index: Some(rng.gen_range(1..=10)),
            message: Some(format!(
                "航点 {} 执行完毕，温度 {:.1}℃",
                rng.gen_range(1..=10),
                self.temperature
            )),
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "robodog_device_client=info".into()),
        )
        .init();

    let config = match Config::from_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("参数错误: {e}");
            print_help();
            std::process::exit(1);
        }
    };

    info!(
        "启动设备客户端: device_id={}, server={}",
        config.device_id, config.server_url
    );

    // 连接重试循环
    let mut retry_count = 0;
    loop {
        match run_client(&config).await {
            Ok(_) => {
                info!("连接正常关闭");
                break;
            }
            Err(e) => {
                retry_count += 1;
                let delay = (retry_count as u64).min(30);
                warn!("连接失败 ({e}), {}s 后重试 (第{retry_count}次)", delay);
                tokio::time::sleep(Duration::from_secs(delay)).await;
            }
        }
    }
}

async fn run_client(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    // ── 建立 WebSocket 连接 ──
    let ws_url = format!("{}/ws/device", config.server_url.trim_end_matches('/'));
    info!("正在连接: {ws_url}");

    let (ws_stream, response) = connect_async(&ws_url).await?;
    info!(
        "WebSocket 已连接: status={}",
        response.status()
    );

    let (mut write, mut read) = ws_stream.split();
    let mut sim = DeviceSimulation::new(config);

    // ── 1. 注册设备 ──
    let register_msg = WsMessage {
        msg_type: "register".to_string(),
        ts: now(),
        payload: json!({
            "deviceId": config.device_id,
            "secret": config.device_secret,
        }),
    };
    let text = serde_json::to_string(&register_msg)?;
    write.send(Message::Text(text.into())).await?;
    info!("已发送注册请求: device_id={}", config.device_id);

    // ── 2. 等待注册确认 ──
    let mut registered = false;
    let mut task_running = false;
    let mut current_task_code: Option<String> = None;

    // 心跳定时器
    let mut heartbeat_timer = interval(Duration::from_secs(config.heartbeat_interval));

    // 任务进度上报定时器（仅当有任务执行时）
    let mut task_progress_timer = interval(Duration::from_secs(15));

    // 设备事件模拟定时器（偶尔发送事件）
    let mut event_timer = interval(Duration::from_secs(120));
    event_timer.tick().await; // 跳过首次立即触发

    loop {
        tokio::select! {
            // ── 接收服务端消息 ──
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let ws_msg: WsMessage = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(e) => {
                                warn!("无法解析服务端消息: {e}, raw={text}");
                                continue;
                            }
                        };

                        handle_server_message(
                            &config,
                            &mut sim,
                            &mut registered,
                            &mut task_running,
                            &mut current_task_code,
                            ws_msg,
                        ).await;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        write.send(Message::Pong(data)).await?;
                    }
                    Some(Ok(Message::Binary(data))) => {
                        info!("服务端发送二进制数据: {} 字节", data.len());
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) => {
                        info!("服务端关闭了连接");
                        break;
                    }
                    Some(Err(e)) => {
                        error!("WebSocket 错误: {e}");
                        break;
                    }
                    None => {
                        info!("服务端连接已断开");
                        break;
                    }
                }
            }

            // ── 心跳定时器 ──
            _ = heartbeat_timer.tick() => {
                if !registered {
                    continue;
                }
                sim.tick();
                let hb = HeartbeatPayload {
                    battery: sim.battery,
                    signal: sim.signal,
                    temperature: f64::trunc(sim.temperature * 10.0) / 10.0,
                    position: sim.position.clone(),
                    odometer: f64::trunc(sim.odometer * 10.0) / 10.0,
                    runtime_minutes: sim.runtime_minutes,
                    current_task: sim.current_task.clone(),
                    firmware: sim.firmware.clone(),
                };
                let msg = WsMessage {
                    msg_type: "heartbeat".to_string(),
                    ts: now(),
                    payload: serde_json::to_value(hb)?,
                };
                let text = serde_json::to_string(&msg)?;
                write.send(Message::Text(text.into())).await?;
            }

            // ── 任务进度上报 ──
            _ = task_progress_timer.tick() => {
                if !task_running {
                    continue;
                }
                if let Some(ref task_code) = current_task_code {
                    let progress = sim.simulate_task_progress(task_code);
                    let msg = WsMessage {
                        msg_type: "task_progress".to_string(),
                        ts: now(),
                        payload: serde_json::to_value(progress)?,
                    };
                    let text = serde_json::to_string(&msg)?;
                    write.send(Message::Text(text.into())).await?;
                }
            }

            // ── 随机事件模拟 ──
            _ = event_timer.tick() => {
                if !registered {
                    continue;
                }
                let mut rng = rand::thread_rng();
                if rng.gen_bool(0.3) {
                    let event = DeviceEventPayload {
                        event_type: "info".to_string(),
                        message: format!("温度传感器读数正常: {:.1}℃", sim.temperature),
                        data: Some(json!({
                            "temperature": sim.temperature,
                            "humidity": rng.gen_range(40..70),
                        })),
                    };
                    let msg = WsMessage {
                        msg_type: "event".to_string(),
                        ts: now(),
                        payload: serde_json::to_value(event)?,
                    };
                    let text = serde_json::to_string(&msg)?;
                    write.send(Message::Text(text.into())).await?;
                }
            }
        }
    }

    Ok(())
}

/// 处理服务端下发的消息
async fn handle_server_message(
    _config: &Config,
    sim: &mut DeviceSimulation,
    registered: &mut bool,
    task_running: &mut bool,
    current_task_code: &mut Option<String>,
    msg: WsMessage,
) {
    match msg.msg_type.as_str() {
        "register_ack" => {
            let success = msg.payload["success"].as_bool().unwrap_or(false);
            if success {
                *registered = true;
                info!(
                    "✅ 设备注册成功: device_id={}, name={}",
                    msg.payload["deviceId"].as_str().unwrap_or(""),
                    msg.payload["deviceName"].as_str().unwrap_or(""),
                );
            } else {
                error!(
                    "❌ 设备注册失败: {}",
                    msg.payload["message"].as_str().unwrap_or("未知错误")
                );
            }
        }

        "heartbeat_ack" => {
            // 心跳确认，可检查是否有待执行命令
            info!("❤ 心跳确认");
            if let Some(cmds) = msg.payload["pendingCommands"].as_array() {
                if !cmds.is_empty() {
                    info!("有 {} 个待处理命令", cmds.len());
                }
            }
        }

        "task_dispatch" => {
            let task_code = msg.payload["taskCode"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            let task_name = msg.payload["taskName"]
                .as_str()
                .unwrap_or("未命名任务");
            let task_type = msg.payload["taskType"]
                .as_str()
                .unwrap_or("未指定");
            let execute_mode = msg.payload["executeMode"]
                .as_str()
                .unwrap_or("未指定");
            let description = msg.payload["description"]
                .as_str()
                .unwrap_or("无描述");

            info!(
                "📋 收到任务: [{}] {} (类型: {}, 模式: {})",
                task_code, task_name, task_type, execute_mode
            );
            info!("   描述: {description}");

            if let Some(route) = msg.payload["route"].as_object() {
                if let Some(route_name) = route.get("routeName").and_then(|v| v.as_str()) {
                    info!("   路线: {route_name}");
                }
            }

            if let Some(route_data) = msg.payload["routeData"].as_object() {
                let point_count = route_data
                    .get("points")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let loop_mode = route_data
                    .get("loop")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未知");
                info!("   航点数: {point_count}, 模式: {loop_mode}");
            }

            *task_running = true;
            *current_task_code = Some(task_code);
            sim.current_task = Some(task_name.to_string());

            info!("任务已接收，等待进度定时器上报");
        }

        "task_control" => {
            let task_code = msg.payload["taskCode"]
                .as_str()
                .unwrap_or("unknown");
            let action = msg.payload["action"]
                .as_str()
                .unwrap_or("unknown");
            let reason = msg.payload["reason"]
                .as_str()
                .unwrap_or("无原因");

            info!(
                "🎮 任务控制: {} → {} (原因: {reason})",
                task_code, action
            );

            match action {
                "pause" => {
                    *task_running = false;
                    info!("⏸ 任务已暂停");
                }
                "resume" => {
                    *task_running = true;
                    info!("▶ 任务已恢复");
                }
                "cancel" => {
                    *task_running = false;
                    *current_task_code = None;
                    sim.current_task = None;
                    info!("⏹ 任务已取消");
                }
                _ => {
                    warn!("未知控制指令: {action}");
                }
            }
        }

        "command" => {
            let command = msg.payload["command"]
                .as_str()
                .unwrap_or("unknown");
            let issued_at = msg.payload["issuedAt"]
                .as_str()
                .unwrap_or("");

            info!("🔧 收到远程指令: '{command}' (下发时间: {issued_at})");

            match command {
                "开始巡检" => {
                    info!("▶ 开始巡检...");
                }
                "暂停任务" => {
                    info!("⏸ 暂停任务...");
                    *task_running = false;
                }
                "召回充电" => {
                    info!("🔋 正在返回充电座...");
                    sim.current_task = None;
                    *current_task_code = None;
                    *task_running = false;
                }
                "远程接管" => {
                    info!("🎮 已进入远程接管模式");
                }
                _ => {
                    warn!("未知指令: {command}");
                }
            }
        }

        "error" => {
            let message = msg.payload["message"]
                .as_str()
                .unwrap_or("未知错误");
            error!("服务端返回错误: {message}");
        }

        "task_progress_ack" => {
            // 进度上报确认，忽略
        }

        "event_ack" => {
            // 事件确认，忽略
        }

        other => {
            warn!("未处理的消息类型: {other}");
        }
    }
}

fn now() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}
