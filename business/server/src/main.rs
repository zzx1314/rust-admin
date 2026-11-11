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
    seed::seed(&pool).await?;

    let state = AppState {
        db: pool,
        config: config.clone(),
        device_connections: gateway::new_device_connections(),
    };
    // 反向控制信道：配置了中台 WS 地址时主动拨出，内网部署靠它接收中台指令
    control_plane::spawn(state.clone());
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
