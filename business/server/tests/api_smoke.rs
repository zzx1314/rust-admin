//! 接口冒烟集成测试。
//!
//! 与 `src/` 里的内联单测不同，这里把服务当成黑盒：真实迁移 + 播种 + 路由，
//! 通过 HTTP 请求验证契约。覆盖：
//!
//! - `GET /api/health` 健康检查；
//! - 未登录访问受保护接口返回 401；
//! - `admin/admin123` 登录后能取到令牌；
//! - `GET /api/dashboard/summary` 驾驶舱接口（包含 `mapDevices` 的设备列映射，
//!   历史上漏选 `video_url` 曾导致首页 500，这里作为回归用例）。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::{db, gateway, seed};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 临时数据库目录，用例结束后清理（含 WAL 附属文件）
struct TempDb {
    dir: PathBuf,
}

impl TempDb {
    fn path(&self) -> PathBuf {
        self.dir.join("test.db")
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_db() -> TempDb {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    TempDb { dir }
}

async fn test_state() -> (AppState, TempDb) {
    let db_dir = temp_db();
    let dir = db_dir.dir.clone();
    let config = Arc::new(Config {
        port: 0,
        database_path: db_dir.path(),
        jwt_secret: "test-secret-at-least-32-bytes-long!!".to_string(),
        jwt_ttl_hours: 1,
        web_dist: dir.join("web-dist"),
        backend_db_path: dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: dir.join("map_ingest"),
    });

    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");

    let state = AppState {
        db: pool,
        config,
        device_connections: gateway::new_device_connections(),
    };
    (state, db_dir)
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体失败");
    serde_json::from_slice(&bytes).expect("响应不是合法 JSON")
}

async fn login(app: &axum::Router) -> String {
    let request = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"username":"admin","password":"admin123"}"#))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "默认账号应能登录");
    let body = json_body(response).await;
    body["data"]["token"]
        .as_str()
        .expect("登录响应缺少 token")
        .to_string()
}

#[tokio::test]
async fn health_reports_ok() {
    let (state, _db) = test_state().await;
    let app = build_router(state);

    let response = app
        .clone()
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["code"], 10200);
    assert_eq!(body["data"]["status"], "ok");
}

#[tokio::test]
async fn protected_route_requires_token() {
    let (state, _db) = test_state().await;
    let app = build_router(state);

    let response = app
        .clone()
        .oneshot(Request::get("/api/parks").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = json_body(response).await;
    assert_eq!(body["code"], 10401);
}

#[tokio::test]
async fn parks_and_dashboard_work_after_login() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let parks = app
        .clone()
        .oneshot(
            Request::get("/api/parks")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(parks.status(), StatusCode::OK);
    let parks_body = json_body(parks).await;
    assert!(
        parks_body["data"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "演示数据应包含园区"
    );

    // 驾驶舱：mapDevices 由 Device 结构体全字段解码，缺列会直接 500
    let dashboard = app
        .clone()
        .oneshot(
            Request::get("/api/dashboard/summary")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(dashboard.status(), StatusCode::OK);
    let body = json_body(dashboard).await;
    assert!(body["data"]["kpis"]["onlineDevices"]["total"].is_number());
    assert!(body["data"]["mapDevices"].is_array());
    for device in body["data"]["mapDevices"].as_array().unwrap() {
        // 新字段必须出现在响应里（缺失即为回归）
        assert!(device.get("videoType").is_some(), "设备缺少 videoType");
        assert!(
            device.get("teleopHolder").is_some(),
            "设备缺少 teleopHolder"
        );
    }
}
