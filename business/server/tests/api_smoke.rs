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
    test_state_with_token(None).await
}

/// `service_token` 为 `Some` 时启用中台内部接口，便于验证控制面契约。
async fn test_state_with_token(service_token: Option<&str>) -> (AppState, TempDb) {
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
        service_token: service_token.map(str::to_string),
        zentai_ws_url: None,
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
async fn synced_sources_lists_source_refs_for_park() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let (park_id, map_id): (i64, String) =
        sqlx::query_as("SELECT park_id, id FROM map_versions LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("演示数据应包含地图基线");
    sqlx::query("UPDATE map_versions SET source_ref = ?1 WHERE id = ?2")
        .bind("/ota/alg_data/map/history_map/demo")
        .bind(&map_id)
        .execute(&pool)
        .await
        .unwrap();

    let app = build_router(state);
    let token = login(&app).await;
    let response = app
        .oneshot(
            Request::get(format!("/api/map-versions/synced-sources?parkId={park_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"][0], "/ota/alg_data/map/history_map/demo");
}

#[tokio::test]
async fn internal_sync_endpoint_requires_service_token() {
    let (state, _db) = test_state().await;
    let app = build_router(state);

    let response = app
        .oneshot(
            Request::post("/api/zentai/devices/RD-001/sync-maps")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
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

/// 中台内部接口：控制面契约（指令 / 任务 / 运行态）。
mod zentai_control {
    use super::*;

    const TOKEN: &str = "test-service-token";

    /// 取一台已绑定园区的演示设备，中台内部接口按设备编号寻址。
    async fn bound_device(pool: &sqlx::SqlitePool) -> (i64, String) {
        sqlx::query_as("SELECT id, device_id FROM devices WHERE park_id IS NOT NULL LIMIT 1")
            .fetch_one(pool)
            .await
            .expect("演示数据应包含已绑定园区的设备")
    }

    #[tokio::test]
    async fn runtime_reports_offline_device() {
        let (state, _db) = test_state_with_token(Some(TOKEN)).await;
        let (_id, code) = bound_device(&state.db).await;
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::get(format!("/api/zentai/devices/{code}/runtime"))
                    .header("x-service-token", TOKEN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = json_body(response).await;
        assert_eq!(body["code"], 10200);
        assert_eq!(body["data"]["deviceId"], code.as_str());
        // 没有 WebSocket 连接：必须如实报告未在线
        assert_eq!(body["data"]["online"], false);
    }

    #[tokio::test]
    async fn command_to_offline_device_reports_not_delivered() {
        let (state, _db) = test_state_with_token(Some(TOKEN)).await;
        let (_id, code) = bound_device(&state.db).await;
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::post(format!("/api/zentai/devices/{code}/command"))
                    .header(
                        header::CONTENT_TYPE,
                        header::HeaderValue::from_static("application/json"),
                    )
                    .header("x-service-token", TOKEN)
                    .header("x-request-id", "smoke-command-offline")
                    .body(Body::from(r#"{"command":"召回充电"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // 设备不在线：显式失败，绝不返回「已下发」
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = json_body(response).await;
        assert!(
            body["msg"].as_str().unwrap_or_default().contains("未送达"),
            "未送达必须显式报错，实际：{}",
            body["msg"]
        );
    }

    #[tokio::test]
    async fn task_dispatch_is_idempotent_per_request_id() {
        let (state, _db) = test_state_with_token(Some(TOKEN)).await;
        let (device_pk, code) = bound_device(&state.db).await;
        let pool = state.db.clone();
        let app = build_router(state);

        let count = || {
            let pool = pool.clone();
            async move {
                let (total,): (i64,) =
                    sqlx::query_as("SELECT COUNT(*) FROM tasks WHERE device_id = ?1")
                        .bind(device_pk)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                total
            }
        };
        let before = count().await;

        let payload = r#"{"taskName":"冒烟巡检","routeId":1,"requestId":"smoke-task-1"}"#;
        let mut statuses = Vec::new();
        for _ in 0..2 {
            let response = app
                .clone()
                .oneshot(
                    Request::post(format!("/api/zentai/devices/{code}/tasks"))
                        .header(
                            header::CONTENT_TYPE,
                            header::HeaderValue::from_static("application/json"),
                        )
                        .header("x-service-token", TOKEN)
                        .header("x-request-id", "smoke-task-1")
                        .body(Body::from(payload))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            statuses.push(status);
            if status == StatusCode::OK {
                let body = json_body(response).await;
                assert_eq!(body["data"]["requestId"], "smoke-task-1");
            }
        }

        assert_eq!(statuses, vec![StatusCode::OK, StatusCode::OK]);
        // 幂等：同一 requestId 只创建一条任务
        assert_eq!(count().await, before + 1);
    }

    #[tokio::test]
    async fn internal_endpoints_reject_missing_or_wrong_token() {
        let (state, _db) = test_state_with_token(Some(TOKEN)).await;
        let (_id, code) = bound_device(&state.db).await;
        let app = build_router(state);

        for token in [None, Some("wrong-token")] {
            let mut request = Request::get(format!("/api/zentai/devices/{code}/runtime"));
            if let Some(token) = token {
                request = request.header("x-service-token", token);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}

/// 反向控制信道（方案 B）：业务平台主动拨入中台，接收指令并回执。
///
/// 这里用假的「中台 WS 服务」验证业务平台侧：握手带头、按 `kind` 分派、
/// 结果回执带 requestId、错误转成 ok=false。
mod control_plane_channel {
    use super::*;
    use axum::Router;
    use axum::extract::ws::{Message, WebSocketUpgrade};
    use axum::routing::get;
    use futures_util::{SinkExt, StreamExt};
    use robodog_ops_server::control_plane;
    use std::time::Duration;
    use tokio::sync::mpsc;

    const TOKEN: &str = "test-service-token";

    /// 假的「中台」：接受拨入，发一条控制请求，收回执后转给测试。
    async fn spawn_fake_zentai(
        device_code: &str,
        kind: &str,
        body: Value,
    ) -> (String, mpsc::Receiver<Value>) {
        let (tx, rx) = mpsc::channel::<Value>(4);
        let code = device_code.to_string();
        let kind = kind.to_string();

        let app = Router::new().route(
            "/api/dogDevice/control-plane/ws",
            get(
                move |ws: WebSocketUpgrade, headers: axum::http::HeaderMap| {
                    let tx = tx.clone();
                    let code = code.clone();
                    let kind = kind.clone();
                    let body = body.clone();
                    async move {
                        // 中台侧要求：服务令牌必须随握手带上
                        assert_eq!(
                            headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                            Some(TOKEN),
                            "控制信道必须带服务令牌"
                        );
                        ws.on_upgrade(move |socket| async move {
                            let (mut sink, mut stream) = socket.split();
                            // 等 hello 握手帧
                            while let Some(Ok(Message::Text(text))) = stream.next().await {
                                let frame: Value = serde_json::from_str(&text).unwrap();
                                if frame["type"] == "hello" {
                                    break;
                                }
                            }
                            let request = serde_json::json!({
                                "type": "request",
                                "payload": {
                                    "requestId": "ws-req-1",
                                    "kind": kind,
                                    "deviceCode": code,
                                    "action": "测试",
                                    "operatorId": 1,
                                    "operatorAdmin": true,
                                    "body": body,
                                },
                            });
                            sink.send(Message::Text(request.to_string().into()))
                                .await
                                .expect("发送控制请求失败");
                            // 收回执
                            while let Some(Ok(Message::Text(text))) = stream.next().await {
                                let frame: Value = serde_json::from_str(&text).unwrap();
                                if frame["type"] == "result" {
                                    let _ = tx.send(frame).await;
                                    break;
                                }
                            }
                        })
                    }
                },
            ),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake zentai");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("ws://{addr}"), rx)
    }

    async fn state_dialing_zentai(url: String) -> (AppState, TempDb) {
        let (state, db_dir) = test_state_with_token(Some(TOKEN)).await;
        let mut config = (*state.config).clone();
        config.zentai_ws_url = Some(url);
        let state = AppState {
            db: state.db,
            config: Arc::new(config),
            device_connections: state.device_connections,
        };
        (state, db_dir)
    }

    async fn bound_device(pool: &sqlx::SqlitePool) -> String {
        sqlx::query_scalar("SELECT device_id FROM devices WHERE park_id IS NOT NULL LIMIT 1")
            .fetch_one(pool)
            .await
            .expect("演示数据应包含已绑定园区的设备")
    }

    #[tokio::test]
    async fn runtime_request_is_answered_over_the_channel() {
        let (probe, _db0) = test_state_with_token(Some(TOKEN)).await;
        let code = bound_device(&probe.db).await;
        let (url, mut results) = spawn_fake_zentai(&code, "runtime", Value::Null).await;
        let (state, _db) = state_dialing_zentai(url).await;

        control_plane::spawn(state);

        let frame = tokio::time::timeout(Duration::from_secs(5), results.recv())
            .await
            .expect("等待回执超时")
            .expect("应有回执帧");
        assert_eq!(frame["payload"]["requestId"], "ws-req-1");
        assert_eq!(frame["payload"]["ok"], true);
        assert_eq!(frame["payload"]["data"]["deviceId"], code.as_str());
        assert_eq!(frame["payload"]["data"]["online"], false);
    }

    #[tokio::test]
    async fn sync_maps_request_reaches_the_same_code_path() {
        let (probe, _db0) = test_state_with_token(Some(TOKEN)).await;
        let code = bound_device(&probe.db).await;
        let body = serde_json::json!({"mode": "manual"});
        let (url, mut results) = spawn_fake_zentai(&code, "sync-maps", body).await;
        let (state, _db) = state_dialing_zentai(url).await;

        control_plane::spawn(state);

        let frame = tokio::time::timeout(Duration::from_secs(5), results.recv())
            .await
            .expect("等待回执超时")
            .expect("应有回执帧");
        // 演示设备没有 WS 连接：同步指令必然送不出去，但说明确实走到了下发实现
        assert_eq!(frame["payload"]["ok"], false);
        let error = frame["payload"]["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("未连接"),
            "离线设备应报告未连接，实际：{error}"
        );
    }

    #[tokio::test]
    async fn offline_device_rejection_becomes_failed_result() {
        let (probe, _db0) = test_state_with_token(Some(TOKEN)).await;
        let code = bound_device(&probe.db).await;
        let body = serde_json::json!({"command": "召回充电"});
        let (url, mut results) = spawn_fake_zentai(&code, "command", body).await;
        let (state, _db) = state_dialing_zentai(url).await;

        control_plane::spawn(state);

        let frame = tokio::time::timeout(Duration::from_secs(5), results.recv())
            .await
            .expect("等待回执超时")
            .expect("应有回执帧");
        assert_eq!(frame["payload"]["ok"], false);
        let error = frame["payload"]["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("未送达"),
            "设备离线必须如实报告未送达，实际：{error}"
        );
    }
}
