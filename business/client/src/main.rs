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
//! # 模块划分
//!
//! `main.rs` 只做进程装配（配置 → 日志 → 连本体 → 连业务），其余按职责分文件：
//!
//! | 文件 | 职责 |
//! |------|------|
//! | [`session`] | 业务连接主循环：注册、心跳 / 进度 / 事件定时器、本体链路的 `select` 调度 |
//! | [`handlers`] | 下行消息分发（`handle_server_message`）+ `task` / `control` / `command` 三个业务分支 |
//! | [`body`] | 本体建链与断线原地重连、链路句柄、一次性建图 |
//! | [`pump`] | 本体推送泵：5009 / 1315 / 4001 分发与状态帧处理 |
//! | [`state`] | `RobotState` / `ActiveTask` 与运动门禁判定 |
//! | [`wire`] | 业务 WS 线协议：信封、心跳 / 进度 / 事件载荷、发送辅助 |
//! | [`maps`] | 地图推送 / 回读、增量同步、中台上传 |
//! | [`locate`] | 载图定位、初始位姿解析、地图匹配 |
//! | [`report`] | 上报合流：事件去重、进度降频、摇杆门禁提示 |
//! | [`ai_patrol`] | 航点 AI 识别（`window` / `evidence` / `agent` 三个子模块） |
//!
//! 新增下行消息请加到 `handlers` 对应的业务分支，不要在 `session` 里写业务逻辑：
//! 主循环只负责「等事件 → 分发」。
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
mod body;
mod dispatch;
mod handlers;
mod lights;
mod locate;
mod maps;
mod mapupload;
mod media_sync;
/// 联调用「假本体」：`--robot mock` 时在进程内起一个智航 Pro 网关模拟器
mod mock_body;
mod photo;
mod phone;
mod pump;
mod record;
mod report;
mod session;
mod signal;
mod speak;
mod state;
mod wire;
mod zhpro;

use crate::body::{connect_body, run_mapping_once};
use crate::session::{next_retry, run_client};
use robodog_device_client::config::{CliArgs, Config, EnvConfig, print_help};
use std::time::Duration;
use tracing::{debug, error, info, warn};

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
