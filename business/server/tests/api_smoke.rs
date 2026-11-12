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
        task_timeout_secs: 120,
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

/// 插入一张「真实 ROS 地图」基线（带 file_path + 米制原点），返回地图 ID。
///
/// 演示基线（`seed.rs`）没有地图包，坐标只能按示意坐标系处理、不允许下发；
/// 需要验证「能下发」的用例自己造一张真图。
async fn insert_real_map(pool: &sqlx::SqlitePool) -> String {
    let map_id = "TEST-MAP@v1.0";
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, map_name, version, frame_id, crs, origin,
            resolution, status, map_width, map_height, file_path, source, created_at)
         VALUES (?1, 1, 'TEST-MAP', 'v1.0', 'test_map', 'ROS map_server', '-14.85, -13.45, 0.0',
            '0.05 m/px', '已发布', 1088, 940, 'maps/test-model.zip', 'backend', '2026-11-01 00:00:00')",
    )
    .bind(map_id)
    .execute(pool)
    .await
    .unwrap();
    map_id.to_string()
}

/// 插入一条路线（坐标语义由 `data` 自己决定：米制 / 画布像素）。
async fn insert_route(
    pool: &sqlx::SqlitePool,
    code: &str,
    map_id: &str,
    data: serde_json::Value,
) -> i64 {
    sqlx::query(
        "INSERT INTO routes (route_code, route_name, park_id, status, version, loop_mode,
            default_speed, route_data, point_count, distance_m, duration_s, battery_pct,
            map_version_id, created_at, updated_at, created_by)
         VALUES (?1, ?2, 1, '已发布', 'v1.0', '往返', 1.2, ?3, 2, 14.1,
            12, 0, ?4, '2026-11-01 00:00:00', '2026-11-01 00:00:00', 1)",
    )
    .bind(code)
    .bind(format!("{code} 冒烟路线"))
    .bind(data.to_string())
    .bind(map_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

/// 插入一条坐标已是地图米制、且全部落在图内的可下发路线（园区 1）。
async fn insert_dispatchable_route(pool: &sqlx::SqlitePool, map_id: &str) -> i64 {
    insert_route(
        pool,
        "RT-TEST-1",
        map_id,
        serde_json::json!({
            "schemaVersion": 3,
            "coordFrame": "map",
            "loop": "往返",
            "points": [
                {"name": "P1", "x": -10.0, "y": -10.0, "dwell": 0, "radius": 0.3, "must": true,
                 "heading": 0, "onFail": "跳过继续", "actions": []},
                {"name": "P2", "x": 0.0, "y": 0.0, "dwell": 5, "radius": 0.3, "must": true,
                 "heading": 90, "onFail": "跳过继续", "actions": []}
            ],
            "legs": [{"mode": "沿道路", "speed": 1.2, "avoid": "标准", "actions": []}],
            "returnLeg": null
        }),
    )
    .await
}

/// 取一台已绑定园区的演示设备主键
async fn bound_device_pk(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT id FROM devices WHERE park_id = 1 LIMIT 1")
        .fetch_one(pool)
        .await
        .expect("演示数据应包含已绑定园区的设备")
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
async fn synced_sources_scopes_by_device() {
    // 地图跟着设备走：设备没绑园区也能判重，且不会把别的设备的图算成已同步
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    for (id, device, source) in [
        ("dev-map-a", "RD-001", "/ota/alg_data/map/history_map/a"),
        ("dev-map-b", "RD-002", "/ota/alg_data/map/history_map/b"),
    ] {
        sqlx::query(
            "INSERT INTO map_versions
                (id, park_id, device_code, map_name, version, status, created_at, source, source_ref)
             VALUES (?1, NULL, ?2, ?3, 'v1.0', '部分标定', '2026-01-01 00:00:00',
                'robot-mapping', ?4)",
        )
        .bind(id)
        .bind(device)
        .bind(format!("map-{device}"))
        .bind(source)
        .execute(&pool)
        .await
        .unwrap();
    }

    let app = build_router(state);
    let token = login(&app).await;
    let response = app
        .oneshot(
            Request::get("/api/map-versions/synced-sources?deviceId=RD-001")
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
        sources.contains(&serde_json::json!("/ota/alg_data/map/history_map/a")),
        "本设备已回传的地图应算已同步: {body}"
    );
    assert!(
        !sources.contains(&serde_json::json!("/ota/alg_data/map/history_map/b")),
        "别的设备的地图不应影响本设备判重: {body}"
    );
}

#[tokio::test]
async fn creating_park_adopts_unbound_device_map() {
    // 「设备建图 → 人创建园区时挑一张自己的设备地图绑为基线」
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, device_code, map_name, version, status, created_at, source, source_ref)
         VALUES ('RD-002@history-0715', NULL, 'RD-002', 'RD-002@history-0715', 'v1.0',
            '部分标定', '2026-01-01 00:00:00', 'robot-mapping',
            '/ota/alg_data/map/history_map/0715')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);
    let token = login(&app).await;
    let response = app
        .oneshot(
            Request::post("/api/parks")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"parkName":"设备地图园区","area":1.2,"mapVersionId":"RD-002@history-0715"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(
        body["data"]["mapVersionId"], "RD-002@history-0715",
        "创建园区时应识别选中的设备地图: {body}"
    );
    let park_id = body["data"]["id"].as_i64().expect("园区应有 id");
    let owner: Option<i64> = sqlx::query_scalar("SELECT park_id FROM map_versions WHERE id = ?1")
        .bind("RD-002@history-0715")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(park_id),
        "被选中的设备地图应认领到新园区"
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
        // 定位与「谁在控制」：缺了就看不出「接管了但摇杆不走」
        assert!(device.get("locStatus").is_some(), "设备缺少 locStatus");
        assert!(device.get("activeMapId").is_some(), "设备缺少 activeMapId");
        assert!(device.get("ctrlSource").is_some(), "设备缺少 ctrlSource");
        assert!(
            device.get("motionStatus").is_some(),
            "设备缺少 motionStatus"
        );
    }
}

/// 非管理员驾驶舱只能看到「归属于自己」的数据。
///
/// 设备归属走中台分配关系，本地运营账号没有中台 id 时不会退回全量；
/// 园区 / 任务 / 告警全部按创建人或可见园区收敛。
#[tokio::test]
async fn dashboard_is_scoped_to_current_user() {
    let (state, _db) = test_state().await;
    let password_hash = bcrypt::hash("operator123", bcrypt::DEFAULT_COST).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, display_name, role, created_at)
         VALUES ('operator', ?1, '运营', 'operator', ?2)",
    )
    .bind(password_hash)
    .bind(robodog_ops_server::db::now())
    .execute(&state.db)
    .await
    .unwrap();

    let app = build_router(state.clone());
    let token = login_as(&app, "operator", "operator123").await;
    let dashboard = app
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
    let data = &body["data"];

    // 演示数据全部由 admin 创建，新运营账号一个都不该看到
    assert_eq!(data["kpis"]["parks"]["value"], 0, "不应看到别人的园区");
    assert_eq!(data["kpis"]["parks"]["zones"], 0, "区域数应跟随可见园区");
    assert_eq!(data["kpis"]["todayTasks"]["value"], 0, "不应看到别人的任务");
    assert_eq!(
        data["kpis"]["pendingAlerts"]["value"], 0,
        "不应看到别人的告警"
    );
    assert!(data["parks"].as_array().unwrap().is_empty());
    assert!(data["alerts"].as_array().unwrap().is_empty());
    assert!(data["timeline"].as_array().unwrap().is_empty());
}

/// 驾驶舱趋势 / 最新动态必须来自真实业务表，而不是预置统计或演示数组。
#[tokio::test]
async fn dashboard_trend_and_timeline_use_live_data() {
    let (state, _db) = test_state().await;

    // 预置统计改成不可能的值：如果驾驶舱还读 daily_stats，这里会露馅
    sqlx::query("UPDATE daily_stats SET plan_count = 999, done_count = 999")
        .execute(&state.db)
        .await
        .unwrap();

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    sqlx::query(
        "INSERT INTO tasks (task_code, task_name, task_status, plan_start,
            created_at, updated_at, created_by)
         VALUES ('TK-TREND-01', '趋势回归任务', '已完成', ?1, ?2, ?2, 1)",
    )
    .bind(format!("{today} 08:00"))
    .bind(format!("{today} 08:00:00"))
    .execute(&state.db)
    .await
    .unwrap();
    // 用一条未来时间的告警保证它一定排在最新动态首位
    sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, status, occurred_at, created_at)
         VALUES ('AL-TREND-01', '警告', '趋势回归告警', '未处理', '2099-01-01 00:00:00', ?1)",
    )
    .bind(robodog_ops_server::db::now())
    .execute(&state.db)
    .await
    .unwrap();

    let app = build_router(state.clone());
    let token = login(&app).await;
    let dashboard = app
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
    let data = &body["data"];

    for value in data["trend"]["plan"].as_array().unwrap() {
        assert_ne!(value.as_i64(), Some(999), "趋势不能读预置的 daily_stats");
    }
    assert!(
        data["kpis"]["todayTasks"]["value"]
            .as_i64()
            .unwrap_or(0)
            >= 1,
        "今日任务应按计划日期/创建日期统计"
    );
    assert!(
        data["trend"]["done"]
            .as_array()
            .and_then(|rows| rows.last())
            .and_then(Value::as_i64)
            .unwrap_or(0)
            >= 1,
        "应统计到真实完成的任务"
    );
    assert_eq!(
        data["timeline"][0]["eventType"], "alert",
        "最新动态应包含真实告警"
    );
}

async fn login_as(app: &axum::Router, username: &str, password: &str) -> String {
    let request = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"username":"{username}","password":"{password}"}}"#
        )))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "账号 {username} 应能登录"
    );
    let body = json_body(response).await;
    body["data"]["token"]
        .as_str()
        .expect("登录响应缺少 token")
        .to_string()
}

/// 本体动作白名单包含 1302 的三种模式（通用/原地/导航）。
///
/// 「能站立但摇杆不走」的现场根因之一是本体停在原地模式：1304 的 lx/ly 在
/// 原地模式下按协议「无作用」。平台必须能手动切回通用模式。
#[tokio::test]
async fn body_actions_include_gait_modes() {
    let (state, _db) = test_state().await;
    let app = build_router(state.clone());
    let token = login(&app).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM devices LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("演示数据应包含设备");
    // 模式切换和其它本体动作一样要求接管态：先把设备置为在线并接管
    sqlx::query("UPDATE devices SET status = '在线待命' WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();
    let taken = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{id}/teleop/take"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(taken.status(), StatusCode::OK, "应先能接管");

    for (action, label) in [
        ("generic_mode", "通用模式"),
        ("in_place_mode", "原地模式"),
        ("navigation_mode", "导航模式"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/devices/{id}/action"))
                    .header(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(format!(r#"{{"action":"{action}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{action} 应被接受");
        let body = json_body(response).await;
        assert_eq!(body["data"]["label"], label);
        assert_eq!(body["data"]["action"], action);
    }
}

/// 「重新定位」：机器狗不在地图上时的人工恢复入口。
///
/// 三个必须成立的契约：
/// 1. 解析不出地图 → 400 并提示显式指定 `mapId`（绝不能静默按错的图重定位）；
/// 2. 设备离线 → 400「未送达」，不能记成已下发；
/// 3. 设备在线 → WS 下发 `重新定位`，payload 带 `mapId` 与初始位姿。
#[tokio::test]
async fn locate_device_dispatches_relocalize_with_pose() {
    let (state, _db) = test_state().await;
    let app = build_router(state.clone());
    let token = login(&app).await;
    let (id, code): (i64, String) = sqlx::query_as("SELECT id, device_id FROM devices LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("演示数据应包含设备");

    // 1) 既没有已载地图、也没有园区基线 → 要求显式 mapId
    sqlx::query("UPDATE devices SET park_id = NULL, active_map_id = NULL WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{id}/locate"))
                .header(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert!(
        body["msg"].as_str().unwrap_or_default().contains("mapId"),
        "应提示显式指定 mapId，实际：{}",
        body["msg"]
    );

    // 2) 指定地图但设备不在线 → 未送达必须显式失败
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{id}/locate"))
                .header(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"mapId":"ROS-MAP-1@v1.0"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert!(
        body["msg"].as_str().unwrap_or_default().contains("未送达"),
        "离线设备必须报未送达，实际：{}",
        body["msg"]
    );

    // 3) 设备在线 → 指令带地图与位姿下发到设备
    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    state.device_connections.write().await.insert(code.clone(), tx);
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{id}/locate"))
                .header(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"mapId":"ROS-MAP-1@v1.0","x":1.5,"y":-2.0,"heading":90}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "在线设备应下发成功");
    let frame = rx.recv().await.expect("应收到下发的指令帧");
    assert_eq!(frame.cmd_type, "remote_command");
    assert_eq!(frame.payload["command"], "重新定位");
    // 线协议形态：{ command, params: { dispatchedAt, operator, params: <附加参数> } }
    assert_eq!(frame.payload["params"]["params"]["mapId"], "ROS-MAP-1@v1.0");
    assert_eq!(frame.payload["params"]["params"]["pose"]["x"], 1.5);
    assert_eq!(frame.payload["params"]["params"]["pose"]["heading"], 90.0);
}

/// 重新定位的入参校验：坐标必须成对出现，heading 缺省按 0 度
#[test]
fn locate_request_requires_paired_coordinates() {
    use robodog_ops_server::models::LocateRequest;

    let full = LocateRequest {
        map_id: None,
        x: Some(1.0),
        y: Some(2.0),
        heading: None,
    };
    let pose = full.pose().expect("成对坐标应合法").expect("应生成 pose");
    assert_eq!(pose["x"], 1.0);
    assert_eq!(pose["heading"], 0.0);

    let partial = LocateRequest {
        map_id: None,
        x: Some(1.0),
        y: None,
        heading: None,
    };
    assert!(partial.pose().is_err(), "缺 y 必须拒绝");

    let none = LocateRequest {
        map_id: None,
        x: None,
        y: None,
        heading: None,
    };
    assert!(none.pose().expect("缺省坐标合法").is_none());
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
        // 演示路线绑的是没有地图包的示意基线，不允许下发；这里造一条真图路线
        let map_id = insert_real_map(&pool).await;
        let route_id = insert_dispatchable_route(&pool, &map_id).await;
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

        let payload = format!(
            r#"{{"taskName":"冒烟巡检","routeId":{route_id},"requestId":"smoke-task-1"}}"#
        );
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
                        .body(Body::from(payload.clone()))
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
        task_timeout_secs: 120,
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
            Some(&park),
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

/// 卡在「执行中」的任务：设备长时间不报进度 → 看门狗掰到异常，并留一条可读事件。
///
/// 现场踩过：网页点「开始执行」只改了库里的状态（没真正下发），客户端回一条
/// 「执行中 / 0%」后彻底静默，任务永久停在执行中。
#[tokio::test]
async fn task_watchdog_marks_stuck_task_abnormal() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    // 真机设备（完整时间戳）+ 一条停留在「执行中」、很久没更新的任务
    sqlx::query(
        "UPDATE devices SET status = '执行任务', current_task_name = '卡住的任务',
            last_heartbeat = '2020-01-01 00:00:00' WHERE device_id = 'RD-002'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let device_pk: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE device_id = 'RD-002'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let task_id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, device_id, task_status, progress,
            created_at, updated_at)
         VALUES ('TK-STUCK-1', '卡住的任务', ?1, '执行中', 5,
            '2020-01-01 00:00:00', '2020-01-01 00:00:00')
         RETURNING id",
    )
    .bind(device_pk)
    .fetch_one(&pool)
    .await
    .unwrap();

    let affected = gateway::sweep_stuck_tasks(&state).await.unwrap();
    assert_eq!(affected, 1, "只有超时未上报的执行中任务会被判定");

    let status: String = sqlx::query_scalar("SELECT task_status FROM tasks WHERE id = ?1")
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "异常");

    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_events WHERE task_id = ?1 AND title LIKE '%执行中断%'",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1, "判定异常应留一条事件");

    let (device_status, current_task): (String, String) =
        sqlx::query_as("SELECT status, current_task_name FROM devices WHERE id = ?1")
            .bind(device_pk)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(device_status, "在线待命", "设备任务占用应释放");
    assert_eq!(current_task, "—");

    // 演示任务的设备用的是相对时间，不能被误判
    let demo: String =
        sqlx::query_scalar("SELECT task_status FROM tasks WHERE task_code = 'TK-1109-01'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(demo, "执行中", "演示数据不受任务看门狗影响");
}

/// 「执行中」不能只改状态：设备离线时接口直接报错，任务保持原状态。
#[tokio::test]
async fn starting_task_on_offline_device_is_rejected() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let app = build_router(state.clone());
    let token = login(&app).await;

    // 演示路线绑的是示意基线；这里用一条真图路线，确保走到「设备未连接」而不是坐标守卫
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_dispatchable_route(&pool, &map_id).await;
    let device_pk = bound_device_pk(&pool).await;
    let (task_id, task_code): (i64, String) = sqlx::query_as(
        "INSERT INTO tasks (task_code, task_name, device_id, park_id, route_id, task_status,
            progress, created_at, updated_at, created_by)
         VALUES ('TK-OFFLINE-1', '离线设备任务', ?1, 1, ?2, '待执行', 0,
            '2026-11-12 00:00:00', '2026-11-12 00:00:00', 1)
         RETURNING id, task_code",
    )
    .bind(device_pk)
    .bind(route_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::put(format!("/api/tasks/{task_id}/status"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"taskStatus":"执行中"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "设备未连接时不应假装下发成功"
    );
    let body = json_body(response).await;
    assert!(
        body["msg"].as_str().unwrap_or_default().contains("未连接"),
        "错误信息应说明设备未连接，实际: {body}"
    );

    let status: String = sqlx::query_scalar("SELECT task_status FROM tasks WHERE id = ?1")
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "待执行", "任务 {task_code} 不应被改成执行中");

    // 显式下发接口同理：设备离线 → 400，状态不变
    let response = app
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/dispatch"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// 未换算的路线（画布像素）在建任务时就被拒绝，且不会留下半截任务。
#[tokio::test]
async fn canvas_pixel_route_is_rejected_before_task_creation() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_route(
        &pool,
        "RT-LEGACY-1",
        &map_id,
        serde_json::json!({
            "schemaVersion": 2,
            "loop": "往返",
            "points": [
                {"name": "P1", "x": 335, "y": 120},
                {"name": "P2", "x": 555, "y": 145}
            ],
            "legs": [],
            "returnLeg": null
        }),
    )
    .await;
    let device_pk = bound_device_pk(&pool).await;

    let app = build_router(state.clone());
    let token = login(&app).await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();

    let body = format!(
        r#"{{"taskName":"像素路线任务","deviceId":{device_pk},"parkId":1,"routeId":{route_id}}}"#
    );
    let response = app
        .oneshot(
            Request::post("/api/tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("像素"),
        "错误信息应说明航点还是画布像素，实际: {message}"
    );

    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, before, "坐标校验失败时不应插入任务");
}

/// 米制但超出地图范围的航点同样拒绝下发（不能等到本体报 13898）。
#[tokio::test]
async fn out_of_bounds_route_is_rejected_before_task_creation() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_route(
        &pool,
        "RT-OOB-1",
        &map_id,
        serde_json::json!({
            "schemaVersion": 3,
            "coordFrame": "map",
            "loop": "往返",
            "points": [
                {"name": "P1", "x": 335.0, "y": 120.0},
                {"name": "P2", "x": 555.0, "y": 145.0}
            ],
            "legs": [],
            "returnLeg": null
        }),
    )
    .await;
    let device_pk = bound_device_pk(&pool).await;

    let app = build_router(state.clone());
    let token = login(&app).await;
    let body = format!(
        r#"{{"taskName":"越界路线任务","deviceId":{device_pk},"parkId":1,"routeId":{route_id}}}"#
    );
    let response = app
        .oneshot(
            Request::post("/api/tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("超出地图范围"),
        "错误信息应指出越界，实际: {message}"
    );
}

/// 历史路线的画布像素坐标：启动迁移一次性换算成地图米，且可重复执行。
#[tokio::test]
async fn legacy_route_coordinates_are_migrated_once() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_route(
        &pool,
        "RT-MIGRATE-1",
        &map_id,
        serde_json::json!({
            "schemaVersion": 2,
            "loop": "往返",
            "points": [
                {"name": "P1", "x": 335, "y": 120},
                {"name": "P2", "x": 555, "y": 145}
            ],
            "legs": [],
            "returnLeg": null
        }),
    )
    .await;

    let converted = robodog_ops_server::coords::migrate_route_coordinates(&pool)
        .await
        .unwrap();
    assert!(converted >= 1, "至少应迁移本条路线");

    let raw: String = sqlx::query_scalar("SELECT route_data FROM routes WHERE id = ?1")
        .bind(route_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let data: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(data["coordFrame"], "map");
    assert_eq!(data["schemaVersion"], 3);
    let x = data["points"][0]["x"].as_f64().unwrap();
    let y = data["points"][0]["y"].as_f64().unwrap();
    assert!((x - 5.71).abs() < 0.05, "x 应换算为地图米，实际 {x}");
    assert!((y - 21.29).abs() < 0.05, "y 应换算为地图米（含 y 翻转），实际 {y}");
    // 里程按米重算：旧口径是「像素 × 1.2」
    let distance: f64 = sqlx::query_scalar("SELECT distance_m FROM routes WHERE id = ?1")
        .bind(route_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(distance < 30.0, "里程应按米重算，实际 {distance}");

    // 幂等：已带 coordFrame=map 的路线不再换算
    let again = robodog_ops_server::coords::migrate_route_coordinates(&pool)
        .await
        .unwrap();
    assert_eq!(again, 0, "迁移必须幂等");
}

/// 取一条任务（与 `handlers::tasks::TASK_SELECT` 同列），用于组装下发载荷。
async fn fetch_task_row(pool: &sqlx::SqlitePool, id: i64) -> robodog_ops_server::models::Task {
    sqlx::query_as(
        "SELECT t.id, t.task_code, t.task_name, t.task_type, t.execute_mode, t.level,
            t.device_id, d.device_id AS device_code, t.park_id, p.park_name, t.zone_id,
            z.zone_name, t.route_id, r.route_code, r.route_name, t.plan_start, t.plan_end,
            t.task_status, t.progress, t.dispatch_policy, t.description, t.robot_task_id,
            t.robot_stage, t.robot_updated_at, t.created_at, t.updated_at, t.created_by
         FROM tasks t
         LEFT JOIN devices d ON d.id = t.device_id
         LEFT JOIN parks p ON p.id = t.park_id
         LEFT JOIN zones z ON z.id = t.zone_id
         LEFT JOIN routes r ON r.id = t.route_id
         WHERE t.id = ?1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// 下发载荷必须带本体真实目录名（`source_ref`），而不是业务 ID 的合成名。
///
/// 现场事故：`map_path` 用了 `ROS-MAP-…_v1.0`，本体上并不存在这个目录，
/// 5000 只回 `13898 map not found or invalid (missing map.yaml)`。
#[tokio::test]
async fn dispatch_payload_prefers_robot_map_dir() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    sqlx::query(
        "UPDATE map_versions SET source_ref = '/ota/alg_data/map/history_map/2026_11_10_16_15_06',
            file_name = '2026_11_10_16_15_06.zip' WHERE id = ?1",
    )
    .bind(&map_id)
    .execute(&pool)
    .await
    .unwrap();
    let route_id = insert_dispatchable_route(&pool, &map_id).await;
    let device_pk = bound_device_pk(&pool).await;
    let task_id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, device_id, park_id, route_id, task_status,
            progress, created_at, updated_at, created_by)
         VALUES ('TK-MAPDIR-1', '地图目录任务', ?1, 1, ?2, '待执行', 0,
            '2026-11-12 00:00:00', '2026-11-12 00:00:00', 1)
         RETURNING id",
    )
    .bind(device_pk)
    .bind(route_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let task = fetch_task_row(&pool, task_id).await;
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .expect("坐标合法时应能组装载荷");
    let map = payload.map.expect("应带地图绑定");
    assert_eq!(map.map_dir, "2026_11_10_16_15_06");
    assert_eq!(map.map_path, "2026_11_10_16_15_06", "map_path 也要用真实目录名");
    assert_eq!(map.map_id, map_id);
}

/// 没有 `source_ref` 时退回业务 ID 合成名（客户端会再用 4013 列表核对）。
#[tokio::test]
async fn dispatch_payload_falls_back_to_synthetic_map_dir() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_dispatchable_route(&pool, &map_id).await;
    let device_pk = bound_device_pk(&pool).await;
    let task_id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, device_id, park_id, route_id, task_status,
            progress, created_at, updated_at, created_by)
         VALUES ('TK-MAPDIR-2', '无目录名任务', ?1, 1, ?2, '待执行', 0,
            '2026-11-12 00:00:00', '2026-11-12 00:00:00', 1)
         RETURNING id",
    )
    .bind(device_pk)
    .bind(route_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let task = fetch_task_row(&pool, task_id).await;
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .unwrap();
    let map = payload.map.unwrap();
    assert_eq!(map.map_dir, robodog_ops_server::mapdir::synthetic_dir(&map_id));
}

/// 中台重复导入的包按「同名 zip」认回本体目录名，且可重复执行。
#[tokio::test]
async fn map_source_ref_backfills_from_same_package_name() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();

    // 设备回传过的那份：有 source_ref
    let robot_map = insert_real_map(&pool).await;
    sqlx::query(
        "UPDATE map_versions SET id = 'ROBOT-MAP-1', source = 'robot-mapping',
            source_ref = '/ota/alg_data/map/history_map/2026_11_10_16_15_06',
            file_name = '2026_11_10_16_15_06.zip' WHERE id = ?1",
    )
    .bind(&robot_map)
    .execute(&pool)
    .await
    .unwrap();
    // 中台导入的同一个包：只有 file_name
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, map_name, version, origin, resolution,
            status, map_width, map_height, source, file_name, created_at)
         VALUES ('ROS-MAP-DUP@v1.0', 1, 'ROS-MAP-DUP', 'v1.0', '-1.0, -1.0, 0.0', '0.05 m/px',
            '已发布', 1088, 940, 'backend', '2026_11_10_16_15_06.zip', '2026-11-12 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // ROS map_server 的 model.zip 不能当成目录名
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, map_name, version, origin, resolution,
            status, map_width, map_height, source, file_name, created_at)
         VALUES ('ROS-MAP-MODEL@v1.0', 1, 'ROS-MAP-MODEL', 'v1.0', '-1.0, -1.0, 0.0', '0.05 m/px',
            '已发布', 1088, 940, 'backend', 'model.zip', '2026-11-12 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let filled = robodog_ops_server::mapdir::backfill_source_refs(&pool)
        .await
        .unwrap();
    assert_eq!(filled, 1, "只应回填同名 zip 的那条");

    let source_ref: Option<String> =
        sqlx::query_scalar("SELECT source_ref FROM map_versions WHERE id = 'ROS-MAP-DUP@v1.0'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        source_ref.as_deref(),
        Some("/ota/alg_data/map/history_map/2026_11_10_16_15_06")
    );

    let model_ref: Option<String> =
        sqlx::query_scalar("SELECT source_ref FROM map_versions WHERE id = 'ROS-MAP-MODEL@v1.0'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(model_ref, None, "model.zip 不是本体目录名");

    // 幂等
    let again = robodog_ops_server::mapdir::backfill_source_refs(&pool)
        .await
        .unwrap();
    assert_eq!(again, 0);
}
