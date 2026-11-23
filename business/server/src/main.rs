use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::{control_plane, db, gateway, notify, seed};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("robodog_ops_server=debug,tower_http=info")),
        )
        .init();

    // 环境变量 > server.toml > 默认值；显式指定的配置文件缺失会直接报错
    let config = Arc::new(Config::load()?);
    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;
    // 历史机器人地图没有设备编号（迁移 015 前入库），按中台 dog_map 回填一次
    match db::backfill_map_device_codes(&pool, &config.backend_db_path).await {
        Ok(0) => {}
        Ok(count) => tracing::info!(count, "地图设备编号回填完成（device_code）"),
        Err(error) => tracing::warn!("地图设备编号回填失败: {error}"),
    }
    seed::seed(&pool).await?;

    // 企业微信推送句柄：地址可以只在平台设置页里（settings.wecom_webhook），
    // 也可以只在环境变量里；两边都空时句柄「不说话」，但消费端照样启动，
    // 这样现场在页面上填完地址就立即生效，不用重启进程。
    let stored_webhook = notify::stored_webhook(&pool).await;
    let (notifier, notify_rx) = notify::Notifier::channel(&config, stored_webhook);
    match notifier.webhook_snapshot() {
        (Some(webhook), source) => {
            // 只打 key 末 6 位：够核对「填的是哪一个」，又不把凭据写进日志
            tracing::info!(
                enabled = config.wecom_enabled,
                min_level = %config.wecom_min_level,
                source = source.map_or("?", |source| source.label()),
                key_tail = %notify::key_tail(&webhook),
                "企业微信告警推送已配置"
            );
        }
        (None, _) => {
            tracing::info!(
                "企业微信告警推送未配置 webhook（可在「系统设置 → 企业微信群机器人」填写）"
            )
        }
    }

    let state = AppState {
        db: pool,
        config: config.clone(),
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        notifier,
        agent_tunnel: Default::default(),
        ai_patrol: Default::default(),
    };
    // 反向控制信道：配置了中台 WS 地址时主动拨出，内网部署靠它接收中台指令
    control_plane::spawn(state.clone());
    // 心跳看门狗：静默掉线（收不到 Close 帧）的设备超时后置为离线
    gateway::spawn_heartbeat_watchdog(state.clone());
    // 任务看门狗：`执行中` 但长时间没有进度上报、或进度停滞的任务置为异常
    gateway::spawn_task_watchdog(state.clone());
    // 计划调度：定时巡检按配置的时间段/执行日到点下发（见 `schedule` 模块）
    robodog_ops_server::schedule::spawn(state.clone());
    // 视频看门狗：没人观看却还在推流时让客户端停掉 ffmpeg
    robodog_ops_server::video::spawn_watchdog(state.clone());
    // 企业微信推送消费端：限流 + 重试都在这里，调用方只负责入队
    if let Some(rx) = notify_rx {
        notify::spawn(state.notify(), rx);
    }
    let router = build_router(state);

    let address = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(address = %address, database = %config.database_path.display(), "robodog ops server started");
    tracing::info!("default account: admin / admin123");
    // ws_handler 需要 ConnectInfo 取设备接入 IP（落 devices.device_ip）
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

/// 等 SIGINT / SIGTERM（Ctrl-C / systemd restart）。
///
/// 以前直接 `serve(..).await`，`systemctl restart` 会把正在跑的请求硬切：
/// 告警推送落库（`notifications`）、任务状态与 `task_events` 写入都可能只写一半。
/// `serve` 退出后进程才结束，后台任务（看门狗 / 调度 / 推送）跟着结束，
/// 所以这里只需要保证 HTTP 侧把在途请求做完。
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            // 注册不上信号就只等 Ctrl-C，别让整个进程因此启动失败
            Err(error) => {
                tracing::warn!("注册 SIGTERM 监听失败，只能靠 SIGINT 退出: {error}");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("收到退出信号，等待在途请求处理完成后退出");
}
