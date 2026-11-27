//! 接口冒烟集成测试。
//!
//! 与 `src/` 里的内联单测不同，这里把服务当成黑盒：真实迁移 + 播种 + 路由，
//! 通过 HTTP 请求验证契约。覆盖：
//!
//! - `GET /api/health` 健康检查；
//! - 未登录访问受保护接口返回 401；
//! - `admin/admin123` 登录后能取到令牌；
//! - `GET /api/dashboard/summary` 驾驶舱接口（包含 `mapDevices` 的设备列映射，
//!   历史上漏选设备列曾导致首页 500，这里作为回归用例）。

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
        https_port: 0,
        tls_cert: None,
        tls_key: None,
        web_dist: dir.join("web-dist"),
        backend_db_path: dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: dir.join("map_ingest"),
        recording_dir: dir.join("recordings"),
        photo_dir: dir.join("photos"),
        recording_keep_days: 30,
        photo_keep_days: 90,
        service_token: service_token.map(str::to_string),
        zentai_ws_url: None,
        device_timeout_secs: 90,
        task_timeout_secs: 120,
        task_stuck_secs: 600,
        backpack_base_url: None,
        backpack_webrtc_url: None,
        backpack_browser_url: None,
        mediamtx_url: None,
        backpack_tunnel_device: None,
        backpack_devices: Vec::new(),
        point_ai: "auto".to_string(),
        ai_window_max_secs: 300,
        public_url: None,
        wecom_enabled: false,
        wecom_webhook: None,
        wecom_min_level: "警告".to_string(),
        wecom_mention_all: false,
    });

    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");

    let state = AppState {
        db: pool,
        notifier: robodog_ops_server::notify::Notifier::disabled(),
        config,
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
        phone_audio: Default::default(),
        ai_patrol: Default::default(),
        caches: Default::default(),
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

/// 任务规划里的「巡检路线」下拉直接按后端顺序展示，模板必须最近更新在前。
#[tokio::test]
async fn route_list_is_ordered_by_recent_update() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let (old_id,): (i64,) = sqlx::query_as("SELECT id FROM routes ORDER BY id ASC LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("演示数据应包含路线");
    sqlx::query("UPDATE routes SET updated_at = '2020-01-01 00:00:00' WHERE id = ?1")
        .bind(old_id)
        .execute(&pool)
        .await
        .unwrap();
    let new_id = sqlx::query(
        "INSERT INTO routes (route_code, route_name, park_id, status, version, loop_mode,
            default_speed, route_data, point_count, distance_m, duration_s, battery_pct,
            created_at, updated_at, created_by)
         VALUES ('RT-NEWEST', '最新编排的路线', 1, '草稿', 'v1.0', '往返', 1.2, '{}', 0, 0,
            0, 0, '2026-11-01 00:00:00', '2030-01-01 00:00:00', 1)",
    )
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    let app = build_router(state);
    let token = login(&app).await;
    let response = app
        .oneshot(
            Request::get("/api/routes")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let ids: Vec<i64> = body["data"]
        .as_array()
        .expect("路线列表应是数组")
        .iter()
        .map(|route| route["id"].as_i64().expect("路线缺少 id"))
        .collect();
    assert_eq!(
        ids.first().copied(),
        Some(new_id),
        "最新更新的路线应排第一: {ids:?}"
    );
    assert_eq!(
        ids.last().copied(),
        Some(old_id),
        "最久未动的路线应排最后: {ids:?}"
    );
}

/// 路线模板只删「没人引用」的：被巡检计划引用的模板必须拒删。
///
/// 以前 `DELETE /routes/{id}` 会先把引用它的任务 `route_id` 置空再删模板，等于把
/// 巡检计划静默改成「临时路径」——计划名字还在、点开没有航点，下发时才暴露。
#[tokio::test]
async fn only_unreferenced_route_templates_can_be_deleted() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let used_route = insert_dispatchable_route(&pool, &map_id).await;
    let free_route = insert_route(
        &pool,
        "RT-FREE-1",
        &map_id,
        serde_json::json!({
            "schemaVersion": 3,
            "coordFrame": "map",
            "loop": "往返",
            "points": [],
            "legs": [],
            "returnLeg": null
        }),
    )
    .await;
    let device_pk = bound_device_pk(&pool).await;
    let task_id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, device_id, park_id, route_id, task_status,
            progress, created_at, updated_at, created_by)
         VALUES ('TK-ROUTE-DEL', '引用路线模板的计划', ?1, 1, ?2, '待执行', 0,
            '2026-11-12 00:00:00', '2026-11-12 00:00:00', 1)
         RETURNING id",
    )
    .bind(device_pk)
    .bind(used_route)
    .fetch_one(&pool)
    .await
    .unwrap();

    let app = build_router(state);
    let token = login(&app).await;

    // 被引用 → 400，模板和计划的绑定都不能动
    let response = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/routes/{used_route}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default();
    assert!(msg.contains("巡检计划引用"), "提示应说明被计划引用: {msg}");
    let still_there: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE id = ?1")
        .bind(used_route)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(still_there, 1, "被引用的模板不能被删掉");
    let bound: Option<i64> = sqlx::query_scalar("SELECT route_id FROM tasks WHERE id = ?1")
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(bound, Some(used_route), "拒删时不能把计划的路线置空");

    // 没有引用的模板 → 正常删除
    let response = app
        .oneshot(
            Request::delete(format!("/api/routes/{free_route}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"], true);
    let gone: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM routes WHERE id = ?1")
        .bind(free_route)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(gone, 0, "没有引用的模板应被删掉");
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
    // 「设备建图 → 人在中台发布 → 创建园区时挑一张自己的设备地图绑为基线」
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, device_code, map_name, version, status, created_at, source, source_id,
             source_ref)
         VALUES ('RD-002@history-0715', NULL, 'RD-002', 'RD-002@history-0715', 'v1.0',
            '已发布', '2026-01-01 00:00:00', 'robot-mapping', '88',
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
    assert_eq!(owner, Some(park_id), "被选中的设备地图应认领到新园区");
}

/// 「设备先绑了别的园区 → 新园区编辑时看不到任何可选地图」的回归。
///
/// 2026-11-24 现场：设备在业务平台绑的是 test 园区，回传的图全被 test 认领；
/// 广汇产业园二期编辑时下拉里一张图都没有（`deviceMapOptions` 只列 `park_id IS NULL`）。
/// 现在**不同园区可以绑定同一张地图基线**：保存只写本园区的基线，
/// `map_versions.park_id`（采集 / 主归属园区）和旧园区的基线都不动。
/// 地图状态由中台统一管理，所以这里用的是「中台已发布」的图——只有已发布才能当基线。
#[tokio::test]
async fn parks_can_share_the_same_map_baseline() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, device_code, map_name, version, status, created_at, source, source_id,
             source_ref)
         VALUES ('RD-002@history-0715', 1, 'RD-002', 'RD-002@history-0715', 'v1.0',
            '已发布', '2026-01-01 00:00:00', 'robot-mapping', '95',
            '/ota/alg_data/map/history_map/0715')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE parks SET map_version_id = 'RD-002@history-0715', map_status = '部分标定'
         WHERE id = 1",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);
    let token = login(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::put("/api/parks/2")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"parkName":"临港物流基地","area":2.4,"mapVersionId":"RD-002@history-0715"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(
        body["data"]["mapVersionId"], "RD-002@history-0715",
        "编辑园区时应能绑定其他园区在用的设备地图: {body}"
    );

    let owner: Option<i64> = sqlx::query_scalar("SELECT park_id FROM map_versions WHERE id = ?1")
        .bind("RD-002@history-0715")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, Some(1), "共用时不该把地图归属搬走");

    let (old_baseline, old_status): (Option<String>, String) =
        sqlx::query_as("SELECT map_version_id, map_status FROM parks WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        old_baseline.as_deref(),
        Some("RD-002@history-0715"),
        "原园区的基线不该被共用方拖走"
    );
    assert_eq!(old_status, "部分标定");
    // 共用方拿到的是「已标定」（基线是一张中台已发布的图）
    let shared_status: String = sqlx::query_scalar("SELECT map_status FROM parks WHERE id = 2")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(shared_status, "已标定");

    // 园区可用地图接口要把共用的基线算进去（新建路线的版本下拉靠它）
    let maps = app
        .oneshot(
            Request::get("/api/parks/2/maps")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(maps.status(), StatusCode::OK);
    let maps = json_body(maps).await;
    assert!(
        maps["data"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["id"] == "RD-002@history-0715")),
        "园区可用地图应包含共用的基线: {maps}"
    );
}

/// 地图状态由中台统一管理：**未发布**（草稿 / 部分标定）的图不能新绑为园区基线，
/// 只有中台「已发布」的图可以。园区当前基线就是它时（中台把图退回草稿）原样提交要放过，
/// 否则编辑园区信息会被自己已经绑着的基线下卡住。
#[tokio::test]
async fn park_baseline_requires_a_published_map() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, device_code, map_name, version, status, created_at, source, source_id,
             source_ref)
         VALUES ('RD-009@draft', NULL, 'RD-009', 'RD-009@draft', 'v1.0', '部分标定',
            '2026-01-01 00:00:00', 'robot-mapping', '77',
            '/ota/alg_data/map/history_map/draft')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);
    let token = login(&app).await;
    let put_baseline = |id: &'static str| {
        Request::put("/api/parks/1")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(
                r#"{{"parkName":"智造园区","area":12.5,"mapVersionId":"{id}"}}"#
            )))
            .unwrap()
    };

    let response = app.clone().oneshot(put_baseline("RD-009@draft")).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "未发布的地图不能绑成园区基线"
    );
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default();
    assert!(
        msg.contains("已发布"),
        "错误要说清楚「状态由中台管理、先在中台发布」: {msg}"
    );

    // 已经是它：原样提交不能报错（中台把某张图退回草稿时也要能继续编辑园区）
    sqlx::query("UPDATE parks SET map_version_id = 'RD-009@draft' WHERE id = 1")
        .execute(&pool)
        .await
        .unwrap();
    let response = app.oneshot(put_baseline("RD-009@draft")).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "保持现状的基线不该被状态门禁挡住"
    );
}

/// 共用不能变成「谁都能挂图」：本运营账号对采集设备没有权限时拒绝。
#[tokio::test]
async fn reclaiming_another_park_map_requires_device_access() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let password_hash = bcrypt::hash("operator123", bcrypt::DEFAULT_COST).unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, display_name, role, created_at)
         VALUES ('operator', ?1, '运营', 'operator', ?2)",
    )
    .bind(password_hash)
    .bind(robodog_ops_server::db::now())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, device_code, map_name, version, status, created_at, source, source_ref)
         VALUES ('RD-002@history-0715', 1, 'RD-002', 'RD-002@history-0715', 'v1.0',
            '部分标定', '2026-01-01 00:00:00', 'robot-mapping',
            '/ota/alg_data/map/history_map/0715')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);
    let token = login_as(&app, "operator", "operator123").await;
    // 自己的园区（`created_by` 是本账号，写权限没问题）
    let created = app
        .clone()
        .oneshot(
            Request::post("/api/parks")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"parkName":"运营园区","area":1.0}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let created = json_body(created).await;
    let park_id = created["data"]["id"].as_i64().expect("园区应有 id");

    let response = app
        .oneshot(
            Request::put(format!("/api/parks/{park_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"parkName":"运营园区","area":1.0,"mapVersionId":"RD-002@history-0715"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "没有设备权限的账号不该能改别人的地图归属"
    );
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default();
    assert!(
        msg.contains("无法绑定"),
        "错误提示应说清楚为什么拒绝: {msg}"
    );

    let owner: Option<i64> = sqlx::query_scalar("SELECT park_id FROM map_versions WHERE id = ?1")
        .bind("RD-002@history-0715")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, Some(1), "被拒绝时地图归属不许变");
}

/// 路线跟着园区共用同一张地图：别园区的图也能为它编路线（以前直接 400）。
#[tokio::test]
async fn route_can_use_a_map_shared_with_another_park() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, device_code, map_name, version, status, created_at, source, source_ref)
         VALUES ('RD-002@history-0715', 1, 'RD-002', 'RD-002@history-0715', 'v1.0',
            '部分标定', '2026-01-01 00:00:00', 'robot-mapping',
            '/ota/alg_data/map/history_map/0715')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE parks SET map_version_id = 'RD-002@history-0715', map_status = '部分标定'
         WHERE id = 2",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state);
    let token = login(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/routes")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"routeName":"共用基线路线","parkId":2,"mapVersionId":"RD-002@history-0715"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "共用地图应能编路线");
    let body = json_body(response).await;
    assert_eq!(body["data"]["mapVersionId"], "RD-002@history-0715");
    assert_eq!(body["data"]["parkId"], 2, "路线园区的基线来自共用图");

    let owner: Option<i64> = sqlx::query_scalar("SELECT park_id FROM map_versions WHERE id = ?1")
        .bind("RD-002@history-0715")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, Some(1), "编路线也不该把地图归属搬走");

    // 不指定地图时按园区当前基线自动选（共用基线同样生效）
    let auto = app
        .oneshot(
            Request::post("/api/routes")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"routeName":"自动取基线","parkId":2}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(auto.status(), StatusCode::OK);
    let auto = json_body(auto).await;
    assert_eq!(
        auto["data"]["mapVersionId"], "RD-002@history-0715",
        "未指定版本时应按园区当前基线（共用的）自动选: {auto}"
    );
}

#[tokio::test]
async fn legacy_robot_maps_get_device_code_from_zentai() {
    // 迁移 015 之前入库的机器人地图没有 device_code，启动时按中台 dog_map 回填，
    // 否则页面上永远显示「设备未登记」，没法对上中台那张图。
    let (state, _db) = test_state().await;
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, map_name, version, status, created_at, source, source_id, source_ref)
         VALUES ('ROS-MAP-77@v1.0', 1, 'legacy-map', 'v1.0', '部分标定',
            '2026-01-01 00:00:00', 'robot-mapping', '77', '/ota/alg_data/map/history_map/legacy')",
    )
    .execute(&state.db)
    .await
    .expect("插入历史地图失败");

    let zentai_path = state.config.backend_db_path.clone();
    let zentai = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&zentai_path)
                .create_if_missing(true),
        )
        .await
        .expect("创建假中台库失败");
    sqlx::query("CREATE TABLE dog_map (id INTEGER PRIMARY KEY, device_code TEXT)")
        .execute(&zentai)
        .await
        .unwrap();
    sqlx::query("INSERT INTO dog_map (id, device_code) VALUES (77, 'RD-0202222672400230')")
        .execute(&zentai)
        .await
        .unwrap();
    zentai.close().await;

    let updated = robodog_ops_server::db::backfill_map_device_codes(&state.db, &zentai_path)
        .await
        .expect("回填不应失败");
    assert_eq!(updated, 1, "应回填 1 行历史地图");

    let device_code: Option<String> =
        sqlx::query_scalar("SELECT device_code FROM map_versions WHERE id = 'ROS-MAP-77@v1.0'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(device_code.as_deref(), Some("RD-0202222672400230"));

    // 幂等：第二次跑不会再改任何行
    let again = robodog_ops_server::db::backfill_map_device_codes(&state.db, &zentai_path)
        .await
        .unwrap();
    assert_eq!(again, 0);
}

/// 「backend 地图库」的导入接口不再出现在控制台（园区基线只从业务库已发布的设备地图里选），
/// 这层门禁留在服务端：非「已发布」的中台图不能导入成业务地图 / 园区基线。
#[tokio::test]
async fn import_backend_map_requires_a_published_map() {
    let (state, _db) = test_state().await;
    let zentai_path = state.config.backend_db_path.clone();
    let zentai = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&zentai_path)
                .create_if_missing(true),
        )
        .await
        .expect("创建假中台库失败");
    sqlx::query(
        "CREATE TABLE dog_map (
            id INTEGER PRIMARY KEY, map_name TEXT, park_code TEXT, park_name TEXT,
            user_id INTEGER, file_name TEXT, file_path TEXT, file_format TEXT, file_size INTEGER,
            remarks TEXT, map_version TEXT, frame_id TEXT, crs TEXT, map_origin TEXT,
            resolution TEXT, floors TEXT, road_count INTEGER, nogo_count INTEGER,
            map_status TEXT, map_hash TEXT, publish_time TEXT, map_width INTEGER,
            map_height INTEGER, create_time TEXT, is_deleted INTEGER NOT NULL DEFAULT 0)",
    )
    .execute(&zentai)
    .await
    .unwrap();
    sqlx::query("CREATE TABLE p_sys_user (id INTEGER PRIMARY KEY, username TEXT)")
        .execute(&zentai)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO dog_map (id, map_name, file_name, file_path, file_format, map_version,
            map_status, create_time, is_deleted)
         VALUES (9, '2026_11_10_16_15_06', 'map.zip', 'maps/map.zip', 'zip', 'v1.0',
            '部分标定', '2026-11-23T03:06:59Z', 0)",
    )
    .execute(&zentai)
    .await
    .unwrap();

    let app = build_router(state);
    let token = login(&app).await;
    let import = |app: axum::Router, token: String| async move {
        app.oneshot(
            Request::post("/api/map-versions/import-backend")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"backendMapId":9,"parkId":1,"bind":true}"#))
                .unwrap(),
        )
        .await
        .unwrap()
    };

    let response = import(app.clone(), token.clone()).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "未发布的地图不能导入");
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default();
    assert!(
        msg.contains("已发布"),
        "错误要说清「先在中台发布」: {msg}"
    );

    // 中台发布之后才允许导入
    sqlx::query("UPDATE dog_map SET map_status = '已发布' WHERE id = 9")
        .execute(&zentai)
        .await
        .unwrap();
    zentai.close().await;
    let response = import(app, token).await;
    assert_eq!(response.status(), StatusCode::OK, "中台已发布后应能导入");
    let body = json_body(response).await;
    assert_eq!(
        body["data"]["sourceId"].as_str(),
        Some("9"),
        "导入行要挂上中台链接: {body}"
    );
    assert_eq!(body["data"]["status"].as_str(), Some("已发布"));
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

/// 入口 HTML 必须每次回源；带哈希的 `/assets/*` 才可以长缓存。
///
/// 2026-11-18 现场：发布新版本后浏览器仍按 `Last-Modified` 吃旧 bundle，
/// 「设备管理还显示未配置视频地址」，必须强制刷新才恢复。
#[tokio::test]
async fn static_index_is_not_cached_and_assets_are_immutable() {
    let (state, _db) = test_state().await;
    let web = state.config.web_dist.clone();
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(
        web.join("index.html"),
        "<!doctype html><div id=\"app\"></div>",
    )
    .unwrap();
    std::fs::write(web.join("assets/app-abc123.js"), "console.log(1)").unwrap();
    let app = build_router(state);

    let index = app
        .clone()
        .oneshot(Request::get("/devices").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(index.status(), StatusCode::OK);
    assert_eq!(
        index.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-cache",
        "SPA 入口不许被浏览器启发式缓存"
    );

    let asset = app
        .oneshot(
            Request::get("/assets/app-abc123.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        asset.headers().get(header::CACHE_CONTROL).unwrap(),
        "public, max-age=31536000, immutable"
    );
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
        data["kpis"]["todayTasks"]["value"].as_i64().unwrap_or(0) >= 1,
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
    sqlx::query(
        "UPDATE devices SET status = '在线待命', body_online = 1,
            sdk_node_available = 1, estop_software = 0 WHERE id = ?1",
    )
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
        // 现场没有物理急停按钮，`estop.software=true` 只能靠 1302 cmd=1 解锁
        ("clear_emergency_stop", "解除急停"),
        // 感知降级时规划器会误判「终点附近有障碍物」，需要能临时关/开绕障
        ("avoid_off", "避障关闭"),
        ("avoid_on", "避障开启"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/api/devices/{id}/action"))
                    .header(
                        header::CONTENT_TYPE,
                        header::HeaderValue::from_static("application/json"),
                    )
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
        // 急停 / 解除急停固件没有 1303 回执，响应必须带上回读校验窗口，
        // 前端据此提示「N 秒内自动校验」，而不是直报成功。
        if matches!(action, "emergency_stop" | "clear_emergency_stop") {
            assert!(
                body["data"]["verify"].is_object(),
                "{action} 应带 verify: {body}"
            );
            assert_eq!(body["data"]["verify"]["field"], "estopSoftware");
        }
    }
}

/// 设备事件流接口：客户端门禁拒绝（`event_type=error`）必须能在控制台查到。
///
/// 现场：点「站立」HTTP 回 200 + wsDelivered=true，本体其实没受理，
/// 只在事件流里留一条「指令 站立 被拒绝: 本体 SDK 节点未就绪」。没有这个接口，
/// 页面就是「点了没反应」。
#[tokio::test]
async fn device_events_expose_client_side_rejection() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let device_pk = bound_device_pk(&pool).await;
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('error', '指令 站立 被拒绝: 本体 SDK 节点未就绪（sdk_node=false）',
                 'wsDelivered=true', ?1, NULL, ?2)",
    )
    .bind(device_pk)
    // 不能用 `db::now()`：演示数据里预置了当天 08:35 / 09:00 的事件，
    // 早上 9 点前跑这个测试时「刚才插入的那条」反而不是最新的（时间排序依赖真实时钟）。
    .bind("9999-12-31 23:59:59")
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state.clone());
    let token = login(&app).await;
    let response = app
        .oneshot(
            Request::get(format!("/api/devices/{device_pk}/events"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let events = body["data"].as_array().cloned().unwrap_or_default();
    assert!(!events.is_empty(), "应返回设备事件: {body}");
    assert_eq!(events[0]["eventType"], "error");
    assert!(
        events[0]["title"]
            .as_str()
            .unwrap_or_default()
            .contains("被拒绝"),
        "最近一条应是被拒绝事件: {body}"
    );
}

/// `ctrl_source = 3` 的门禁文案必须区分「真遥控器」与「本体自己占着导航能力」。
///
/// 现场事故：遥控器明明关着，每次下发都提示「遥控器被接管」，运维只能去关一个
/// 本来就关着的遥控器 —— 真因是本体内残留任务把导航能力占住了（航点回 13897）。
#[tokio::test]
async fn dispatch_gate_explains_capability_busy_instead_of_blaming_remote() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let device_pk = bound_device_pk(&pool).await;
    let park_id: Option<i64> = sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
        .bind(device_pk)
        .fetch_one(&pool)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE devices SET status = '在线待命', body_online = 1, sdk_node_available = 1,
            estop_software = 0, ctrl_source = 3 WHERE id = ?1",
    )
    .bind(device_pk)
    .execute(&pool)
    .await
    .unwrap();

    let task_id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, device_id, park_id, task_status, progress,
            created_at, updated_at, created_by)
         VALUES ('TK-CTRL-3', '控制源门禁', ?1, ?2, '待执行', 0,
            '2026-11-12 00:00:00', '2026-11-12 00:00:00', 1)
         RETURNING id",
    )
    .bind(device_pk)
    .bind(park_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let app = build_router(state.clone());
    let token = login(&app).await;

    // 1) 没有 capability busy 证据：只能含糊地说「可能是遥控器 / 第三方 App」
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/dispatch"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default().to_string();
    assert!(
        msg.contains("ctrl_source=3") && msg.contains("物理遥控器"),
        "门禁应说清是控制源问题: {msg}"
    );
    assert!(
        !msg.contains("13897"),
        "没有证据时不能提 capability busy: {msg}"
    );

    // 2) 本体近期回过 13897：文案要指向本体内残留任务 / 算法节点
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('progress', '航点 新航点 2 执行失败',
                 '5001 响应失败: code=13897 resource conflict (capability busy)',
                 ?1, NULL, ?2)",
    )
    .bind(device_pk)
    .bind(robodog_ops_server::db::now())
    .execute(&pool)
    .await
    .unwrap();

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
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default();
    assert!(
        msg.contains("13897") && msg.contains("残留任务"),
        "有 capability busy 证据时应指向本体内残留任务: {msg}"
    );
}

/// 远程接管期间下发任务必须被拒。
///
/// 现场教训：接管时客户端会把本体切到 `1302 cmd=60` 通用（手动）模式，导航能力被判给
/// 手动控制，随后每个航点的 `5000` 都回 `13897 resource conflict (capability busy)`，
/// 平台上只能看到「航点执行失败」。在服务端就拦住，比让本体拒五次更早、更可读。
#[tokio::test]
async fn task_dispatch_is_rejected_while_teleop_held() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let device_pk = bound_device_pk(&pool).await;
    let park_id: Option<i64> = sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
        .bind(device_pk)
        .fetch_one(&pool)
        .await
        .unwrap();

    // 接管要求设备在线
    sqlx::query(
        "UPDATE devices SET status = '在线待命', body_online = 1,
            sdk_node_available = 1, estop_software = 0 WHERE id = ?1",
    )
    .bind(device_pk)
    .execute(&pool)
    .await
    .unwrap();

    let task_id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, device_id, park_id, task_status, progress,
            created_at, updated_at, created_by)
         VALUES ('TK-TELEOP-1', '接管中下发', ?1, ?2, '待执行', 0,
            '2026-11-12 00:00:00', '2026-11-12 00:00:00', 1)
         RETURNING id",
    )
    .bind(device_pk)
    .bind(park_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let app = build_router(state.clone());
    let token = login(&app).await;
    let taken = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{device_pk}/teleop/take"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(taken.status(), StatusCode::OK, "应先能接管");

    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/dispatch"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "接管期间不应下发任务"
    );
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default();
    assert!(msg.contains("接管"), "错误信息应说明接管中，实际: {msg}");

    // 释放接管后校验通过（此时才会因为设备未连接而报另一个错）
    sqlx::query("UPDATE devices SET teleop_holder = NULL, teleop_at = NULL WHERE id = ?1")
        .bind(device_pk)
        .execute(&pool)
        .await
        .unwrap();
    let response = app
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/dispatch"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default();
    assert!(
        !msg.contains("接管"),
        "释放接管后不应再因为接管被拒，实际: {msg}"
    );
}

/// 「重新定位」：机器狗不在地图上时的人工恢复入口。///
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
                .header(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/json"),
                )
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
                .header(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/json"),
                )
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
    sqlx::query(
        "UPDATE devices SET body_online = 1, sdk_node_available = 1,
            estop_software = 0 WHERE id = ?1",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .unwrap();
    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    state
        .device_connections
        .write()
        .await
        .insert(code.clone(), tx);
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{id}/locate"))
                .header(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/json"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    r#"{"mapId":"ROS-MAP-1@v1.0","x":1.5,"y":-2.0,"heading":90}"#,
                ))
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

/// 「任务详情 → 呼叫（喊话）」：平台 → 设备指令 `呼叫` → 客户端 → 板端 `/api/speak/call`。
///
/// 三条契约：
/// 1. 设备不在线 → 400「未送达」（不能假成功）；
/// 2. 设备在线 → WS 下发 `呼叫`，参数带 `text` / `loop` / `seconds`；
/// 3. 事件流里留一条 `call`（操作员回溯「谁在什么时候喊了什么」）。
#[tokio::test]
async fn task_call_dispatches_speak_to_the_device() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let app = build_router(state.clone());
    let token = login(&app).await;
    let device_pk = bound_device_pk(&pool).await;
    let device_code: String = sqlx::query_scalar("SELECT device_id FROM devices WHERE id = ?1")
        .bind(device_pk)
        .fetch_one(&pool)
        .await
        .expect("演示设备应有编号");
    let stamp = db::now();
    let task_id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, device_id,
            task_status, progress, created_at, updated_at)
         VALUES ('TK-CALL-1', '呼叫测试', '巡检任务', '立即执行', ?1, '执行中', 10, ?2, ?2)
         RETURNING id",
    )
    .bind(device_pk)
    .bind(&stamp)
    .fetch_one(&pool)
    .await
    .expect("插入测试任务失败");

    // 1) 设备客户端不在线：先报未送达，别让人以为喊了
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/call"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"text":"设备巡检，请勿靠近"}"#))
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

    // 2) 空内容：当场拒，不白下发一条
    let (tx, mut rx) = tokio::sync::broadcast::channel(8);
    state
        .device_connections
        .write()
        .await
        .insert(device_code.clone(), tx);
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/call"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"text":"   "}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "空内容要拒");

    // 3) 在线：循环喊 30s 的呼叫要原样落到设备指令里
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/call"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    r#"{"text":"设备巡检请避让","loop":true,"seconds":30}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let frame = rx.recv().await.expect("应收到下发的指令帧");
    assert_eq!(frame.cmd_type, "remote_command");
    assert_eq!(frame.payload["command"], "呼叫");
    assert_eq!(frame.payload["params"]["params"]["text"], "设备巡检请避让");
    assert_eq!(frame.payload["params"]["params"]["loop"], true);
    assert_eq!(frame.payload["params"]["params"]["seconds"], 30);

    // 4) 停止呼叫：同一条设备指令通道
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/call/stop"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let frame = rx.recv().await.expect("应收到停止指令帧");
    assert_eq!(frame.payload["command"], "停止呼叫");

    // 5) 任务事件流里能看到这条呼叫
    let title: Option<String> = sqlx::query_scalar(
        "SELECT title FROM task_events WHERE task_id = ?1 AND event_type = 'call' LIMIT 1",
    )
    .bind(task_id)
    .fetch_optional(&pool)
    .await
    .expect("查询任务事件失败");
    assert!(
        title.as_deref().unwrap_or_default().contains("呼叫"),
        "事件流应留一条呼叫痕迹，实际：{title:?}"
    );
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

        let payload =
            format!(r#"{{"taskName":"冒烟巡检","routeId":{route_id},"requestId":"smoke-task-1"}}"#);
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
            command_results: state.command_results,
            video_hub: state.video_hub,
            notifier: robodog_ops_server::notify::Notifier::disabled(),
            agent_tunnel: Default::default(),
            phone_audio: Default::default(),
            ai_patrol: Default::default(),
            caches: Default::default(),
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
    use robodog_ops_server::ingest::{self, IngestMode, IngestRequest, ParkRef};
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
            https_port: 0,
            tls_cert: None,
            tls_key: None,
            web_dist: std::env::temp_dir().join("web-dist"),
            backend_db_path: std::env::temp_dir().join("backend.db"),
            zentai_base_url: Some(base),
            zentai_username: None,
            zentai_password: None,
            ingest_dir: std::env::temp_dir().join("map_ingest"),
            recording_dir: std::env::temp_dir().join("recordings"),
            photo_dir: std::env::temp_dir().join("photos"),
            recording_keep_days: 30,
            photo_keep_days: 90,
            service_token: Some(TOKEN.to_string()),
            zentai_ws_url: None,
            device_timeout_secs: 90,
            task_timeout_secs: 120,
            task_stuck_secs: 600,
            backpack_base_url: None,
            backpack_webrtc_url: None,
            backpack_browser_url: None,
            mediamtx_url: None,
            backpack_tunnel_device: None,
            backpack_devices: Vec::new(),
            point_ai: "auto".to_string(),
            ai_window_max_secs: 300,
            public_url: None,
            wecom_enabled: false,
            wecom_webhook: None,
            wecom_min_level: "警告".to_string(),
            wecom_mention_all: false,
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
                "/api/dogInternal/media/upload",
                post(|headers: HeaderMap, mut multipart: Multipart| async move {
                    assert_eq!(
                        headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                        Some(TOKEN)
                    );
                    let mut saw_file = false;
                    let mut saw_zip = false;
                    let mut info = Value::Null;
                    while let Some(field) = multipart.next_field().await.unwrap() {
                        match field.name() {
                            Some("file") => {
                                let bytes = field.bytes().await.unwrap();
                                if bytes.starts_with(b"PK\x03\x04") {
                                    saw_zip = true;
                                } else {
                                    assert!(
                                        bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
                                        "图片原字节应被原样转发"
                                    );
                                }
                                saw_file = true;
                            }
                            Some("mediaInfo") => {
                                let text = field.text().await.unwrap();
                                info = serde_json::from_str(&text).unwrap();
                            }
                            _ => {}
                        }
                    }
                    assert!(saw_file, "必须收到媒体文件");
                    assert_eq!(info["deviceCode"], "RD-001");
                    assert_eq!(info["deviceId"], "RD-001", "两份设备编号都要补齐");
                    // 批量（zip）：业务平台不解包，只把条目元数据原样转发给中台
                    if info.get("entries").is_some() {
                        assert!(saw_zip, "带 entries 的必须是压缩包");
                        assert_eq!(info["entries"].as_array().map(Vec::len), Some(2));
                        assert_eq!(info["entries"][0]["name"], "0001.jpg");
                        assert_eq!(info["entries"][0]["planId"], "TK-9");
                        return axum::Json(serde_json::json!({
                            "code": 10200,
                            "msg": "success",
                            "data": {
                                "created": true,
                                "total": 2,
                                "failed": 0,
                                "results": [
                                    {"name": "0001.jpg", "ok": true, "created": true, "mediaId": 7},
                                    {"name": "0002.jpg", "ok": true, "created": false, "mediaId": 8},
                                ],
                            },
                        }));
                    }
                    assert_eq!(info["planId"], "TK-9");
                    assert_eq!(info["pointIndex"], 2);
                    axum::Json(serde_json::json!({
                        "code": 10200,
                        "msg": "success",
                        "data": {"created": true, "media": {"id": 7, "mediaName": "RD-001 TK-9 航点2"}},
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
            mode: IngestMode::Incremental,
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
            None,
        )
        .await
        .expect("地图应可回传");
        assert_eq!(map.id, 42);
        assert_eq!(map.map_name, "A 区机器人建图");

        // 全程没有登录换 JWT
        assert_eq!(token_calls.load(Ordering::SeqCst), 0, "不应调用 /api/token");
    }

    /// 设备客户端回传航点 AI 存证图：业务平台只做校验 + 转发中台内部接口，
    /// 中台凭证/账号留在服务端（这里断言全程没走 `/api/token`）。
    #[tokio::test]
    async fn media_ingest_forwards_to_zentai_internal_upload() {
        let (base, token_calls) = spawn_fake_zentai().await;
        let config = service_config(base);
        let (state, _db) = test_state_with_token(Some(TOKEN)).await;
        let state = AppState {
            config: Arc::new(config),
            ..state
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind api");
        let addr = listener.local_addr().expect("local addr");
        let router = build_router(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let api = format!("http://{addr}");

        let login: Value = reqwest::Client::new()
            .post(format!("{api}/api/auth/login"))
            .json(&serde_json::json!({"username": "admin", "password": "admin123"}))
            .send()
            .await
            .expect("login")
            .json()
            .await
            .expect("login json");
        let token = login["data"]["token"].as_str().expect("token").to_string();

        let info = serde_json::json!({
            "deviceCode": "RD-001",
            "planId": "TK-9",
            "pointIndex": 2,
            "capturedAt": "2026-11-20 10:00:00",
            "source": "ai-patrol",
            "remarks": "dogcam_01.jpg",
        });
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(vec![0xFF, 0xD8, 0xFF, 0x01, 0x02])
                    .file_name("dogcam_01.jpg")
                    .mime_str("image/jpeg")
                    .expect("mime"),
            )
            .text("mediaInfo", info.to_string());
        let response = reqwest::Client::new()
            .post(format!("{api}/api/media/ingest"))
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await
            .expect("media ingest");
        assert_eq!(response.status(), 200, "回传应成功");
        let body: Value = response.json().await.expect("json");
        assert_eq!(body["data"]["created"], serde_json::json!(true));
        assert_eq!(body["data"]["media"]["id"], serde_json::json!(7));
        assert_eq!(
            token_calls.load(Ordering::SeqCst),
            0,
            "媒体回传也不该走账号登录"
        );
    }

    /// 扩展名/文件头不是图片直接拒绝：不要把任意文件往中台搬。
    #[tokio::test]
    async fn media_ingest_rejects_non_image() {
        let (base, _token_calls) = spawn_fake_zentai().await;
        let config = service_config(base);
        let (state, _db) = test_state_with_token(Some(TOKEN)).await;
        let state = AppState {
            config: Arc::new(config),
            ..state
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind api");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let api = format!("http://{addr}");
        let login: Value = reqwest::Client::new()
            .post(format!("{api}/api/auth/login"))
            .json(&serde_json::json!({"username": "admin", "password": "admin123"}))
            .send()
            .await
            .expect("login")
            .json()
            .await
            .expect("login json");
        let token = login["data"]["token"].as_str().expect("token").to_string();

        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(b"plain text".to_vec())
                    .file_name("notes.txt")
                    .mime_str("text/plain")
                    .expect("mime"),
            )
            .text(
                "mediaInfo",
                serde_json::json!({"deviceId": "RD-001"}).to_string(),
            );
        let response = reqwest::Client::new()
            .post(format!("{api}/api/media/ingest"))
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await
            .expect("media ingest");
        assert_eq!(response.status(), 400, "非图片必须拒绝");
    }

    /// 客户端现行方式：一个 zip 传 N 张。业务平台只验大小/形态 + 补设备信息，
    /// 解包与逐张入库都在中台，响应原样带回 `results[]`（客户端按它删板端文件）。
    #[tokio::test]
    async fn media_zip_ingest_forwards_batch_to_zentai() {
        let (base, token_calls) = spawn_fake_zentai().await;
        let config = service_config(base);
        let (state, _db) = test_state_with_token(Some(TOKEN)).await;
        let state = AppState {
            config: Arc::new(config),
            ..state
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind api");
        let addr = listener.local_addr().expect("local addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, build_router(state)).await;
        });
        let api = format!("http://{addr}");
        let login: Value = reqwest::Client::new()
            .post(format!("{api}/api/auth/login"))
            .json(&serde_json::json!({"username": "admin", "password": "admin123"}))
            .send()
            .await
            .expect("login")
            .json()
            .await
            .expect("login json");
        let token = login["data"]["token"].as_str().expect("token").to_string();

        let info = serde_json::json!({
            "deviceCode": "RD-001",
            "source": "ai-patrol",
            "entries": [
                {"name": "0001.jpg", "planId": "TK-9", "pointIndex": 0, "capturedAt": "2026-11-20 10:00:00"},
                {"name": "0002.jpg", "planId": "TK-9", "pointIndex": 0, "capturedAt": "2026-11-20 10:00:01"},
            ],
        });
        let zip = media_zip(&[
            ("0001.jpg", vec![0xFF, 0xD8, 0xFF, 0x01]),
            ("0002.jpg", vec![0xFF, 0xD8, 0xFF, 0x02]),
        ]);
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(zip)
                    .file_name("media-RD-001.zip")
                    .mime_str("application/zip")
                    .expect("mime"),
            )
            .text("mediaInfo", info.to_string());
        let response = reqwest::Client::new()
            .post(format!("{api}/api/media/ingest"))
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await
            .expect("zip media ingest");
        assert_eq!(response.status(), 200, "压缩包回传应成功");
        let body: Value = response.json().await.expect("json");
        let results = body["data"]["results"].as_array().expect("results 数组");
        assert_eq!(results.len(), 2);
        assert_eq!(body["data"]["failed"], serde_json::json!(0));
        assert_eq!(results[0]["created"], serde_json::json!(true));
        assert_eq!(results[1]["created"], serde_json::json!(false));
        assert_eq!(
            token_calls.load(Ordering::SeqCst),
            0,
            "压缩包回传同样不该走账号登录"
        );

        // 压缩包但没带 entries：直接拒绝（不猜每张图属于哪个计划/航点）
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(media_zip(&[(
                    "0001.jpg",
                    vec![0xFF, 0xD8, 0xFF, 0x01],
                )]))
                .file_name("media-RD-001.zip")
                .mime_str("application/zip")
                .expect("mime"),
            )
            .text(
                "mediaInfo",
                serde_json::json!({"deviceCode": "RD-001"}).to_string(),
            );
        let response = reqwest::Client::new()
            .post(format!("{api}/api/media/ingest"))
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await
            .expect("zip media ingest");
        assert_eq!(response.status(), 400, "缺 entries 必须拒绝");
    }

    fn media_zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(*name, options).expect("写入条目");
            writer.write_all(bytes).expect("写入内容");
        }
        writer.finish().expect("生成 zip").into_inner()
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
    assert!(
        (y - 21.29).abs() < 0.05,
        "y 应换算为地图米（含 y 翻转），实际 {y}"
    );
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
///
/// 走 `TaskRow`（持久化投影）再转 `Task`（对外形状），和服务端同一条路径。
async fn fetch_task_row(pool: &sqlx::SqlitePool, id: i64) -> robodog_ops_server::models::Task {
    let row: robodog_ops_server::models::TaskRow = sqlx::query_as(&format!(
        "{} WHERE t.id = ?1",
        robodog_ops_server::handlers::tasks::TASK_SELECT
    ))
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    row.into()
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
    assert_eq!(
        map.map_path, "2026_11_10_16_15_06",
        "map_path 也要用真实目录名"
    );
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
    assert_eq!(
        map.map_dir,
        robodog_ops_server::mapdir::synthetic_dir(&map_id)
    );
}

/// 计划发布状态（`publishStatus`）：草稿只保存，已发布才进自动调度。
///
/// 背景：`task_status` 是执行状态，没有「这条计划定稿了没有」的概念。
/// 草稿必须能被自动调度器、设备上线补发两条自动路径一起挡住（`schedule::DueState::Draft`），
/// 否则「先存一份备稿」就等于「立刻让它跑起来」。
#[tokio::test]
async fn task_publish_status_gates_auto_scheduling() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let device_pk = bound_device_pk(&pool).await;
    let park_id: Option<i64> = sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
        .bind(device_pk)
        .fetch_one(&pool)
        .await
        .unwrap();
    let park_id = park_id.expect("演示设备应已绑定园区");

    let app = build_router(state.clone());
    let token = login(&app).await;

    // 1) 建成草稿：窗口覆盖当前时刻也不下发（三条自动路径都认 publish_status）
    let now = chrono::Local::now();
    let start = (now - chrono::Duration::minutes(5))
        .format("%H:%M")
        .to_string();
    let end = (now + chrono::Duration::minutes(30))
        .format("%H:%M")
        .to_string();
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(format!(
                    r#"{{"taskName":"备稿计划","executeMode":"周期任务","deviceId":{device_pk},
                        "parkId":{park_id},"planStart":"{start}","planEnd":"{end}",
                        "publishStatus":"草稿"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(
        body["data"]["publishStatus"], "草稿",
        "建计划时要认 publishStatus"
    );
    assert!(body["data"]["publishedAt"].is_null(), "草稿没有发布时间");
    let task_id = body["data"]["id"].as_i64().expect("应返回任务 id");
    let (status, dispatched_on): (String, Option<String>) =
        sqlx::query_as("SELECT task_status, last_dispatched_on FROM tasks WHERE id = ?1")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "待执行", "草稿不能直接被下发成执行中");
    assert!(dispatched_on.is_none(), "草稿不应计入当天已下发");

    // 2) 发布：翻成已发布 + 补发布时间，事件流里写清后续怎么跑
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/publish"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"]["publishStatus"], "已发布");
    assert!(
        body["data"]["publishedAt"].as_str().is_some(),
        "发布要记下发布时间"
    );
    let published_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_events WHERE task_id = ?1 AND event_type = 'published'",
    )
    .bind(task_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(published_events, 1, "发布要留痕（运维要看它什么时候生效）");

    // 3) 重复发布 / 重复撤回：状态没变就 400，不要静默成功
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/publish"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 4) 撤回：回草稿，不再自动下发（已发布的那一次不受影响）
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/tasks/{task_id}/unpublish"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"]["publishStatus"], "草稿");
    assert!(
        body["data"]["publishedAt"].as_str().is_some(),
        "撤回不清空发布时间（用来回溯这一版从什么时候开始跑）"
    );

    // 5) 乱填发布状态：建计划阶段就 400，不要静默当成已发布
    let response = app
        .oneshot(
            Request::post("/api/tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(format!(
                    r#"{{"taskName":"乱填状态","deviceId":{device_pk},"parkId":{park_id},
                        "publishStatus":"发布"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("发布状态"),
        "错误文案要指出发布状态，实际: {message}"
    );
}

/// 任务级结束动作（返航开关）：显式 `endTask` 必须覆盖推断，并用独立的
/// `endTaskMode` 表达（旧客端只认 `endTask`，不能把 `none` 的语义挂到它上面）。
#[tokio::test]
async fn task_end_action_drives_dispatch_payload() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_dispatchable_route(&pool, &map_id).await;
    let device_pk = bound_device_pk(&pool).await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    async fn create(app: &axum::Router, token: &str, body: String) -> Value {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/tasks")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "建任务应成功");
        json_body(response).await
    }

    // 1) 显式「原地待命」：落库 + 载荷带 endTaskMode
    let body = create(
        &app,
        &token,
        format!(
            r#"{{"taskName":"原地待命任务","deviceId":{device_pk},"parkId":1,
                "routeId":{route_id},"endTask":"none"}}"#
        ),
    )
    .await;
    assert_eq!(body["data"]["endTask"], "none");
    let task_id = body["data"]["id"].as_i64().expect("应返回任务 id");
    let task = fetch_task_row(&pool, task_id).await;
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .expect("应能组装载荷");
    assert_eq!(payload.end_task.as_deref(), Some("none"));
    assert_eq!(
        payload.end_task_mode.as_deref(),
        Some("none"),
        "显式原地必须带 endTaskMode，否则旧兜底会继续返航"
    );

    // 2) 不传：跟随路线（老行为），不带 endTaskMode
    let body = create(
        &app,
        &token,
        format!(
            r#"{{"taskName":"跟随路线任务","deviceId":{device_pk},"parkId":1,
                "routeId":{route_id}}}"#
        ),
    )
    .await;
    assert!(body["data"]["endTask"].is_null());
    let task_id = body["data"]["id"].as_i64().expect("应返回任务 id");
    let task = fetch_task_row(&pool, task_id).await;
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .unwrap();
    assert!(payload.end_task_mode.is_none(), "没配置就不应该下发");

    // 3) 乱填：建任务阶段就 400，不要静默当成跟随路线
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(format!(
                    r#"{{"taskName":"错误结束动作","deviceId":{device_pk},"parkId":1,
                        "routeId":{route_id},"endTask":"回家"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert!(
        body["msg"]
            .as_str()
            .unwrap_or_default()
            .contains("结束动作"),
        "错误信息应说明结束动作可选值: {body}"
    );
}

/// 计划级「仅夜间自动开灯」：建计划时能存下来，并原样进下发载荷
/// （客户端只把它转给板端 Agent，白天/夜间的判定在板端）。
#[tokio::test]
async fn task_light_night_only_reaches_dispatch_payload() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_dispatchable_route(&pool, &map_id).await;
    let device_pk = bound_device_pk(&pool).await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    async fn create(app: &axum::Router, token: &str, body: String) -> Value {
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/tasks")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "建任务应成功");
        json_body(response).await
    }

    // 1) 勾上：落库 + 载荷为 true
    let body = create(
        &app,
        &token,
        format!(
            r#"{{"taskName":"夜间补光巡检","deviceId":{device_pk},"parkId":1,
                "routeId":{route_id},"lightNightOnly":true}}"#
        ),
    )
    .await;
    assert_eq!(
        body["data"]["lightNightOnly"], true,
        "建任务的回执要带回来，否则前端以为没存上: {body}"
    );
    let task_id = body["data"]["id"].as_i64().expect("应返回任务 id");
    let task = fetch_task_row(&pool, task_id).await;
    assert!(task.light_night_only);
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .expect("应能组装载荷");
    assert!(
        payload.light_night_only,
        "勾了仅夜间必须下发，否则板端会白天也开灯"
    );

    // 2) 不传：默认不限制时段（老计划 / 老调用方行为不变）
    let body = create(
        &app,
        &token,
        format!(
            r#"{{"taskName":"普通巡检","deviceId":{device_pk},"parkId":1,
                "routeId":{route_id}}}"#
        ),
    )
    .await;
    assert_eq!(body["data"]["lightNightOnly"], false);
    let task_id = body["data"]["id"].as_i64().expect("应返回任务 id");
    let task = fetch_task_row(&pool, task_id).await;
    assert!(!task.light_night_only);
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .unwrap();
    assert!(!payload.light_night_only);

    // 3) 编辑时显式改成 false（编辑弹窗取消勾选）：不能因为「没传」被忽略
    let response = app
        .clone()
        .oneshot(
            Request::put(format!("/api/tasks/{task_id}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    r#"{"taskName":"普通巡检","lightNightOnly":true}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "编辑应成功");
    let task = fetch_task_row(&pool, task_id).await;
    assert!(task.light_night_only, "编辑弹窗勾上要生效");
    let response = app
        .clone()
        .oneshot(
            Request::put(format!("/api/tasks/{task_id}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    r#"{"taskName":"普通巡检","lightNightOnly":false}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let task = fetch_task_row(&pool, task_id).await;
    assert!(!task.light_night_only, "取消勾选也要生效");
}

/// 计划级「自动开灯」：勾了就**以计划为准**，航点动作里的到点开灯 / 离开关灯不再生效。
///
/// 服务端只负责落库 + 下发 `lightFill` / `lightLed`（怎么开、什么时候关由客户端的
/// `ActiveTask::light_at` 一处收口）；这里断言的是「能存住、能改、总是下发」，因为
/// 老客户端忽略这两个字段时会退回按航点动作走（航点动作仍留在 `routeData` 里）。
#[tokio::test]
async fn plan_lights_reach_dispatch_payload_and_default_to_waypoint_actions() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_dispatchable_route(&pool, &map_id).await;
    let device_pk = bound_device_pk(&pool).await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    async fn send(
        app: &axum::Router,
        token: &str,
        method: &str,
        uri: String,
        body: String,
    ) -> Value {
        let request = match method {
            "POST" => Request::post(uri),
            _ => Request::put(uri),
        };
        let response = app
            .clone()
            .oneshot(
                request
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "请求应成功");
        json_body(response).await
    }

    // 1) 只勾补光灯：另一盏灯必须是 false（别替现场「顺手全开」）
    let body = send(
        &app,
        &token,
        "POST",
        "/api/tasks".to_string(),
        format!(
            r#"{{"taskName":"计划自动开灯","deviceId":{device_pk},"parkId":1,
                "routeId":{route_id},"lightFill":true}}"#
        ),
    )
    .await;
    assert_eq!(body["data"]["lightFill"], true, "回执要带回来: {body}");
    assert_eq!(body["data"]["lightLed"], false);
    let task_id = body["data"]["id"].as_i64().expect("应返回任务 id");
    let task = fetch_task_row(&pool, task_id).await;
    assert!(task.light_fill && !task.light_led);
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .expect("应能组装载荷");
    assert!(
        payload.light_fill && !payload.light_led,
        "计划灯必须原样下发：客户端据此忽略航点动作"
    );

    // 2) 不传：两盏都 false → 保持「按航点动作」的老行为
    let body = send(
        &app,
        &token,
        "POST",
        "/api/tasks".to_string(),
        format!(
            r#"{{"taskName":"航点动作开灯","deviceId":{device_pk},"parkId":1,
                "routeId":{route_id}}}"#
        ),
    )
    .await;
    assert_eq!(body["data"]["lightFill"], false);
    assert_eq!(body["data"]["lightLed"], false);
    let waypoint_task_id = body["data"]["id"].as_i64().expect("应返回任务 id");
    let task = fetch_task_row(&pool, waypoint_task_id).await;
    assert!(!task.light_fill && !task.light_led, "默认不能给计划灯");

    // 3) 编辑：显式关掉补光灯、打开 LED，两盏要各自生效
    send(
        &app,
        &token,
        "PUT",
        format!("/api/tasks/{task_id}"),
        r#"{"taskName":"计划自动开灯","lightFill":false,"lightLed":true}"#.to_string(),
    )
    .await;
    let task = fetch_task_row(&pool, task_id).await;
    assert!(!task.light_fill && task.light_led, "编辑后要以请求为准");

    // 4) 只传一个字段：另一个保持原值（前端只发改动的那个也不至于被清掉）
    send(
        &app,
        &token,
        "PUT",
        format!("/api/tasks/{task_id}"),
        r#"{"taskName":"计划自动开灯","lightFill":true}"#.to_string(),
    )
    .await;
    let task = fetch_task_row(&pool, task_id).await;
    assert!(task.light_fill && task.light_led, "没传的那盏保持原值");
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .unwrap();
    assert!(payload.light_fill && payload.light_led);
}

/// 计划里自定义「仅夜间」的时段：能存、能改、能清回板端默认，并原样进下发载荷。
///
/// 判夜仍在板端（客户端只把 `nightStart`/`nightEnd` 转下去），所以这里只验
/// 「存得住 + 会下发 + 非法值 400」。
#[tokio::test]
async fn plan_night_window_is_configurable_and_reaches_dispatch_payload() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let map_id = insert_real_map(&pool).await;
    let route_id = insert_dispatchable_route(&pool, &map_id).await;
    let device_pk = bound_device_pk(&pool).await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    async fn send(
        app: &axum::Router,
        token: &str,
        method: &str,
        uri: String,
        body: String,
    ) -> (StatusCode, Value) {
        let request = match method {
            "POST" => Request::post(uri),
            _ => Request::put(uri),
        };
        let response = app
            .clone()
            .oneshot(
                request
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        (status, json_body(response).await)
    }

    // 1) 建计划：自定义 19:00-06:00（跨零点）
    let (status, body) = send(
        &app,
        &token,
        "POST",
        "/api/tasks".to_string(),
        format!(
            r#"{{"taskName":"夜巡 19-6","deviceId":{device_pk},"parkId":1,
                "routeId":{route_id},"lightNightOnly":true,"lightFill":true,
                "lightNightStart":"19:00","lightNightEnd":"06:00"}}"#
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["lightNightStart"], "19:00", "{body}");
    assert_eq!(body["data"]["lightNightEnd"], "06:00", "{body}");
    let task_id = body["data"]["id"].as_i64().expect("应返回任务 id");
    let task = fetch_task_row(&pool, task_id).await;
    assert_eq!(task.light_night_start.as_deref(), Some("19:00"));
    assert_eq!(task.light_night_end.as_deref(), Some("06:00"));
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .expect("应能组装载荷");
    assert_eq!(payload.light_night_start.as_deref(), Some("19:00"));
    assert_eq!(payload.light_night_end.as_deref(), Some("06:00"));

    // 2) 只给一半 / 起止相同 / 格式不对 → 400（静默退默认最难查）
    for bad in [
        r#""lightNightStart":"19:00""#,
        r#""lightNightEnd":"06:00""#,
        r#""lightNightStart":"18:00","lightNightEnd":"18:00""#,
        r#""lightNightStart":"晚上七点","lightNightEnd":"06:00""#,
    ] {
        let (status, body) = send(
            &app,
            &token,
            "POST",
            "/api/tasks".to_string(),
            format!(
                r#"{{"taskName":"坏时段","deviceId":{device_pk},"parkId":1,
                    "routeId":{route_id},{bad}}}"#
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} → {body}");
        assert!(
            body["msg"]
                .as_str()
                .unwrap_or_default()
                .contains("夜间时段"),
            "{bad} 的错误信息要说明夜间时段: {body}"
        );
    }

    // 3) 编辑只传一个字段：其余保持原值（前端只想改结束时间也能用）
    let (status, _) = send(
        &app,
        &token,
        "PUT",
        format!("/api/tasks/{task_id}"),
        r#"{"taskName":"夜巡 19-6","lightNightStart":"20:00","lightNightEnd":"05:00"}"#.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let task = fetch_task_row(&pool, task_id).await;
    assert_eq!(task.light_night_start.as_deref(), Some("20:00"));

    // 4) 编辑不传这两个字段：保持原值
    let (status, _) = send(
        &app,
        &token,
        "PUT",
        format!("/api/tasks/{task_id}"),
        r#"{"taskName":"夜巡 20-5"}"#.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let task = fetch_task_row(&pool, task_id).await;
    assert_eq!(task.light_night_start.as_deref(), Some("20:00"));
    assert_eq!(task.light_night_end.as_deref(), Some("05:00"));

    // 5) 传空串 = 清回板端默认（载荷里就不带这两个字段了）
    let (status, _) = send(
        &app,
        &token,
        "PUT",
        format!("/api/tasks/{task_id}"),
        r#"{"taskName":"夜巡 默认时段","lightNightStart":"","lightNightEnd":""}"#.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let task = fetch_task_row(&pool, task_id).await;
    assert!(task.light_night_start.is_none() && task.light_night_end.is_none());
    let payload = robodog_ops_server::dispatch::build_dispatch_payload(&pool, &task)
        .await
        .unwrap();
    assert!(payload.light_night_start.is_none() && payload.light_night_end.is_none());
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

/// 遥控器断电后本体仍报 `ctrl_source = 3`：普通接管被拒，强制接管能建立并放行摇杆。
///
/// 现场流程：遥控器先用过 → 直接关电源 → 本体状态还有延迟（甚至一直报 3）。
/// 如果只按 `ctrl_source` 硬拒，运维只能干等；带 `force` 的接管可以让运维二次确认后
/// 立刻接手，并且这个标记会落库（`devices.teleop_force`），释放接管时清零。
#[tokio::test]
async fn force_takeover_bypasses_remote_preempt() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let id: i64 = sqlx::query_scalar("SELECT id FROM devices LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("演示数据应包含设备");
    sqlx::query(
        "UPDATE devices SET status = '在线待命', body_online = 1, sdk_node_available = 1,
            estop_software = 0, ctrl_source = 3, teleop_holder = NULL, teleop_at = NULL,
            teleop_force = 0 WHERE id = ?1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state.clone());
    let token = login(&app).await;

    // 1) 常规接管：遥控器还占着控制权，必须被拒
    let denied = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{id}/teleop/take"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        denied.status(),
        StatusCode::BAD_REQUEST,
        "遥控器占权时应拒绝常规接管"
    );

    // 2) 强制接管（运维二次确认遥控器已断电）
    let forced = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{id}/teleop/take"))
                .header(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/json"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"force":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(forced.status(), StatusCode::OK, "强制接管应成功");
    let body = json_body(forced).await;
    assert_eq!(body["data"]["forced"], true);
    let flag: i64 = sqlx::query_scalar("SELECT teleop_force FROM devices WHERE id = ?1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(flag, 1, "强制接管标记应落库");

    // 3) 强制接管期间摇杆不再被 ctrl_source=3 拦掉
    let axes = app
        .clone()
        .oneshot(
            Request::post(format!("/api/devices/{id}/teleop"))
                .header(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/json"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    r#"{"forward":0.5,"strafe":0.0,"turn":0.0,"speed":0.5}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(axes.status(), StatusCode::OK, "强制接管后摇杆应被接受");

    // 4) 释放接管必须把强制标记清掉，否则这台设备以后永久免疫遥控器抢占检测
    let released = app
        .oneshot(
            Request::post(format!("/api/devices/{id}/teleop/release"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(released.status(), StatusCode::OK);
    let flag: i64 = sqlx::query_scalar("SELECT teleop_force FROM devices WHERE id = ?1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(flag, 0, "释放接管后强制标记应清零");
}

/// 「本体重连」是运维指令：客户端不在线时必须直接 400，不能静默成功。
///
/// 它让设备客户端进程退出并由 systemd 重建本体链路，属于"链路半死"时的兜底手段；
/// 真正下发时客户端还会再自检任务/建图/接管态。
#[tokio::test]
async fn body_reconnect_requires_online_client() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let id: i64 = sqlx::query_scalar("SELECT id FROM devices LIMIT 1")
        .fetch_one(&pool)
        .await
        .expect("演示数据应包含设备");
    sqlx::query(
        "UPDATE devices SET status = '在线待命', body_online = 1, run_mode = 'robot',
            teleop_holder = NULL, teleop_at = NULL WHERE id = ?1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    let app = build_router(state.clone());
    let token = login(&app).await;
    let response = app
        .oneshot(
            Request::post(format!("/api/devices/{id}/command"))
                .header(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("application/json"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"command":"本体重连"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "客户端不在线时应拒绝重连"
    );
    let body = json_body(response).await;
    let msg = body["msg"].as_str().unwrap_or_default();
    // 白名单要认这条指令（否则会是「不支持的指令」），拒绝原因是"未连接"
    assert!(!msg.contains("不支持的指令"), "本体重连应在白名单内: {msg}");
    assert!(msg.contains("未连接"), "拒绝原因应说明客户端未连接: {msg}");
}

// ─── 建图回传：增量 vs 刷新 ─────────────────────────────────────────

/// 造一个最小的机器人地图包（`model.pgm` + `model.yaml`）。
fn robot_map_zip(pgm: &[u8], mtime: (u16, u8, u8, u8, u8, u8)) -> Vec<u8> {
    use std::io::Write;

    let (year, month, day, hour, minute, second) = mtime;
    let mut buffer = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(&mut buffer);
    let options = zip::write::SimpleFileOptions::default().last_modified_time(
        zip::DateTime::from_date_and_time(year, month, day, hour, minute, second).unwrap(),
    );
    writer.start_file("model.yaml", options).unwrap();
    writer
        .write_all(b"image: model.pgm\nresolution: 0.05\norigin: [-1.0, -2.0, 0.0]\nnegate: 0\noccupied_thresh: 0.65\nfree_thresh: 0.196\n")
        .unwrap();
    writer.start_file("model.pgm", options).unwrap();
    writer.write_all(pgm).unwrap();
    writer.finish().unwrap();
    buffer.into_inner()
}

/// 构造 multipart 请求体（`file` + `mapInfo`）。
fn multipart_map_body(boundary: &str, package: &[u8], info: &Value) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"mapInfo\"\r\n\r\n{}\r\n",
            info
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"map.zip\"\r\nContent-Type: application/zip\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(package);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

async fn post_ingest(app: &axum::Router, token: &str, package: &[u8], info: Value) -> Value {
    const BOUNDARY: &str = "robodog-test-boundary";
    let body = multipart_map_body(BOUNDARY, package, &info);
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/map-versions/ingest")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = json_body(response).await;
    assert!(status.is_success(), "回传接口应成功（{status}）：{body}");
    body
}

/// `mode=incremental` 命中 `sourceRef` 就跳过，`mode=refresh` 内容有变化才覆盖。
#[tokio::test]
async fn ingest_refresh_overwrites_existing_map_only_when_content_changes() {
    use axum::extract::Multipart;
    use axum::http::HeaderMap;
    use axum::routing::{get, post};
    use robodog_ops_server::ingest::sha256_hex;
    use std::sync::Mutex as StdMutex;

    const TOKEN: &str = "test-service-token";
    const SOURCE_REF: &str = "/ota/alg_data/map/history_map/map_20261110_2312";
    const ZENTAI_ID: i64 = 42;

    // 中台假实现：记录收到的 mapInfo，回 fixed id
    let uploads: Arc<StdMutex<Vec<Value>>> = Arc::new(StdMutex::new(Vec::new()));
    let recorded = uploads.clone();
    let zentai = axum::Router::new()
        .route(
            "/api/dogInternal/maps/exists",
            post(|headers: HeaderMap, _body: axum::Json<Value>| async move {
                assert_eq!(
                    headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                    Some(TOKEN)
                );
                axum::Json(serde_json::json!({
                    "code": 10200,
                    "msg": "success",
                    "data": {"existing": [ZENTAI_ID]},
                }))
            }),
        )
        .route(
            "/api/dogInternal/devices/assignments",
            get(|_headers: HeaderMap| async move {
                axum::Json(serde_json::json!({
                    "code": 10200,
                    "msg": "success",
                    "data": [],
                }))
            }),
        )
        .route(
            "/api/dogInternal/maps/upload",
            post(move |headers: HeaderMap, mut multipart: Multipart| {
                let recorded = recorded.clone();
                async move {
                    assert_eq!(
                        headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                        Some(TOKEN)
                    );
                    let mut map_name = String::new();
                    while let Some(field) = multipart.next_field().await.unwrap() {
                        match field.name() {
                            Some("file") => {
                                let _ = field.bytes().await.unwrap();
                            }
                            Some("mapInfo") => {
                                let text = field.text().await.unwrap();
                                let info: Value = serde_json::from_str(&text).unwrap();
                                map_name = info["mapName"].as_str().unwrap_or_default().to_string();
                                recorded.lock().unwrap().push(info);
                            }
                            _ => {}
                        }
                    }
                    axum::Json(serde_json::json!({
                        "code": 10200,
                        "msg": "success",
                        "data": {"id": ZENTAI_ID, "mapName": map_name, "fileName": "map.zip"},
                    }))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake zentai");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, zentai).await;
    });

    let (state, db_dir) = test_state_with_token(Some(TOKEN)).await;
    let mut config = (*state.config).clone();
    config.zentai_base_url = Some(format!("http://{addr}"));
    let state = AppState {
        db: state.db,
        notifier: robodog_ops_server::notify::Notifier::disabled(),
        config: Arc::new(config),
        device_connections: state.device_connections,
        command_results: state.command_results,
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
        phone_audio: Default::default(),
        ai_patrol: Default::default(),
        caches: Default::default(),
    };
    let app = build_router(state.clone());
    let token = login(&app).await;

    // 业务库里已经有一行：设备回传过的同一张图 + 本地包副本
    let original = robot_map_zip(b"P5\n4 4\n255\n\x01\x02\x03\x04", (2026, 11, 10, 23, 12, 0));
    let stored = db_dir.dir.join("stored-map.zip");
    std::fs::write(&stored, &original).expect("写入本地副本失败");
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, device_code, map_name, version, status,
            file_name, file_path, file_size, source, source_id, source_ref, created_at)
         VALUES (?1, NULL, 'RD-001', '地图A', 'v1.0', '部分标定', 'map.zip', ?2, ?3,
            'robot-mapping', ?4, ?5, '2026-11-10 23:12:00')",
    )
    .bind("ROS-MAP-42@v1.0")
    .bind(stored.display().to_string())
    .bind(original.len() as i64)
    .bind(ZENTAI_ID.to_string())
    .bind(SOURCE_REF)
    .execute(&state.db)
    .await
    .unwrap();

    let info = |mode: &str| {
        serde_json::json!({
            "mapName": "地图A",
            "deviceId": "RD-001",
            "sourceRef": SOURCE_REF,
            "mode": mode,
        })
    };

    // 1) 增量模式：命中 sourceRef 直接跳过，不碰中台
    let body = post_ingest(&app, &token, &original, info("incremental")).await;
    assert_eq!(body["data"]["skipped"], true, "增量模式应跳过: {body}");
    assert!(uploads.lock().unwrap().is_empty(), "增量模式不应上传中台");

    // 2) 刷新模式 + 完全相同的内容（只是重新打包的时间戳不同）：跳过
    let repacked = robot_map_zip(b"P5\n4 4\n255\n\x01\x02\x03\x04", (2027, 1, 2, 3, 4, 5));
    assert_ne!(repacked, original, "时间戳不同，整包字节必然不同");
    let body = post_ingest(&app, &token, &repacked, info("refresh")).await;
    assert_eq!(body["data"]["skipped"], true, "内容未变化应跳过: {body}");
    assert!(
        body["data"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("内容未变化"),
        "跳过原因应说明内容未变化: {body}"
    );
    assert!(uploads.lock().unwrap().is_empty(), "内容未变化不应上传中台");

    // 3) 刷新模式 + 改过栅格：覆盖中台同一条记录，业务行沿用原 id
    let changed = robot_map_zip(b"P5\n4 4\n255\n\x09\x08\x07\x06", (2027, 1, 2, 3, 4, 5));
    let body = post_ingest(&app, &token, &changed, info("refresh")).await;
    assert_eq!(body["data"]["skipped"], false, "内容变化应刷新: {body}");
    assert_eq!(
        body["data"]["mapId"].as_str(),
        Some("ROS-MAP-42@v1.0"),
        "刷新应沿用业务地图 id（路线/任务引用它）: {body}"
    );
    assert_eq!(body["data"]["replaced"], true, "应标记为原地覆盖");
    let sent = uploads.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "变化后应上传一次中台: {sent:?}");
    assert_eq!(
        sent[0]["id"].as_i64(),
        Some(ZENTAI_ID),
        "刷新必须带中台 id 原地覆盖，不能新建副本: {:?}",
        sent[0]
    );

    // 本地副本与业务行都被刷新成新内容
    let refreshed = std::fs::read(body["data"]["stashPath"].as_str().unwrap()).expect("读取暂存包");
    assert_eq!(sha256_hex(&refreshed), sha256_hex(&changed));
}

/// 假中台：`/exists` 只认 `existing` 里的 id（模拟「其余地图已被软删除」），
/// `/upload` 记录收到的 mapInfo 并回 `upload_id`。
async fn fake_zentai(
    token: &'static str,
    existing: Vec<i64>,
    upload_id: i64,
    upload_version: Option<&'static str>,
) -> (String, Arc<std::sync::Mutex<Vec<Value>>>) {
    use axum::extract::Multipart;
    use axum::http::HeaderMap;
    use axum::routing::{get, post};
    use std::sync::Mutex as StdMutex;

    let uploads: Arc<StdMutex<Vec<Value>>> = Arc::new(StdMutex::new(Vec::new()));
    let recorded = uploads.clone();
    let zentai = axum::Router::new()
        .route(
            "/api/dogInternal/maps/exists",
            post(move |_headers: HeaderMap, _body: axum::Json<Value>| {
                let existing = existing.clone();
                async move {
                    axum::Json(serde_json::json!({
                        "code": 10200,
                        "msg": "success",
                        "data": {"existing": existing},
                    }))
                }
            }),
        )
        .route(
            "/api/dogInternal/devices/assignments",
            get(|_headers: HeaderMap| async move {
                axum::Json(serde_json::json!({"code": 10200, "msg": "success", "data": []}))
            }),
        )
        .route(
            "/api/dogInternal/maps/upload",
            post(move |headers: HeaderMap, mut multipart: Multipart| {
                let recorded = recorded.clone();
                async move {
                    assert_eq!(
                        headers.get("x-service-token").and_then(|v| v.to_str().ok()),
                        Some(token)
                    );
                    let mut map_name = String::new();
                    while let Some(field) = multipart.next_field().await.unwrap() {
                        match field.name() {
                            Some("file") => {
                                let _ = field.bytes().await.unwrap();
                            }
                            Some("mapInfo") => {
                                let text = field.text().await.unwrap();
                                let info: Value = serde_json::from_str(&text).unwrap();
                                map_name = info["mapName"].as_str().unwrap_or_default().to_string();
                                recorded.lock().unwrap().push(info);
                            }
                            _ => {}
                        }
                    }
                    axum::Json(serde_json::json!({
                        "code": 10200,
                        "msg": "success",
                        "data": {
                            "id": upload_id,
                            "mapName": map_name,
                            "mapVersion": upload_version,
                            "fileName": "map.zip",
                        },
                    }))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake zentai");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, zentai).await;
    });
    (format!("http://{addr}"), uploads)
}

/// 用假中台搭一个「业务库里已经有一行 + 本地存了同一份包」的现场。
async fn ingest_state(
    zentai_base: &str,
) -> (AppState, TempDb, axum::Router, String, Vec<u8>) {
    let (state, db_dir) = test_state_with_token(Some("test-service-token")).await;
    let mut config = (*state.config).clone();
    config.zentai_base_url = Some(zentai_base.to_string());
    let state = AppState {
        db: state.db,
        notifier: robodog_ops_server::notify::Notifier::disabled(),
        config: Arc::new(config),
        device_connections: state.device_connections,
        command_results: state.command_results,
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
        phone_audio: Default::default(),
        ai_patrol: Default::default(),
        caches: Default::default(),
    };
    let app = build_router(state.clone());
    let token = login(&app).await;
    let package = robot_map_zip(b"P5\n4 4\n255\n\x01\x02\x03\x04", (2026, 11, 27, 9, 54, 17));
    (state, db_dir, app, token, package)
}

/// 回归 2026-11-27 现场：在中台**删掉**一张机器人地图后点「同步地图」，
/// 以前因为「本体包内容没变」直接跳过（回执「无需更新」），中台永远补不回来。
///
/// 中台是地图主库，被删的图必须能靠同步重传回来：内容没变也要新建一条。
#[tokio::test]
async fn ingest_refresh_reuploads_map_deleted_in_zentai() {
    const SOURCE_REF: &str = "/ota/alg_data/map/history_map/2026_11_27_09_54_17";
    // 中台里这张图已被软删（exists 不返回 42）
    let (base, uploads) = fake_zentai("test-service-token", Vec::new(), 99, None).await;
    let (state, db_dir, app, token, package) = ingest_state(&base).await;
    let stored = db_dir.dir.join("stored-map.zip");
    std::fs::write(&stored, &package).expect("写入本地副本");
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, device_code, map_name, version, status,
            file_name, file_path, file_size, source, source_id, source_ref, created_at)
         VALUES ('ROS-MAP-42@v1.0', NULL, 'RD-001', '2026_11_27_09_54_17', 'v1.0', '部分标定',
            'map.zip', ?1, ?2, 'robot-mapping', '42', ?3, '2026-11-27 13:52:06')",
    )
    .bind(stored.display().to_string())
    .bind(package.len() as i64)
    .bind(SOURCE_REF)
    .execute(&state.db)
    .await
    .unwrap();

    let info = serde_json::json!({
        "mapName": "2026_11_27_09_54_17",
        "deviceId": "RD-001",
        "sourceRef": SOURCE_REF,
        "mode": "refresh",
    });
    let body = post_ingest(&app, &token, &package, info).await;

    assert_eq!(
        body["data"]["skipped"], false,
        "中台已经删了这张图，即使内容和本地副本一致也必须重传: {body}"
    );
    assert_eq!(
        body["data"]["replaced"], false,
        "中台没有这张图了，不能带 id 去覆盖（会撞「地图不存在」）: {body}"
    );
    assert_eq!(
        body["data"]["mapId"].as_str(),
        Some("ROS-MAP-42@v1.0"),
        "重传要沿用原业务地图 id（路线 / 任务引用它）: {body}"
    );
    assert_eq!(body["data"]["zentaiMapId"].as_i64(), Some(99));
    let sent = uploads.lock().unwrap().clone();
    assert_eq!(sent.len(), 1, "应重新上传一次中台: {sent:?}");
    assert!(
        sent[0]["id"].is_null(),
        "中台已没有这条记录，mapInfo 不能带 id: {:?}",
        sent[0]
    );
}

/// 遥控器改完同步到一张**中台已发布**的地图：中台自动新建下一个版本（返回新 id + v1.1），
/// 业务平台也跟着**新增一行**；旧行继续指向旧版本（路线 / 任务还引用它）。
///
/// 以前这种事只会撞「已发布不可原地覆盖」400，现场只能人工派生版本，派生出来的占位行
/// 又没有中台链接，于是两边的地图数量越差越多（2026-11-27 现场 7 : 12）。
#[tokio::test]
async fn ingest_new_zentai_version_creates_a_business_row() {
    const SOURCE_REF: &str = "/ota/alg_data/map/history_map/2026_11_26_15_21_40";
    // 中台那张图还在（exists 返回 42），但因为它已发布，中台把这次同步做成了新版本 #100
    let (base, uploads) = fake_zentai("test-service-token", vec![42], 100, Some("v1.1")).await;
    let (state, _db_dir, app, token, package) = ingest_state(&base).await;
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, device_code, map_name, version, status,
            file_name, file_path, file_size, source, source_id, source_ref, created_at)
         VALUES ('ROS-MAP-42@v1.0', 1, 'RD-001', '2026_11_26_15_21_40', 'v1.0', '已发布',
            'map.zip', 'maps/missing.zip', 1, 'robot-mapping', '42', ?1, '2026-11-26 15:46:13')",
    )
    .bind(SOURCE_REF)
    .execute(&state.db)
    .await
    .unwrap();

    let info = serde_json::json!({
        "mapName": "2026_11_26_15_21_40",
        "deviceId": "RD-001",
        "sourceRef": SOURCE_REF,
        "mode": "refresh",
    });
    let body = post_ingest(&app, &token, &package, info).await;

    assert_eq!(body["data"]["zentaiMapId"].as_i64(), Some(100), "{body}");
    assert_eq!(
        body["data"]["mapId"].as_str(),
        Some("ROS-MAP-100@v1.1"),
        "中台新建版本 → 业务也新增一行，版本号跟中台走: {body}"
    );
    let sent = uploads.lock().unwrap().clone();
    assert_eq!(
        sent[0]["id"].as_i64(),
        Some(42),
        "仍然带着旧 id 请求覆盖（要不要新版本由中台决定）: {sent:?}"
    );

    // 旧行不动：已发布、仍指向 #42（路线 / 任务还在用它）
    let (status, source_id): (String, String) =
        sqlx::query_as("SELECT status, source_id FROM map_versions WHERE id = 'ROS-MAP-42@v1.0'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert_eq!(status, "已发布");
    assert_eq!(source_id, "42");

    // 新行：部分标定（等中台发布）、指向 #100、保留本体路径
    let (status, version, source_id, source_ref): (String, String, String, Option<String>) =
        sqlx::query_as(
            "SELECT status, version, source_id, source_ref FROM map_versions
             WHERE id = 'ROS-MAP-100@v1.1'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(status, "部分标定");
    assert_eq!(version, "v1.1");
    assert_eq!(source_id, "100");
    assert_eq!(source_ref.as_deref(), Some(SOURCE_REF));
}

/// 回归 2026-11-27 现场（第二条）：同一个本体目录在业务库里可能留着**已归档**的历史行
/// （曾指向一张「已发布」的中台地图）。以前刷新时拿它去覆盖，被中台
/// 「已发布不可原地覆盖」400 拒掉，整条同步跟着报失败——运维只看到「同步失败」。
///
/// 归档行不是这张图的副本：**跳过、不算失败**，既不去覆盖已发布的中台地图，
/// 也不新建重复副本。
#[tokio::test]
async fn ingest_refresh_skips_archived_business_row_instead_of_failing() {
    const SOURCE_REF: &str = "/ota/alg_data/map/history_map/2026_11_23_10_49_02";
    // 中台那张「已发布」的图还在（exists 返回 42）
    let (base, uploads) = fake_zentai("test-service-token", vec![42], 100, None).await;
    let (state, db_dir, app, token, package) = ingest_state(&base).await;
    let stored = db_dir.dir.join("archived-map.zip");
    std::fs::write(&stored, &package).expect("写入本地副本");
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, device_code, map_name, version, status,
            file_name, file_path, file_size, source, source_id, source_ref, created_at)
         VALUES ('ROS-MAP-42@v1.0', NULL, 'RD-001', '2026_11_23_10_49_02', 'v1.0', '已归档',
            'map.zip', ?1, ?2, 'robot-mapping', '42', ?3, '2026-11-23 10:57:30')",
    )
    .bind(stored.display().to_string())
    .bind(package.len() as i64)
    .bind(SOURCE_REF)
    .execute(&state.db)
    .await
    .unwrap();

    let info = serde_json::json!({
        "mapName": "2026_11_23_10_49_02",
        "deviceId": "RD-001",
        "sourceRef": SOURCE_REF,
        "mode": "refresh",
    });
    let body = post_ingest(&app, &token, &package, info).await;

    assert_eq!(body["data"]["skipped"], true, "归档行应跳过: {body}");
    assert!(
        body["data"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("已归档"),
        "跳过原因要说清是归档记录: {body}"
    );
    assert_eq!(
        body["data"]["zentaiMapId"].as_i64(),
        Some(42),
        "已发布的中台地图原样保留: {body}"
    );
    assert!(
        uploads.lock().unwrap().is_empty(),
        "跳过不能上传 → 不新建重复副本，也不撞「已发布不可覆盖」"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM map_versions WHERE id = 'ROS-MAP-42@v1.0'")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(status, "已归档");
}

/// 回归 2026-11-27 现场（第三条）：控制台「派生新版本」建出来的**占位行**
/// （`source = local`、没有 `source_id`、没有本地包）不能被「同步地图」当成机器人副本。
///
/// 以前它会被覆盖成机器人原始包、`source_id` 被改指一张新建的中台地图，
/// 连带把绑在它上面的园区基线换掉（现场：园区「广汇产业园二期」的 v1.1 基线被改）。
#[tokio::test]
async fn ingest_refresh_leaves_derived_map_versions_untouched() {
    const SOURCE_REF: &str = "/ota/alg_data/map/history_map/2026_11_23_10_49_02";
    let (base, uploads) = fake_zentai("test-service-token", vec![42], 100, None).await;
    let (state, _db_dir, app, token, package) = ingest_state(&base).await;
    // 派生版本：没有 source_id、没有 file_path/file_size（控制台只记标定信息）
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, device_code, map_name, version, status,
            source, source_id, source_ref, created_at)
         VALUES ('2026_11_23_10_49_02@v1.1', NULL, 'RD-001', '2026_11_23_10_49_02', 'v1.1',
            '已发布', 'local', NULL, ?1, '2026-11-27 09:49:17')",
    )
    .bind(SOURCE_REF)
    .execute(&state.db)
    .await
    .unwrap();

    let info = serde_json::json!({
        "mapName": "2026_11_23_10_49_02",
        "deviceId": "RD-001",
        "sourceRef": SOURCE_REF,
        "mode": "refresh",
    });
    let body = post_ingest(&app, &token, &package, info).await;

    assert_eq!(body["data"]["skipped"], true, "派生行应跳过: {body}");
    assert!(
        uploads.lock().unwrap().is_empty(),
        "派生行不该触发中台上传（会凭空多出一张重复地图）"
    );
    // 派生行一个字段都不许动
    let (status, source, source_id, file_path): (String, String, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT status, source, source_id, file_path FROM map_versions
             WHERE id = '2026_11_23_10_49_02@v1.1'",
        )
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(status, "已发布");
    assert_eq!(source, "local");
    assert_eq!(source_id, None, "派生行不该被挂上中台 id");
    assert_eq!(file_path, None, "派生行不该被塞进机器人包");
}

/// 排班 / 工单主流程：责任人库 → 排班（冲突拦截）→ 告警转工单 → 状态流转留痕。
#[tokio::test]
async fn schedules_and_work_orders_flow() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let auth = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            (response.status(), json_body(response).await)
        }
    };
    let post = |path: &str, body: String| {
        let mut request = auth("POST", path);
        *request.body_mut() = Body::from(body);
        request
    };
    let put = |path: &str, body: String| {
        let mut request = auth("PUT", path);
        *request.body_mut() = Body::from(body);
        request
    };

    // 演示数据默认不播种：新库的责任人库是空的，测试自己建
    let (status, staff) = send(auth("GET", "/api/staff")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        staff["data"].as_array().map(Vec::len),
        Some(0),
        "默认不应写入示例责任人"
    );

    // 责任人库增：`fetch_staff` 用编号参数（?1/?2/?3），漏绑会直接 500
    let (status, new_staff) = send(post(
        "/api/staff",
        r#"{"name":"冒烟责任人","role":"运维工程师","team":"运维三组","phone":"13800000000"}"#
            .to_string(),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "新增责任人应成功: {new_staff}");
    let staff_id = new_staff["data"]["id"].as_i64().expect("缺少责任人 id");
    assert_eq!(new_staff["data"]["staffCode"], format!("EMP-{staff_id:02}"));
    assert_eq!(new_staff["data"]["status"], "在岗");

    // 先排一条历史班，验证「责任人改名 → 排班里的冗余姓名跟着改」
    let (status, past_shift) = send(post(
        "/api/shifts",
        format!(
            r#"{{"shiftDate":"2020-12-31","slot":"早班","staffId":{staff_id},
                "parkId":1,"zoneId":1,"status":"已发布"}}"#
        ),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "准备历史排班失败: {past_shift}");

    // 责任人库改：角色 / 状态 / 技能都能改，改名同步排班
    let (status, renamed) = send(put(
        &format!("/api/staff/{staff_id}"),
        r#"{"name":"冒烟责任人2","status":"休息","skills":"红外测温 / 应急处置"}"#.to_string(),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "修改责任人应成功: {renamed}");
    assert_eq!(renamed["data"]["name"], "冒烟责任人2");
    assert_eq!(renamed["data"]["status"], "休息");
    assert_eq!(renamed["data"]["skills"], "红外测温 / 应急处置");
    let (_, past_list) = send(auth("GET", "/api/shifts?from=2020-12-31&to=2020-12-31")).await;
    assert_eq!(
        past_list["data"][0]["ownerName"], "冒烟责任人2",
        "改名后历史排班的姓名应同步"
    );

    // 新建排班（用很远的日期，避免与其它用例撞车）
    let (status, created) = send(post(
        "/api/shifts",
        format!(
            r#"{{"shiftDate":"2030-01-01","slot":"早班","staffId":{staff_id},
                "parkId":1,"zoneId":1,"status":"已发布"}}"#
        ),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "新建排班应成功: {created}");
    assert_eq!(created["data"]["timeRange"], "08:00 - 16:00");
    assert_eq!(
        created["data"]["deviceCount"], 2,
        "负责设备按园区/区域实时统计"
    );
    let shift_id = created["data"]["id"].as_i64().expect("缺少排班 id");

    // 同一责任人同一天同一班次 → 冲突
    let (status, conflict) = send(post(
        "/api/shifts",
        format!(r#"{{"shiftDate":"2030-01-01","slot":"早班","staffId":{staff_id}}}"#),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "重复排班应被拦截");
    assert!(
        conflict["msg"]
            .as_str()
            .unwrap_or_default()
            .contains("冲突"),
        "提示应说明冲突原因: {conflict}"
    );

    // 周视图拖拽复制：把这条排班拖到另一天的另一个班次
    let (status, duplicated) = send(post(
        &format!("/api/shifts/{shift_id}/duplicate"),
        r#"{"shiftDate":"2030-01-02","slot":"中班"}"#.to_string(),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "拖拽复制应成功: {duplicated}");
    assert_eq!(duplicated["data"]["shiftDate"], "2030-01-02");
    assert_eq!(duplicated["data"]["slot"], "中班");
    assert_eq!(duplicated["data"]["timeRange"], "16:00 - 00:00");
    assert_eq!(duplicated["data"]["ownerName"], "冒烟责任人2");
    assert_eq!(duplicated["data"]["parkId"], 1, "园区要一起复制");
    assert_eq!(duplicated["data"]["status"], "已发布", "状态一起复制");
    assert_ne!(
        duplicated["data"]["shiftCode"],
        created["data"]["shiftCode"]
    );
    let duplicated_id = duplicated["data"]["id"].as_i64().unwrap();

    // 拖到原格子 / 拖到已有同名同时段 → 明确拒绝
    let (status, same) = send(post(
        &format!("/api/shifts/{duplicated_id}/duplicate"),
        r#"{"shiftDate":"2030-01-02","slot":"中班"}"#.to_string(),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "拖回原格子应拒绝: {same}");
    let (status, dup_conflict) = send(post(
        &format!("/api/shifts/{shift_id}/duplicate"),
        r#"{"shiftDate":"2030-01-02","slot":"中班"}"#.to_string(),
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "目标时段已有人应拒绝: {dup_conflict}"
    );
    assert!(
        dup_conflict["msg"]
            .as_str()
            .unwrap_or_default()
            .contains("冲突"),
        "提示应说明冲突: {dup_conflict}"
    );
    let (status, _) = send(auth("DELETE", &format!("/api/shifts/{duplicated_id}"))).await;
    assert_eq!(status, StatusCode::OK);

    // 告警转工单：保留来源编号，并原告警推进到「处理中」
    let (status, order) = send(post(
        "/api/workorders",
        format!(
            r#"{{"title":"冒烟测试工单","orderType":"告警处置","priority":"高",
                "staffId":{staff_id},"parkId":1,"deviceId":1,"source":"告警 AL-001",
                "sourceRef":"AL-001","alertId":1}}"#
        ),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "新建工单应成功: {order}");
    assert_eq!(order["data"]["status"], "待接单");
    assert_eq!(order["data"]["assignee"], "冒烟责任人2");
    let order_id = order["data"]["id"].as_i64().expect("缺少工单 id");
    assert_eq!(
        order["data"]["logs"].as_array().map(Vec::len),
        Some(1),
        "创建动作要留一条轨迹"
    );

    // 还有未来排班 + 在办工单时不允许删责任人（删了会在排班/派单里留空档）
    let (status, blocked) = send(auth("DELETE", &format!("/api/staff/{staff_id}"))).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "有在办工单/未来排班应拒绝删除"
    );
    assert!(
        blocked["msg"].as_str().unwrap_or_default().contains("排班"),
        "提示应说明还有排班/工单: {blocked}"
    );

    // 状态流转：待接单 → 处理中（进度 30 + 轨迹追加）
    let (status, advanced) = send(put(
        &format!("/api/workorders/{order_id}/status"),
        r#"{"status":"处理中"}"#.to_string(),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(advanced["data"]["status"], "处理中");
    assert_eq!(advanced["data"]["progress"], 30);
    assert_eq!(
        advanced["data"]["logs"].as_array().map(Vec::len),
        Some(2),
        "流转要留痕"
    );

    // 追加处理记录
    let (status, logged) = send(post(
        &format!("/api/workorders/{order_id}/logs"),
        r#"{"content":"已到达现场，开始排查"}"#.to_string(),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(logged["data"]["logs"].as_array().map(Vec::len), Some(3));

    let (_, stats) = send(auth("GET", "/api/workorders/stats")).await;
    assert!(stats["data"]["open"].as_i64().unwrap_or_default() >= 1);
    let (_, shift_stats) = send(auth("GET", "/api/shifts/stats")).await;
    assert!(shift_stats["data"]["total"].as_i64().unwrap_or_default() >= 1);
    // 去重人数跨状态统计：按状态分组后取最大值会漏人
    assert_eq!(
        shift_stats["data"]["owners"].as_i64(),
        Some(1),
        "两个班次都是同一个责任人: {shift_stats}"
    );

    // 清理：工单（连带轨迹）→ 未来排班 → 责任人
    let (status, _) = send(auth("DELETE", &format!("/api/workorders/{order_id}"))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(auth("DELETE", &format!("/api/shifts/{shift_id}"))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, removed) = send(auth("DELETE", &format!("/api/staff/{staff_id}"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "只剩历史排班的责任人应可删除: {removed}"
    );

    // 历史排班保留（姓名还在），但不再引用责任人库
    let (_, past_after) = send(auth("GET", "/api/shifts?from=2020-12-31&to=2020-12-31")).await;
    let rows = past_after["data"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "历史排班不应被连带删除: {past_after}");
    assert!(rows[0]["staffId"].is_null(), "历史排班应解除责任人引用");
    assert_eq!(rows[0]["ownerName"], "冒烟责任人2");
}

/// 排班整段复制：周复制按星期对齐、月复制按日号对齐（天数不足跳过）、
/// 重复复制不产生重复记录、覆盖模式替换目标时段。
#[tokio::test]
async fn shift_copy_aligns_periods_and_handles_conflicts() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let auth = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            (response.status(), json_body(response).await)
        }
    };
    let post = |path: &str, body: &str| {
        let mut request = auth("POST", path);
        *request.body_mut() = Body::from(body.to_string());
        request
    };

    // 演示数据默认不播种：先建责任人，后面所有排班都挂在这个人身上
    let (status, staff) = send(post(
        "/api/staff",
        r#"{"name":"复制用例责任人","role":"运维工程师"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "准备责任人失败: {staff}");
    let staff_id = staff["data"]["id"].as_i64().unwrap();
    let (status, other_staff) = send(post(
        "/api/staff",
        r#"{"name":"占位责任人","role":"运维工程师"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "准备责任人失败: {other_staff}");
    let other_staff_id = other_staff["data"]["id"].as_i64().unwrap();

    // 源区间：2031-03（3 月的 3 号、4 号、31 号各一条）
    for (date, slot, owner_id) in [
        ("2031-03-03", "早班", staff_id),
        ("2031-03-04", "中班", other_staff_id),
        ("2031-03-31", "夜班", staff_id),
    ] {
        let (status, body) = send(post(
            "/api/shifts",
            &format!(
                r#"{{"shiftDate":"{date}","slot":"{slot}","staffId":{owner_id},
                    "parkId":1,"status":"已发布"}}"#
            ),
        ))
        .await;
        assert_eq!(status, StatusCode::OK, "准备源排班失败: {body}");
    }

    // 月复制：3 月 → 4 月，31 号超出 4 月天数被跳过
    let (status, result) = send(post(
        "/api/shifts/copy",
        r#"{"sourceFrom":"2031-03-01","sourceTo":"2031-03-31",
            "targetFrom":"2031-04-01","targetTo":"2031-04-30","status":"待确认"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "月复制应成功: {result}");
    assert_eq!(result["data"]["source"], 3);
    assert_eq!(result["data"]["created"], 2, "3/31 应因 4 月无 31 号被跳过");
    assert_eq!(result["data"]["skipped"].as_array().map(Vec::len), Some(1));
    assert!(
        result["data"]["skipped"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("天数不足"),
        "跳过原因应说明目标区间天数不足: {result}"
    );

    // 目标区间拿到的是「同一日号 + 复制时指定的状态」
    let (_, target) = send(auth("GET", "/api/shifts?from=2031-04-01&to=2031-04-30")).await;
    let rows = target["data"].as_array().expect("shifts 应为数组");
    assert_eq!(rows.len(), 2, "4 月应有 2 条复制记录: {target}");
    let dates: Vec<&str> = rows
        .iter()
        .filter_map(|row| row["shiftDate"].as_str())
        .collect();
    assert_eq!(dates, vec!["2031-04-03", "2031-04-04"]);
    assert!(rows.iter().all(|row| row["status"] == "待确认"));

    // 再复制一次（跳过模式）：不产生重复记录，全部回报「已有排班」
    let (status, again) = send(post(
        "/api/shifts/copy",
        r#"{"sourceFrom":"2031-03-01","sourceTo":"2031-03-31",
            "targetFrom":"2031-04-01","targetTo":"2031-04-30"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["data"]["created"], 0);
    assert_eq!(again["data"]["skipped"].as_array().map(Vec::len), Some(3));
    assert!(
        again["data"]["skipped"][0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("已有排班"),
        "跳过原因应说明目标时段已占用: {again}"
    );

    // 目标时段被别人占了 → 覆盖模式替换该时段
    let (status, occupant) = send(post(
        "/api/shifts",
        &format!(
            r#"{{"shiftDate":"2031-04-03","slot":"早班","staffId":{other_staff_id},"status":"已发布"}}"#
        ),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "准备占用排班失败: {occupant}");
    let occupant_id = occupant["data"]["id"].as_i64().unwrap();
    let (status, replaced) = send(post(
        "/api/shifts/copy",
        r#"{"sourceFrom":"2031-03-01","sourceTo":"2031-03-31",
            "targetFrom":"2031-04-01","targetTo":"2031-04-30","mode":"replace"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "覆盖模式复制失败: {replaced}");
    assert_eq!(replaced["data"]["created"], 2);
    assert!(replaced["data"]["replaced"].as_i64().unwrap_or_default() >= 1);
    let (_, after) = send(auth("GET", "/api/shifts?from=2031-04-01&to=2031-04-30")).await;
    assert!(
        !after["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == occupant_id),
        "覆盖模式应删掉目标时段原有排班"
    );

    // 周复制：保持同名星期（源周同一天 → 下周同一天）
    let (status, weekday_source) = send(post(
        "/api/shifts",
        &format!(r#"{{"shiftDate":"2031-04-07","slot":"早班","staffId":{staff_id},"parkId":1}}"#),
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "准备周复制源排班失败: {weekday_source}"
    );
    let (_, week) = send(post(
        "/api/shifts/copy",
        r#"{"sourceFrom":"2031-04-07","sourceTo":"2031-04-13",
            "targetFrom":"2031-04-14","targetTo":"2031-04-20"}"#,
    ))
    .await;
    assert_eq!(week["data"]["created"], 1, "周复制应复制 1 条: {week}");
    let (_, week_target) = send(auth("GET", "/api/shifts?from=2031-04-14&to=2031-04-20")).await;
    let week_rows = week_target["data"].as_array().unwrap();
    assert_eq!(week_rows.len(), 1, "目标周应只有 1 条: {week_target}");
    assert_eq!(week_rows[0]["shiftDate"], "2031-04-14");
    assert_eq!(week_rows[0]["ownerName"], "复制用例责任人");

    // 区间重叠直接拒绝（避免把刚复制的记录再复制一遍）
    let (status, overlap) = send(post(
        "/api/shifts/copy",
        r#"{"sourceFrom":"2031-04-01","sourceTo":"2031-04-30",
            "targetFrom":"2031-04-01","targetTo":"2031-04-30"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "重叠区间应拒绝: {overlap}");
    assert!(
        overlap["msg"].as_str().unwrap_or_default().contains("重叠"),
        "提示应说明区间重叠: {overlap}"
    );
}

/// 班次自定义：默认早 / 中 / 夜可改，也能自己加「白班」这类班次。
///
/// 关注三件事：时段一律由服务端按班次表算（前端只传班次名）、
/// 改名 / 改时段要同步到已排的排班、被排班引用的班次只能停用不能删。
#[tokio::test]
async fn shift_slots_are_customizable() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let auth = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            (response.status(), json_body(response).await)
        }
    };
    let post = |path: &str, body: &str| {
        let mut request = auth("POST", path);
        *request.body_mut() = Body::from(body.to_string());
        request
    };
    let put = |path: &str, body: &str| {
        let mut request = auth("PUT", path);
        *request.body_mut() = Body::from(body.to_string());
        request
    };

    // 迁移插入的三个默认班次：名称 / 时段 / 顺序都在，且都是启用中
    let (status, slots) = send(auth("GET", "/api/shift-slots")).await;
    assert_eq!(status, StatusCode::OK, "班次列表应可读: {slots}");
    let names: Vec<&str> = slots["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|slot| slot["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["早班", "中班", "夜班"], "应有三个默认班次");
    assert_eq!(slots["data"][0]["timeRange"], "08:00 - 16:00");
    assert_eq!(slots["data"][2]["timeRange"], "00:00 - 08:00");
    assert_eq!(slots["data"][0]["active"], true);

    // 自定义班次：`8:00` 这种写法后端会归一成 `08:00`，时段由服务端拼
    let (status, created) = send(post(
        "/api/shift-slots",
        r#"{"name":"白班","startTime":"8:00","endTime":"20:00"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "自定义班次应成功: {created}");
    let slot_id = created["data"]["id"].as_i64().unwrap();
    assert_eq!(created["data"]["timeRange"], "08:00 - 20:00");
    assert_eq!(created["data"]["sortOrder"], 4, "不指定顺序应排到最后");
    assert_eq!(created["data"]["shiftCount"], 0);

    // 名称重复 / 时间格式不合法 → 400 且提示原因
    let (status, dup) = send(post(
        "/api/shift-slots",
        r#"{"name":"白班","startTime":"08:00","endTime":"20:00"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "重名班次应拒绝: {dup}");
    let (status, bad_time) = send(post(
        "/api/shift-slots",
        r#"{"name":"早高峰","startTime":"7点","endTime":"09:00"}"#,
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "非法时间应拒绝: {bad_time}"
    );
    assert!(
        bad_time["msg"]
            .as_str()
            .unwrap_or_default()
            .contains("HH:MM"),
        "提示应说明时间格式: {bad_time}"
    );

    // 用自定义班次排班：时段跟着班次走，未知班次直接拒绝
    let (status, staff) = send(post(
        "/api/staff",
        r#"{"name":"自定义班次责任人","role":"运维工程师"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "准备责任人失败: {staff}");
    let staff_id = staff["data"]["id"].as_i64().unwrap();
    let (status, shift) = send(post(
        "/api/shifts",
        &format!(
            r#"{{"shiftDate":"2032-05-01","slot":"白班","staffId":{staff_id},
                "parkId":1,"status":"已发布"}}"#
        ),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "自定义班次应能排班: {shift}");
    assert_eq!(shift["data"]["timeRange"], "08:00 - 20:00");
    let shift_id = shift["data"]["id"].as_i64().unwrap();

    let (status, unknown) = send(post(
        "/api/shifts",
        &format!(r#"{{"shiftDate":"2032-05-02","slot":"不存在的班次","staffId":{staff_id}}}"#),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "未知班次应拒绝: {unknown}");
    assert!(
        unknown["msg"]
            .as_str()
            .unwrap_or_default()
            .contains("班次设置"),
        "提示应指路「班次设置」: {unknown}"
    );

    // 改时段 + 改名：已排的排班同步刷成新名称与新时段
    let (status, renamed) = send(put(
        &format!("/api/shift-slots/{slot_id}"),
        r#"{"name":"早晚班","startTime":"08:00","endTime":"21:30"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "修改班次应成功: {renamed}");
    assert_eq!(renamed["data"]["name"], "早晚班");
    assert_eq!(renamed["data"]["shiftCount"], 1);
    let (_, shift_after) = send(auth("GET", &format!("/api/shifts/{shift_id}"))).await;
    assert_eq!(shift_after["data"]["slot"], "早晚班");
    assert_eq!(shift_after["data"]["timeRange"], "08:00 - 21:30");

    // 还有排班在用时不允许删除，只能停用
    let (status, in_use) = send(auth("DELETE", &format!("/api/shift-slots/{slot_id}"))).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "在用班次不应能删: {in_use}"
    );
    assert!(
        in_use["msg"].as_str().unwrap_or_default().contains("停用"),
        "提示应建议改为停用: {in_use}"
    );

    // 停用后：列表里还在（active=false），新建排班不带班次时落到第一个启用班次
    let (status, deactivated) = send(put(
        &format!("/api/shift-slots/{slot_id}"),
        r#"{"active":false}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "停用班次应成功: {deactivated}");
    assert_eq!(deactivated["data"]["active"], false);
    let (status, fallback) = send(post(
        "/api/shifts",
        &format!(r#"{{"shiftDate":"2032-05-03","staffId":{staff_id},"status":"草案"}}"#),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "不传班次应落到默认班次: {fallback}");
    assert_eq!(fallback["data"]["slot"], "早班");

    // 没被引用的班次可以直接删
    let (status, spare) = send(post(
        "/api/shift-slots",
        r#"{"name":"临时班","startTime":"12:00","endTime":"14:00"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "新增临时班次失败: {spare}");
    let spare_id = spare["data"]["id"].as_i64().unwrap();
    let (status, deleted) = send(auth("DELETE", &format!("/api/shift-slots/{spare_id}"))).await;
    assert_eq!(status, StatusCode::OK, "未使用的班次应能删除: {deleted}");
}

/// 告警自动派单：告警 → 当天排班找到责任人 → 自动建工单 → 推送消息带上责任人与工单号。
///
/// 这里刻意用 `Notifier::disabled()` 的 state：**派单与建单不能依赖企业微信配置**，
/// 现场没配 webhook 也必须有人管这条告警。
#[tokio::test]
async fn alert_auto_dispatches_to_duty_owner() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    let app = build_router(state.clone());
    let token = login(&app).await;

    let auth = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            (response.status(), json_body(response).await)
        }
    };
    let post = |path: &str, body: String| {
        let mut request = auth("POST", path);
        *request.body_mut() = Body::from(body);
        request
    };

    // 设备绑到园区 2 / 区域 4：告警只有园区+区域，没有计划号也能对上当班排班
    sqlx::query("UPDATE devices SET park_id = 2, zone_id = 4 WHERE device_id = 'RD-002'")
        .execute(&pool)
        .await
        .unwrap();
    let device_pk: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE device_id = 'RD-002'")
        .fetch_one(&pool)
        .await
        .unwrap();

    // 告警模拟的是「设备正在执行计划时报错」：现行口径（`alert_only_in_task`）只有
    // 执行计划期间的告警才会自动建单 + 推群，所以先把设备上的任务置成执行中。
    sqlx::query("UPDATE tasks SET task_status = '执行中' WHERE device_id = ?1")
        .bind(device_pk)
        .execute(&pool)
        .await
        .unwrap();

    // 责任人 + 今天早班排班（园区 2 / 区域 4）。演示播种的排班都在别的日期，
    // 所以今天这条是唯一候选，匹配结果可断言。
    let (status, staff) = send(post(
        "/api/staff",
        r#"{"name":"派单责任人","role":"运维工程师","team":"运维一组","phone":"13800001234"}"#
            .to_string(),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "建责任人失败: {staff}");
    let staff_id = staff["data"]["id"].as_i64().unwrap();

    let today = robodog_ops_server::db::today();
    let (status, shift) = send(post(
        "/api/shifts",
        format!(
            r#"{{"shiftDate":"{today}","slot":"早班","staffId":{staff_id},
                "parkId":2,"zoneId":4,"taskName":"装卸平台早班巡查","status":"已发布"}}"#
        ),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "建当班排班失败: {shift}");

    // 告警落库 → 自动派单
    let (status, alert) = send(post(
        "/api/alerts",
        format!(
            r#"{{"level":"严重","content":"导航指令非法（13440），航点 3 无法到达",
                "deviceId":{device_pk},"parkId":2}}"#
        ),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "建告警失败: {alert}");
    let alert_id = alert["data"]["id"].as_i64().unwrap();
    let alert_code = alert["data"]["alertCode"].as_str().unwrap().to_string();
    assert_eq!(
        alert["data"]["assignee"], "派单责任人",
        "应按当班排班派给责任人: {alert}"
    );
    assert_eq!(alert["data"]["staffId"], serde_json::json!(staff_id));
    let note = alert["data"]["assigneeNote"].as_str().unwrap_or_default();
    assert!(note.contains("当班排班"), "派单依据应写明排班: {note}");
    let order_code = alert["data"]["workOrderCode"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(order_code.starts_with("WO-"), "应自动建单: {alert}");

    // 工单内容：来源、原始告警编号、优先级、处理人
    let (_, orders) = send(auth("GET", "/api/workorders")).await;
    let rows = orders["data"].as_array().unwrap();
    let order = rows
        .iter()
        .find(|row| row["orderCode"] == serde_json::json!(order_code))
        .expect("自动派单应生成工单");
    assert_eq!(order["source"], "告警自动派单");
    assert_eq!(order["sourceRef"], serde_json::json!(alert_code));
    assert_eq!(order["alertId"], serde_json::json!(alert_id));
    assert_eq!(order["assignee"], "派单责任人");
    assert_eq!(order["priority"], "紧急", "严重告警应派紧急单");

    // 同一设备反复报同一句故障：复用那张还在办的工单，不新建第二张
    let (status, repeated) = send(post(
        "/api/alerts",
        format!(
            r#"{{"level":"严重","content":"导航指令非法（13440），航点 3 无法到达",
                "deviceId":{device_pk},"parkId":2}}"#
        ),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "重复告警写入失败: {repeated}");
    assert_ne!(repeated["data"]["alertCode"], serde_json::json!(alert_code));
    assert_eq!(
        repeated["data"]["workOrderCode"],
        serde_json::json!(order_code),
        "同类告警应挂到同一张在办工单上: {repeated}"
    );
    let (_, after) = send(auth("GET", "/api/workorders")).await;
    let same_title = after["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["title"] == order["title"])
        .count();
    assert_eq!(same_title, 1, "重复告警不应再建单: {after}");

    // 同一告警不允许再建一张单（页面「转工单」会撞上这条）
    let (status, duplicate) = send(post(
        "/api/workorders",
        format!(r#"{{"title":"重复转单","assignee":"派单责任人","alertId":{alert_id}}}"#),
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "重复转单应拒绝: {duplicate}"
    );
    assert!(
        duplicate["msg"]
            .as_str()
            .unwrap_or_default()
            .contains("已派单"),
        "提示应说明已派过单: {duplicate}"
    );

    // 双向外键（alerts.work_order_id ↔ work_orders.alert_id）：两边都要能删掉，
    // 现场删测试数据时踩过 —— 直接 DELETE 会返回 FOREIGN KEY constraint failed。
    let repeated_id = repeated["data"]["id"].as_i64().unwrap();
    for id in [alert_id, repeated_id] {
        let (status, body) = send(auth("DELETE", &format!("/api/alerts/{id}"))).await;
        assert_eq!(status, StatusCode::OK, "删除已派单的告警应成功: {body}");
    }
    let (_, after_delete) = send(auth("GET", "/api/workorders")).await;
    let order_id = after_delete["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["orderCode"] == serde_json::json!(order_code))
        .and_then(|row| row["id"].as_i64())
        .expect("工单应仍在（删告警只解除引用）");
    let (status, body) = send(auth("DELETE", &format!("/api/workorders/{order_id}"))).await;
    assert_eq!(status, StatusCode::OK, "删除工单应成功: {body}");
}

/// 企业微信 webhook 改为**平台内配置**：地址即凭据，所以读只回掩码、
/// 写单独接口、通用设置接口既看不到也不接受它。
#[tokio::test]
async fn wecom_webhook_is_configured_from_the_console() {
    let (state, _db) = test_state().await;
    let app = build_router(state.clone());
    let token = login(&app).await;
    let auth = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            (response.status(), json_body(response).await)
        }
    };
    let put = |path: &str, body: &str| {
        let mut request = auth("PUT", path);
        *request.body_mut() = Body::from(body.to_string());
        request
    };

    // 1. 新库里这行是空的：没配地址就不该自称能推
    let (status, body) = send(auth("GET", "/api/notifications/webhook")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["configured"], false, "新库应未配置: {body}");
    assert_eq!(body["data"]["source"], serde_json::Value::Null);

    // 2. 少 key= 的地址保存时就拒掉（否则发出去只会拿到 93000）
    let (status, body) = send(put(
        "/api/notifications/webhook",
        r#"{"webhook":"https://qyapi.weixin.qq.com/cgi-bin/webhook/send"}"#,
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "缺 key= 应拒绝: {body}");

    // 3. 正常地址：只回掩码，完整 key 不出现在响应里
    let key = "94fdb35e-1234-5678-9abc-def012345678";
    let (status, body) = send(put(
        "/api/notifications/webhook",
        &format!(r#"{{"webhook":"https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key={key}"}}"#),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "保存应成功: {body}");
    assert_eq!(body["data"]["configured"], true);
    assert_eq!(body["data"]["source"], "db", "平台配置的来源是 db: {body}");
    let masked = body["data"]["masked"].as_str().unwrap_or_default();
    assert!(
        masked.contains("key=94fdb35e…"),
        "掩码应保留前 8 位: {masked}"
    );
    assert!(!masked.contains(key), "掩码里不能有完整 key: {masked}");
    // 默认连「原文」字段都不返回：地址即凭据，不该随手进屏幕
    assert!(
        body["data"].get("url").is_none(),
        "普通读取不能带 url 字段: {body}"
    );

    // 3b. 管理员点「显示」才拿得到原文（保存后「我刚才配的是什么」要能找回）
    let (status, body) = send(auth("GET", "/api/notifications/webhook?reveal=true")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"]["url"]
            .as_str()
            .unwrap_or_default()
            .ends_with(key),
        "reveal=true 应返回完整地址: {body}"
    );

    // 4. 落库了（重启后仍生效），且通用设置接口既看不到这行、也不接受写
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT setting_value FROM settings WHERE setting_key = 'wecom_webhook'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert!(
        stored.as_deref().unwrap_or_default().ends_with(key),
        "地址应完整存进 settings：{stored:?}"
    );

    let (_, settings) = send(auth("GET", "/api/settings")).await;
    let keys: Vec<&str> = settings["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["settingKey"].as_str())
        .collect();
    assert!(
        !keys.contains(&"wecom_webhook"),
        "secret 类型不出现在通用设置列表: {keys:?}"
    );
    assert!(keys.contains(&"wecom_alert_push"), "其它开关应照旧可见");

    let (status, body) = send(put(
        "/api/settings",
        r#"[{"settingKey":"wecom_webhook","settingValue":"https://evil.example.com/send?key=x"}]"#,
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "敏感配置必须走专用接口: {body}"
    );

    // 5. 清空 = 回退环境变量（这里没有环境变量，于是回到未配置）
    let (status, body) = send(put("/api/notifications/webhook", r#"{"webhook":""}"#)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["configured"], false, "清空后应未配置: {body}");
    let cleared: Option<String> = sqlx::query_scalar(
        "SELECT setting_value FROM settings WHERE setting_key = 'wecom_webhook'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(cleared.as_deref(), Some(""), "清空后库里是空串");
}

/// 责任人库 Excel 导入：模板下载 → 导入（真 Excel 文件）→ 重复导入幂等 →
/// 坏行只影响自己。
#[tokio::test]
async fn staff_excel_import_flow() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let auth = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let content_type = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let disposition = response
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, content_type, disposition, bytes.to_vec())
        }
    };
    let upload = |bytes: Vec<u8>, filename: &str| {
        let boundary = "----robodogstaffimport";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
filename=\"{filename}\"\r\nContent-Type: application/vnd.openxmlformats-\
officedocument.spreadsheetml.sheet\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut request = auth("POST", "/api/staff/import");
        request.headers_mut().insert(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}")
                .parse()
                .unwrap(),
        );
        *request.body_mut() = Body::from(body);
        request
    };
    // 1) 下载模板：xlsx MIME + 中文文件名 + 真的是 zip（PK 开头）
    let (status, content_type, disposition, template) =
        send(auth("GET", "/api/staff/import-template")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        content_type.contains("spreadsheetml.sheet"),
        "模板 MIME 不对: {content_type}"
    );
    assert!(
        disposition.contains("filename*=UTF-8''") && disposition.contains("staff-import-template"),
        "缺少中文文件名回退: {disposition}"
    );
    assert!(template.starts_with(b"PK"), "模板不是 zip/xlsx");

    // 2) 导入 openpyxl 生成的真 Excel 文件（inlineStr + 数字单元格 + 空行）
    let fixture = include_bytes!("fixtures/staff-import-openpyxl.xlsx");
    let request = upload(fixture.to_vec(), "staff.xlsx");
    let (status, _, _, raw) = send(request).await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(body["code"], 10200, "导入接口应返回业务码: {body}");
    assert_eq!(body["data"]["total"], 3);
    assert_eq!(body["data"]["created"], 3, "{body}");
    assert_eq!(body["data"]["skipped"], 0);
    assert_eq!(body["data"]["failed"], 0);

    let (status, _, _, raw) = send(auth("GET", "/api/staff")).await;
    assert_eq!(status, StatusCode::OK);
    let list: Value = serde_json::from_slice(&raw).unwrap();
    let rows = list["data"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let names: Vec<&str> = rows.iter().filter_map(|row| row["name"].as_str()).collect();
    assert_eq!(names, vec!["张三", "李四", "王五 & <测试>"]);
    assert_eq!(rows[0]["staffCode"], "EMP-101");
    assert_eq!(rows[1]["staffCode"], "EMP-02", "工号留空时自动分配");
    assert_eq!(rows[1]["phone"], "13900000000", "数字单元格应还原成文本");
    assert_eq!(rows[1]["status"], "休息");
    assert_eq!(rows[2]["skills"], "充电维护");

    // 3) 同一份文件再导一次：全部跳过，不重复建人
    let request = upload(fixture.to_vec(), "staff.xlsx");
    let (status, _, _, raw) = send(request).await;
    let body: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["created"], 0, "{body}");
    assert_eq!(body["data"]["skipped"], 3);
    let (_, _, _, raw) = send(auth("GET", "/api/staff")).await;
    let list: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(
        list["data"].as_array().unwrap().len(),
        3,
        "重复导入不应新增"
    );

    // 4) 坏行只影响自己：重名跳过、状态不认识的报失败、好行照常入库
    let sheet = robodog_ops_server::xlsx::Sheet {
        name: "责任人".to_string(),
        headers: vec![
            "姓名".into(),
            "工号".into(),
            "角色".into(),
            "班组".into(),
            "联系电话".into(),
            "在岗状态".into(),
            "技能".into(),
        ],
        rows: vec![
            vec!["赵六".into(), String::new(), "值班员".into()],
            vec!["赵六".into(), String::new(), "值班员".into()],
            vec!["钱七".into(), String::new(), "值班员".into()],
            vec![String::new(), String::new(), "值班员".into()],
        ]
        .into_iter()
        .map(|row| {
            let mut row = row;
            row.resize(7, String::new());
            row
        })
        .collect(),
        widths: vec![],
    };
    let bytes = robodog_ops_server::xlsx::write_workbook(&[sheet]).expect("构造测试 xlsx 失败");
    let request = upload(bytes, "staff2.xlsx");
    let (status, _, _, raw) = send(request).await;
    let body: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 4, "{body}");
    assert_eq!(body["data"]["created"], 2, "赵六 + 钱七: {body}");
    assert_eq!(body["data"]["skipped"], 1, "文件内重名: {body}");
    assert_eq!(body["data"]["failed"], 1, "姓名为空: {body}");
    let issues = body["data"]["issues"].as_array().unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0]["row"], 3);
    assert_eq!(issues[0]["status"], "skipped");
    assert_eq!(issues[1]["row"], 5);
    assert_eq!(issues[1]["status"], "failed");

    // 5) 非 xlsx 拒收（.xls 只给可读提示，不做转换）
    let request = upload(b"not an excel file".to_vec(), "staff.xls");
    let (status, _, _, raw) = send(request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&raw).unwrap();
    assert!(
        body["msg"].as_str().unwrap_or_default().contains(".xlsx"),
        "{body}"
    );
}

/// 中继默认档位是页面可配置项（系统设置 → 视频中继）：枚举写坏了要拒绝，
/// 合法值保存后通用设置接口能读回来（板端起流时平台再随 `video_start` 下发）。
#[tokio::test]
async fn relay_profile_setting_is_validated() {
    let (state, _db) = test_state().await;
    let app = build_router(state.clone());
    let token = login(&app).await;
    let auth = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            (response.status(), json_body(response).await)
        }
    };
    let put = |path: &str, body: &str| {
        let mut request = auth("PUT", path);
        *request.body_mut() = Body::from(body.to_string());
        request
    };

    // 默认档位：现场主要走 4G，省流档 + 允许自动降档
    let (_, body) = send(auth("GET", "/api/settings")).await;
    let row = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["settingKey"] == "video_relay_profile")
        .expect("默认档位应出现在系统设置里");
    assert_eq!(row["settingValue"], "degrade", "{body}");
    let auto = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["settingKey"] == "video_relay_auto_degrade")
        .expect("自动降档开关应出现在系统设置里");
    assert_eq!(auto["settingValue"], "true", "{body}");

    // 不认识的档位名：直接拒绝，别让板端拿到一个只能回退默认的配置
    let (status, body) = send(put(
        "/api/settings",
        r#"[{"settingKey":"video_relay_profile","settingValue":"1080p"}]"#,
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非法档位应拒绝: {body}");

    // 合法值保存成功，读回来是新值
    let (status, body) = send(put(
        "/api/settings",
        r#"[{"settingKey":"video_relay_profile","settingValue":"degrade2"}]"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "合法档位应保存: {body}");
    let saved = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["settingKey"] == "video_relay_profile")
        .and_then(|row| row["settingValue"].as_str());
    assert_eq!(saved, Some("degrade2"));
}

/// 告警白名单（系统设置 → 通知策略）：默认压掉 `12608,13440`，错项当场拒收。
///
/// 这个键写错的代价很隐蔽：填了「RS 与 lidar 时间不同步」不会报错，白名单等于
/// 没配，现场只看到告警还在刷 → 只能靠接口把错项直接退回去。
#[tokio::test]
async fn alert_ignore_fault_codes_setting_is_validated() {
    let (state, _db) = test_state().await;
    let app = build_router(state.clone());
    let token = login(&app).await;
    let auth = |method: &str, path: &str| {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::empty())
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            (response.status(), json_body(response).await)
        }
    };
    let put = |body: &str| {
        let mut request = auth("PUT", "/api/settings");
        *request.body_mut() = Body::from(body.to_string());
        request
    };

    // 默认值：现场那两条高频噪音，老库升级也靠 seed 补上
    let (_, body) = send(auth("GET", "/api/settings")).await;
    let row = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["settingKey"] == "alert_ignore_fault_codes")
        .expect("告警白名单应出现在系统设置里");
    assert_eq!(row["settingValue"], "12608,13440", "{body}");

    // 粘了整句说明：直接拒收（静默丢掉 = 填了不生效，现场查不出来）
    let (status, body) = send(put(
        r#"[{"settingKey":"alert_ignore_fault_codes","settingValue":"12608,RS 与 lidar 时间不同步"}]"#,
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "非数字项应拒绝: {body}");
    assert!(
        body["msg"].as_str().unwrap_or_default().contains("RS"),
        "错误提示要点出哪一项不认识: {body}"
    );

    // 合法值：逗号 / code= 前缀 / 中文逗号都接受，保存后读回来是规范化后的原文
    let (status, body) = send(put(
        r#"[{"settingKey":"alert_ignore_fault_codes","settingValue":" code=12608，13382 "}]"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "合法白名单应保存: {body}");
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT setting_value FROM settings WHERE setting_key = 'alert_ignore_fault_codes'",
    )
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(stored.as_deref(), Some(" code=12608，13382 "));

    // 清空 = 不再过滤（不能把「没配」当成「全静默」）
    let (status, _) = send(put(
        r#"[{"settingKey":"alert_ignore_fault_codes","settingValue":""}]"#,
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// 建计划时执行状态必须校验。
///
/// 以前 `create_task_core` 把请求体里的 `taskStatus` 原样写库（发布状态有校验、执行状态
/// 没有），写坏一个值之后网关、AI 识别、看门狗里所有 `matches!(task_status, …)`
/// 都会静默走「非终态」分支，任务永远收不了口。
#[tokio::test]
async fn task_creation_rejects_unknown_execution_status() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let response = app
        .clone()
        .oneshot(
            Request::post("/api/tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    r#"{"taskName":"状态写坏的计划","taskStatus":"跑完了"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = json_body(response).await;
    assert!(
        body["msg"]
            .as_str()
            .unwrap_or_default()
            .contains("无效的任务状态"),
        "应明确说状态不合法: {body}"
    );

    // 合法状态仍然放行（默认「待执行」）
    let response = app
        .oneshot(
            Request::post("/api/tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"taskName":"正常计划"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"]["taskStatus"], "待执行");
}

/// 并发建计划不能拿到重复的 `taskCode`。
///
/// `task_code` 是 `UNIQUE`，以前靠 `SELECT COALESCE(MAX(id),0)+1` 先读后写，
/// 池子里 8 条连接同时进来就会撞唯一键（返回 500 且把 SQLite 原文回给前端）。
/// 现在走 `ids::next_id` 的原子分配，这里从 HTTP 层压一遍。
#[tokio::test]
async fn concurrent_task_creation_never_reuses_a_task_code() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let mut handles = Vec::new();
    for index in 0..8 {
        let app = app.clone();
        let token = token.clone();
        handles.push(tokio::spawn(async move {
            let response = app
                .oneshot(
                    Request::post("/api/tasks")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::from(format!(r#"{{"taskName":"并发计划-{index}"}}"#)))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = json_body(response).await;
            (status, body)
        }));
    }

    let mut codes = Vec::new();
    for handle in handles {
        let (status, body) = handle.await.unwrap();
        assert_eq!(status, StatusCode::OK, "建计划失败: {body}");
        codes.push(
            body["data"]["taskCode"]
                .as_str()
                .expect("响应缺少 taskCode")
                .to_string(),
        );
    }
    codes.sort();
    codes.dedup();
    assert_eq!(codes.len(), 8, "并发建单出现了重复编号: {codes:?}");
}

/// 唯一键冲突按 400 回，且**不能**把 SQLite 原文甩给前端。
///
/// 以前 `From<sqlx::Error>` 直接把 `error.to_string()` 塞进 `AppError::Internal`，
/// `IntoResponse` 又原样当 `msg` 返回 —— 浏览器能看到
/// `UNIQUE constraint failed: parks.park_code`（等于泄露库结构）。
/// 现在唯一键冲突是可预期的并发结果，给一句能指导操作的话。
#[tokio::test]
async fn duplicate_code_reports_a_readable_error_instead_of_sql_text() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let body = r#"{"parkName":"重复编号园区","parkCode":"PK-DUP-1"}"#;
    let create = |body: &'static str| {
        let app = app.clone();
        let token = token.clone();
        async move {
            let response = app
                .oneshot(
                    Request::post("/api/parks")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            (response.status(), json_body(response).await)
        }
    };

    let (status, created) = create(body).await;
    assert_eq!(status, StatusCode::OK, "首次创建应成功: {created}");

    let (status, duplicated) = create(r#"{"parkName":"另一个园区","parkCode":"PK-DUP-1"}"#).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{duplicated}");
    let message = duplicated["msg"].as_str().unwrap_or_default();
    assert!(
        !message.contains("UNIQUE") && !message.contains("constraint"),
        "响应里不该出现 SQLite 原文: {message}"
    );
    assert!(message.contains("编号"), "应说明是编号占用: {message}");
}

/// 中台发布地图后，业务库的状态要跟着走（园区基线徽章一起收敛）。
///
/// 2026-11-24 现场：中台点了「发布」，中台 `dog_map.map_status` 已是 `已发布`，
/// 业务平台还一直显示入库时的「部分标定」，园区基线徽章也是部分标定。
#[tokio::test]
async fn map_status_syncs_from_zentai() {
    let (state, _db) = test_state().await;
    let pool = state.db.clone();
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, device_code, map_name, version, status, created_at, source,
             source_id, source_ref)
         VALUES ('ROS-MAP-1795402650069@v1.0', 1, 'RD-002', '2026_11_23_10_49_02', 'v1.0',
            '部分标定', '2026-01-01 00:00:00', 'robot-mapping', '1795402650069',
            '/ota/alg_data/map/history_map/2026_11_23_10_49_02')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // 业务库自己归档的历史图：中台还标着已发布，同步不能把它复活
    sqlx::query(
        "INSERT INTO map_versions
            (id, park_id, device_code, map_name, version, status, created_at, source,
             source_id, source_ref)
         VALUES ('ROS-MAP-1794473226125@v1.0', 1, 'RD-002', '2026_11_10_16_15_06', 'v1.0',
            '已归档', '2026-01-01 00:00:00', 'robot-mapping', '1794473226125',
            '/ota/alg_data/map/history_map/2026_11_10_16_15_06')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE parks SET map_version_id = 'ROS-MAP-1795402650069@v1.0', map_status = '部分标定'
         WHERE id = 1",
    )
    .execute(&pool)
    .await
    .unwrap();

    // 假中台库：同一张图已经发布
    let zentai_path = state.config.backend_db_path.clone();
    let zentai = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&zentai_path)
                .create_if_missing(true),
        )
        .await
        .expect("创建假中台库失败");
    sqlx::query(
        "CREATE TABLE dog_map (
            id INTEGER PRIMARY KEY,
            map_status TEXT,
            is_deleted INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(&zentai)
    .await
    .unwrap();
    sqlx::query("INSERT INTO dog_map (id, map_status) VALUES (1795402650069, '已发布')")
        .execute(&zentai)
        .await
        .unwrap();
    sqlx::query("INSERT INTO dog_map (id, map_status) VALUES (1794473226125, '已发布')")
        .execute(&zentai)
        .await
        .unwrap();
    zentai.close().await;

    let changed = robodog_ops_server::handlers::maps::sync_map_statuses(&state)
        .await
        .expect("状态同步不应失败");
    assert_eq!(changed, 1, "应同步 1 张地图");

    let status: String = sqlx::query_scalar(
        "SELECT status FROM map_versions WHERE id = 'ROS-MAP-1795402650069@v1.0'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "已发布", "中台发布后业务库应显示已发布");

    let park_status: String = sqlx::query_scalar("SELECT map_status FROM parks WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(park_status, "已标定", "园区基线徽章应跟着变成已标定");

    let archived: String = sqlx::query_scalar(
        "SELECT status FROM map_versions WHERE id = 'ROS-MAP-1794473226125@v1.0'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(archived, "已归档", "业务侧归档的图不许被中台状态复活");

    // 幂等：再同步一次不该再改动任何行
    let again = robodog_ops_server::handlers::maps::sync_map_statuses(&state)
        .await
        .expect("重复同步不应失败");
    assert_eq!(again, 0);
}

/// `POST /api/auth/refresh`：拿当前令牌换一枚新令牌（滑动续期）。
///
/// 业务平台的 JWT 是绝对过期的（默认 24h），没有续期手段时操作员干着活就会因为
/// 某个接口 401 被踢回登录页。这里钉住三件事：能换到新令牌、新令牌可用、
/// 没带令牌时不能凭空续期（401）。
#[tokio::test]
async fn refresh_rotates_the_session_token() {
    let (state, _db) = test_state().await;
    let app = build_router(state);
    let token = login(&app).await;

    let response = app
        .clone()
        .oneshot(
            Request::post("/api/auth/refresh")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "有效令牌应能续期");
    let body = json_body(response).await;
    let fresh = body["data"]["token"]
        .as_str()
        .expect("续期响应缺少 token")
        .to_string();
    assert_eq!(body["data"]["user"]["username"], "admin");

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {fresh}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "续期后的令牌要能直接用");

    let response = app
        .oneshot(
            Request::post("/api/auth/refresh")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "没带令牌不能续期"
    );
}

/// 配了 https 的 `public_url` 后：明文入口上的**页面**请求 302 到 https，
/// 而 `/api/*`（设备客户端 / 板端 SDK 走的就是这条）一律不动。
///
/// 同一份 localStorage 存不下两个源的登录态：`http://ip:3008` 与 `https://ip:3443`
/// 各存一份，操作员从企业微信告警链接过去就像被自动退出（2026-11-26 现场反馈）。
#[tokio::test]
async fn plain_http_pages_redirect_to_https_but_api_does_not() {
    let (state, _db) = test_state().await;
    let mut config = (*state.config).clone();
    config.public_url = Some("https://ops.example.com:3443".to_string());
    let state = AppState {
        config: std::sync::Arc::new(config),
        ..state
    };
    let app = build_router(state.clone()).layer(axum::middleware::from_fn_with_state(
        state.clone(),
        robodog_ops_server::app::redirect_pages_to_https,
    ));

    // 浏览器打开页面：302，路径与查询串原样带过去
    let response = app
        .clone()
        .oneshot(
            Request::get("/devices?q=RD-1")
                .header(header::ACCEPT, "text/html,application/xhtml+xml")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        response.headers()[header::LOCATION],
        "https://ops.example.com:3443/devices?q=RD-1"
    );

    // 接口请求哪怕带了 HTML Accept 也不能重定向
    let response = app
        .oneshot(
            Request::get("/api/health")
                .header(header::ACCEPT, "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "API 不该被重定向到 https"
    );
}

/// 没配 https 的 `public_url` 时不做任何重定向（否则整个控制台会跳到连不上的地址，
/// 比如只在明文上部署、或者 https 起不来的时候）。
#[tokio::test]
async fn pages_are_not_redirected_without_an_https_public_url() {
    let (state, _db) = test_state().await;
    let app = build_router(state.clone()).layer(axum::middleware::from_fn_with_state(
        state.clone(),
        robodog_ops_server::app::redirect_pages_to_https,
    ));

    let response = app
        .oneshot(
            Request::get("/")
                .header(header::ACCEPT, "text/html")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::FOUND);
}
