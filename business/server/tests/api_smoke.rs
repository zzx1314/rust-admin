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
        device_timeout_secs: 90,
    });

    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");

    let state = AppState {
        db: pool,
        config,
        device_connections: gateway::new_device_connections(),
        video_hub: Default::default(),
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

/// 中台连不上时，登录必须报 503 + 可读原因，而不是 500「服务器错误」
///
/// 现场踩过：本地密码输错 → 回落到中台校验 → 中台没起 → 500，
/// 页面上只看到「Request failed with status code 500」，完全看不出是哪个组件挂了。
#[tokio::test]
async fn login_reports_upstream_unavailable_instead_of_500() {
    let (state, _db) = test_state().await;
    let mut config = (*state.config).clone();
    // 1 号端口必然拒绝连接：等价于「中台没启动」
    config.zentai_base_url = Some("http://127.0.0.1:1".to_string());
    let state = AppState {
        config: Arc::new(config),
        ..state
    };
    let app = build_router(state);

    let request = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"username":"admin","password":"definitely-wrong"}"#,
        ))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "中台不可用应是 503，不是 500"
    );
    let body = json_body(response).await;
    assert_eq!(body["code"], 10503);
    let msg = body["msg"].as_str().unwrap_or_default();
    assert!(msg.contains("中台不可达"), "提示应说明中台连不上: {msg}");
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
async fn synced_sources_drops_maps_deleted_in_zentai() {
    // 假中台：只认 dog_map.id = 1 仍存在（另一张已被软删除）
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake zentai");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let router = axum::Router::new().route(
            "/api/dogInternal/maps/exists",
            axum::routing::post(|| async {
                axum::Json(serde_json::json!({
                    "code": 10200,
                    "data": { "existing": [1] }
                }))
            }),
        );
        let _ = axum::serve(listener, router).await;
    });

    let (state, _db) = test_state_with_token(Some("svc-token")).await;
    let mut config = (*state.config).clone();
    config.zentai_base_url = Some(format!("http://{addr}"));
    let state = AppState {
        config: Arc::new(config),
        ..state
    };
    let pool = state.db.clone();
    let (park_id,): (i64,) = sqlx::query_as("SELECT park_id FROM map_versions LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("演示数据应包含地图基线");
    // source_id = 中台 dog_map.id：1 还活着，2 已被中台删除
    for (id, source_id) in [("robot-map-alive", "1"), ("robot-map-stale", "2")] {
        sqlx::query(
            "INSERT INTO map_versions
                (id, park_id, map_name, version, status, created_at, source, source_id, source_ref)
             VALUES (?1, ?2, ?3, 'v1.0', '部分标定', '2026-01-01 00:00:00', 'robot-mapping', ?4, ?5)",
        )
        .bind(id)
        .bind(park_id)
        .bind(format!("map-{source_id}"))
        .bind(source_id)
        .bind(format!("/ota/alg_data/map/history_map/{source_id}"))
        .execute(&pool)
        .await
        .unwrap();
    }

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
    let sources = body["data"].as_array().cloned().unwrap_or_default();
    assert!(
        sources.contains(&serde_json::json!("/ota/alg_data/map/history_map/1")),
        "中台仍存在的地图应算已同步: {body}"
    );
    assert!(
        !sources.contains(&serde_json::json!("/ota/alg_data/map/history_map/2")),
        "中台已删除的地图不能算已同步，否则设备端永远不会重传: {body}"
    );
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
            video_hub: state.video_hub,
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

/// 出站到中台：配置了共享服务令牌后走 `/api/dogInternal/*`，不再登录换 JWT。
///
/// 这里用一个假的「中台内部接口」服务验证三件事：
///   1. 请求必须带 `x-service-token`，且路径是内部接口；
///   2. 全程不碰 `/api/token`（说明没有账号登录、没有会话过期问题）；
///   3. 台账 / 分配关系 / 地图回传的解析逻辑与老路径一致。
mod zentai_service_token {
    use super::*;
    use axum::Router;
    use axum::extract::Multipart;
    use axum::http::HeaderMap;
    use axum::routing::{get, post};
    use robodog_ops_server::config::Config;
    use robodog_ops_server::ingest::{self, IngestRequest, ParkRef};
    use robodog_ops_server::zentai::{self, DeviceUpsert};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TOKEN: &str = "test-service-token";

    fn service_config(base: String) -> Config {
        Config {
            port: 0,
            database_path: std::env::temp_dir().join("robodog-zentai-test.db"),
            jwt_secret: "test-secret-at-least-32-bytes-long!!".to_string(),
            jwt_ttl_hours: 1,
            web_dist: std::env::temp_dir().join("web-dist"),
            backend_db_path: std::env::temp_dir().join("backend.db"),
            zentai_base_url: Some(base),
            zentai_username: None,
            zentai_password: None,
            ingest_dir: std::env::temp_dir().join("map_ingest"),
            service_token: Some(TOKEN.to_string()),
            zentai_ws_url: None,
            device_timeout_secs: 90,
        }
    }

    /// 启动假中台；`token_calls` 记录是否有账号登录发生。
    async fn spawn_fake_zentai() -> (String, Arc<AtomicUsize>) {
        let token_calls = Arc::new(AtomicUsize::new(0));
        let login_hits = token_calls.clone();

        let app = Router::new()
            .route(
                "/api/token",
                post(move || {
                    let login_hits = login_hits.clone();
                    async move {
                        login_hits.fetch_add(1, Ordering::SeqCst);
                        axum::Json(serde_json::json!({"code": 10401, "msg": "不应被调用"}))
                    }
                }),
            )
            .route(
                "/api/dogInternal/devices/assignments",
                get(|headers: HeaderMap| async move {
                    assert_eq!(
                        headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                        Some(TOKEN)
                    );
                    axum::Json(serde_json::json!({
                        "code": 10200,
                        "msg": "success",
                        "data": [{
                            "deviceId": "RD-001",
                            "users": [{"id": 7, "username": "u7", "userIds": [7]}],
                            "userIds": [7],
                        }],
                    }))
                }),
            )
            .route(
                "/api/dogInternal/devices/upsert",
                post(|headers: HeaderMap, body: axum::Json<Value>| async move {
                    assert_eq!(
                        headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                        Some(TOKEN)
                    );
                    assert_eq!(body["deviceId"], "RD-300");
                    axum::Json(serde_json::json!({
                        "code": 10200,
                        "msg": "success",
                        "data": {"deviceId": "RD-300"},
                    }))
                }),
            )
            .route(
                "/api/dogInternal/maps/exists",
                post(|headers: HeaderMap, body: axum::Json<Value>| async move {
                    assert_eq!(
                        headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                        Some(TOKEN)
                    );
                    assert_eq!(body["ids"], serde_json::json!([42, 43]));
                    axum::Json(serde_json::json!({
                        "code": 10200,
                        "msg": "success",
                        "data": {"existing": [42]},
                    }))
                }),
            )
            .route(
                "/api/dogInternal/maps/upload",
                post(|headers: HeaderMap, mut multipart: Multipart| async move {
                    assert_eq!(
                        headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                        Some(TOKEN)
                    );
                    let mut saw_file = false;
                    let mut map_name = String::new();
                    while let Some(field) = multipart.next_field().await.unwrap() {
                        match field.name() {
                            Some("file") => {
                                let _ = field.bytes().await.unwrap();
                                saw_file = true;
                            }
                            Some("mapInfo") => {
                                let text = field.text().await.unwrap();
                                let info: Value = serde_json::from_str(&text).unwrap();
                                map_name = info["mapName"].as_str().unwrap_or_default().to_string();
                                assert_eq!(info["ownerUserId"], 7);
                            }
                            _ => {}
                        }
                    }
                    assert!(saw_file, "必须收到地图包");
                    axum::Json(serde_json::json!({
                        "code": 10200,
                        "msg": "success",
                        "data": {"id": 42, "mapName": map_name, "fileName": "map.zip"},
                    }))
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake zentai");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), token_calls)
    }

    #[tokio::test]
    async fn service_token_replaces_account_login() {
        let (base, token_calls) = spawn_fake_zentai().await;
        let config = service_config(base);

        // 只配令牌也算「与中台通信就绪」，但中台账号登录路径关闭
        assert!(config.zentai_ready());
        assert!(config.has_service_token());
        assert!(!config.zentai_account_ready());
        assert_eq!(config.zentai_auth_mode(), "service-token");

        let devices = zentai::device_assignments(&config)
            .await
            .expect("分配关系应可读取");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, "RD-001");
        assert_eq!(devices[0].user_ids, vec![7]);

        zentai::upsert_device(
            &config,
            &DeviceUpsert {
                device_id: "RD-300".to_string(),
                device_name: Some("测试狗".to_string()),
                model: None,
                status: Some("在线待命".to_string()),
                firmware: None,
                device_ip: None,
                park_code: None,
                park_name: None,
                remarks: None,
                user_ids: vec![7],
            },
        )
        .await
        .expect("设备台账应可同步");

        let http = reqwest::Client::new();
        let request = IngestRequest {
            map_name: "A 区机器人建图".to_string(),
            park_id: Some(1),
            device_id: Some("RD-001".to_string()),
            bind: false,
            map_type: None,
            remarks: None,
            source: None,
            source_ref: None,
        };
        let park = ParkRef {
            code: "P01".to_string(),
            name: "A 区".to_string(),
        };
        let map = ingest::push_to_zentai(
            &config,
            &http,
            &request,
            &park,
            "map.zip",
            b"PK\x03\x04",
            Some(7),
        )
        .await
        .expect("地图应可回传");
        assert_eq!(map.id, 42);
        assert_eq!(map.map_name, "A 区机器人建图");

        // 全程没有登录换 JWT
        assert_eq!(token_calls.load(Ordering::SeqCst), 0, "不应调用 /api/token");
    }

    /// 中台已删除的地图不能算「已同步」，否则设备端永远不会重传。
    #[tokio::test]
    async fn alive_map_ids_filters_deleted_maps() {
        let (base, _token_calls) = spawn_fake_zentai().await;
        let config = service_config(base);
        let alive = zentai::alive_map_ids(&config, &[42, 43])
            .await
            .expect("应能查询中台地图存在性");
        assert!(alive.contains(&42), "中台仍存在的地图应返回");
        assert!(!alive.contains(&43), "中台已删除的地图不应返回");
    }
}

#[tokio::test]
async fn refresh_probes_connections_and_marks_offline() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    // 播种数据里有 11 台「在线」设备，但没有任何 WebSocket 连接
    let online_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE status <> '离线'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(online_before > 0, "演示数据应包含在线设备");

    let app = build_router(state);
    let token = login(&app).await;
    let response = app
        .oneshot(
            Request::post("/api/devices/refresh")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    // 没有活跃连接 → 探活结果为 0 台在线（不再伪造 last_heartbeat）
    assert_eq!(body["data"], 0);

    let offline: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE status = '离线'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(offline, online_before + 1, "未连接的设备应全部置为离线");
}

#[tokio::test]
async fn watchdog_marks_stale_real_device_offline() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    // 一台「真机」：完整时间戳且已超时；演示设备的相对时间不参与判定
    sqlx::query(
        "UPDATE devices SET status = '在线待命', last_heartbeat = '2020-01-01 00:00:00' \
         WHERE device_id = 'RD-002'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let affected = gateway::sweep_stale_devices(&state).await.unwrap();
    assert_eq!(affected, 1, "只有带完整时间戳的超时设备会被判定");

    let status: String =
        sqlx::query_scalar("SELECT status FROM devices WHERE device_id = 'RD-002'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "离线");

    let alerts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM alerts WHERE alert_code LIKE 'AL-HB-%' AND content LIKE '%心跳超时%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(alerts, 1, "掉线应写一条心跳超时告警");

    // 演示设备的相对时间不受影响
    let demo: String = sqlx::query_scalar("SELECT status FROM devices WHERE device_id = 'RD-001'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_ne!(demo, "离线", "演示数据的相对时间不应被看门狗误判");
}
