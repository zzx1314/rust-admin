use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::{control_plane, db, gateway, seed};
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

    let state = AppState {
        db: pool,
        config: config.clone(),
        device_connections: gateway::new_device_connections(),
        video_hub: Default::default(),
    };
    // 反向控制信道：配置了中台 WS 地址时主动拨出，内网部署靠它接收中台指令
    control_plane::spawn(state.clone());
    // 心跳看门狗：静默掉线（收不到 Close 帧）的设备超时后置为离线
    gateway::spawn_heartbeat_watchdog(state.clone());
    // 任务看门狗：`执行中` 但长时间没有进度上报的任务置为异常
    gateway::spawn_task_watchdog(state.clone());
    // 视频看门狗：没人观看却还在推流时让客户端停掉 ffmpeg
    robodog_ops_server::video::spawn_watchdog(state.clone());
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
    .await?;
    Ok(())
}
