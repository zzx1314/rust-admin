mod app;
mod auth;
mod config;
mod db;
mod error;
mod gateway;
mod gateway_types;
mod geo;
mod handlers;
mod models;
mod seed;

use app::{AppState, build_router};
use config::Config;
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

    let config = Arc::new(Config::from_env());
    let pool = db::connect(&config).await?;
    db::migrate(&pool).await?;
    seed::seed(&pool).await?;

    let state = AppState {
        db: pool,
        config: config.clone(),
        device_connections: gateway::new_device_connections(),
    };
    let router = build_router(state);

    let address = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(address = %address, database = %config.database_path.display(), "robodog ops server started");
    tracing::info!("default account: admin / admin123");
    axum::serve(listener, router).await?;
    Ok(())
}
