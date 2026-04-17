use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use x_rust::app;
use x_rust::config::AppConfig;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "x_rust=debug,tower_http=debug,axum=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::new().unwrap_or_else(|e| {
        eprintln!("Failed to load config: {}", e);
        std::process::exit(1);
    });

    let app = app::App::new(&config.database_url)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to initialize app: {}", e);
            std::process::exit(1);
        });

    let addr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .unwrap_or_else(|_| {
            eprintln!("Invalid server address");
            std::process::exit(1);
        });
    app.run(&config, addr).await.unwrap_or_else(|e| {
        eprintln!("Server error: {}", e);
        std::process::exit(1);
    });
}
