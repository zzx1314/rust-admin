use crate::auth::{login_handler, me_handler, require_auth};
use crate::config::Config;
use crate::error::ApiResponse;
use crate::gateway;
use crate::gateway::DeviceConnections;
use crate::handlers::{
    alerts, dashboard, devices, ingest, maps, parks, routes, service, settings, tasks, teleop,
    zones,
};
use axum::extract::State;
use axum::http::Method;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    pub device_connections: DeviceConnections,
}

async fn health(State(state): State<AppState>) -> Json<ApiResponse<Value>> {
    ApiResponse::ok(json!({
        "status": "ok",
        "service": "robodog-ops-server",
        "database": state.config.database_path.display().to_string(),
        // 建图回传中台是否就绪；未配置时机器人建图无法自动入库
        "mapIngest": {
            "ready": state.config.zentai_ready(),
            "zentaiUrl": state.config.zentai_base_url,
            "ingestDir": state.config.ingest_dir.display().to_string(),
        },
    }))
}

pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/auth/me", get(me_handler))
        .route("/parks", get(parks::list_parks).post(parks::create_park))
        .route(
            "/parks/{id}",
            get(parks::get_park)
                .put(parks::update_park)
                .delete(parks::delete_park),
        )
        .route("/parks/{id}/maps", get(maps::list_park_maps))
        .route("/map-versions", get(maps::list_maps))
        .route(
            "/map-versions/synced-sources",
            get(maps::list_synced_sources),
        )
        .route("/map-versions/backend", get(maps::list_backend_maps))
        .route(
            "/map-versions/import-backend",
            post(maps::import_backend_map),
        )
        .route("/map-versions/ingest", post(ingest::ingest_map))
        .route("/map-versions/{id}", get(maps::get_map))
        .route("/map-versions/{id}/raster", get(maps::map_raster))
        .route("/map-versions/{id}/refs", get(maps::map_refs))
        .route("/map-versions/{id}/publish", post(maps::publish_map))
        .route(
            "/map-versions/{id}/new-version",
            post(maps::new_map_version),
        )
        .route("/zones", get(zones::list_zones).post(zones::create_zone))
        .route(
            "/zones/{id}",
            get(zones::get_zone)
                .put(zones::update_zone)
                .delete(zones::delete_zone),
        )
        .route(
            "/devices",
            get(devices::list_devices).post(devices::create_device),
        )
        .route("/devices/stats", get(devices::device_stats))
        .route("/devices/refresh", post(devices::refresh_devices))
        .route(
            "/devices/{id}",
            get(devices::get_device)
                .put(devices::update_device)
                .delete(devices::delete_device),
        )
        .route("/devices/{id}/command", post(devices::send_command))
        .route(
            "/devices/{id}/bind",
            post(devices::bind_device).delete(devices::unbind_device),
        )
        .route("/devices/{id}/action", post(teleop::send_action))
        .route("/devices/{id}/teleop", post(teleop::send_teleop))
        .route("/devices/{id}/teleop/take", post(teleop::take_over))
        .route("/devices/{id}/teleop/release", post(teleop::release))
        .route("/devices/{id}/teleop/ping", post(teleop::keepalive))
        .route("/tasks", get(tasks::list_tasks).post(tasks::create_task))
        .route("/tasks/stats", get(tasks::task_stats))
        .route(
            "/tasks/{id}",
            get(tasks::get_task)
                .put(tasks::update_task)
                .delete(tasks::delete_task),
        )
        .route(
            "/tasks/{id}/status",
            axum::routing::put(tasks::update_task_status),
        )
        .route(
            "/routes",
            get(routes::list_routes).post(routes::create_route),
        )
        .route(
            "/routes/{id}",
            get(routes::get_route)
                .put(routes::update_route)
                .delete(routes::delete_route),
        )
        .route("/routes/{id}/validate", post(routes::validate_route))
        .route("/routes/{id}/publish", post(routes::publish_route))
        .route("/routes/{id}/optimize", post(routes::optimize_route))
        .route(
            "/alerts",
            get(alerts::list_alerts).post(alerts::create_alert),
        )
        .route("/alerts/stats", get(alerts::alert_stats))
        .route("/alerts/handle-all", post(alerts::handle_all_alerts))
        .route(
            "/alerts/{id}",
            get(alerts::get_alert)
                .put(alerts::update_alert)
                .delete(alerts::delete_alert),
        )
        .route("/alerts/{id}/handle", post(alerts::handle_alert))
        .route(
            "/settings",
            get(settings::list_settings).put(settings::update_settings),
        )
        .route("/dashboard/summary", get(dashboard::summary))
        .layer(from_fn_with_state(state.clone(), require_auth));

    let public = Router::new()
        .route("/auth/login", post(login_handler))
        .route("/health", get(health))
        // 中台内部接口：由共享服务令牌鉴权，不使用业务 JWT
        .route(
            "/zentai/devices/{device_id}/sync-maps",
            post(service::sync_device_maps),
        )
        // 兼容历史上挂在 /api 下的设备网关路径（实际对外路径见下方 top-level 路由）
        .route("/ws/device", get(gateway::ws_handler));

    let api = public.merge(protected);

    let mut router = Router::new()
        // 设备 WebSocket 网关：客户端与文档约定为 `/ws/device`
        .route("/ws/device", get(gateway::ws_handler))
        .nest("/api", api)
        .with_state(state.clone())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http());

    let index = state.config.web_dist.join("index.html");
    if state.config.web_dist.is_dir() && index.is_file() {
        router = router.fallback_service(
            ServeDir::new(&state.config.web_dist).fallback(ServeFile::new(index)),
        );
    }

    router
}
