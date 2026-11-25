//! 航点净空的黑盒回归：终点贴障碍时校验要报、发布要拦。
//!
//! 背景：`endTaskMode = none`（原地待命）时**最后一个航点就是停车位姿**（本体
//! `end_task = 0` 停在最后一点），贴墙 0.3 m 就永远进不去（`13377 无有效绕障轨迹`），
//! 5009 停在「前往 P{n}」、任务不终态。判据与实现见
//! `business/docs/waypoint-clearance.md` 与 `src/handlers/maps/clearance.rs`。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::{db, gateway, seed};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 地图几何：40×40 格、0.05 m/px、原点 (-1, -1)
const MAP_CELLS: u32 = 40;
/// 障碍墙所在的列（地图 x = 0.0）
const WALL_COL: u32 = 20;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> TempDir {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-clearance-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    TempDir(dir)
}

/// 写一张「按矩形补丁上色」的 PGM 地图包，返回给 `map_versions.file_path` 用的相对路径。
/// 基底 = 254（可通行），`patches` 是 `(col0, row0, col1, row1, value)`（左闭右开，row 从顶部数）。
fn write_patched_map(
    backend_db: &Path,
    name: &str,
    cols: u32,
    rows: u32,
    patches: &[(u32, u32, u32, u32, u8)],
) -> String {
    let relative = format!("maps/{name}.zip");
    let path = backend_db
        .parent()
        .expect("backend.db 应有父目录")
        .join("device_uploads")
        .join(&relative);
    std::fs::create_dir_all(path.parent().unwrap()).expect("创建地图目录失败");

    let mut pixels = vec![254_u8; (cols * rows) as usize];
    for (col0, row0, col1, row1, value) in patches {
        for row in *row0..(*row1).min(rows) {
            for col in *col0..(*col1).min(cols) {
                pixels[(row * cols + col) as usize] = *value;
            }
        }
    }
    let mut pgm = format!("P5\n{cols} {rows}\n255\n").into_bytes();
    pgm.extend_from_slice(&pixels);

    let file = std::fs::File::create(&path).expect("创建地图包失败");
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file("map.pgm", zip::write::SimpleFileOptions::default())
        .expect("写入 zip 失败");
    zip.write_all(&pgm).expect("写入 PGM 失败");
    zip.finish().expect("收尾 zip 失败");
    relative
}

/// 写一张「一堵墙」的 PGM 地图包，返回给 `map_versions.file_path` 用的相对路径
fn write_map_package(backend_db: &Path) -> String {
    write_patched_map(
        backend_db,
        "clearance-test",
        MAP_CELLS,
        MAP_CELLS,
        &[(WALL_COL, 0, WALL_COL + 1, MAP_CELLS, 0)],
    )
}

async fn test_state() -> (AppState, TempDir) {
    let dir = temp_dir();
    let config = Arc::new(Config {
        port: 0,
        database_path: dir.0.join("test.db"),
        jwt_secret: "test-secret-at-least-32-bytes-long!!".to_string(),
        jwt_ttl_hours: 1,
        https_port: 0,
        tls_cert: None,
        tls_key: None,
        web_dist: dir.0.join("web-dist"),
        backend_db_path: dir.0.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: dir.0.join("map_ingest"),
        recording_dir: dir.0.join("recordings"),
        photo_dir: dir.0.join("photos"),
        recording_keep_days: 30,
        photo_keep_days: 90,
        service_token: None,
        zentai_ws_url: None,
        device_timeout_secs: 90,
        task_timeout_secs: 120,
        task_stuck_secs: 600,
        backpack_base_url: None,
        backpack_webrtc_url: None,
        backpack_browser_url: None,
        mediamtx_url: None,
        backpack_tunnel_device: None,
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
    (state, dir)
}

async fn insert_map(
    pool: &sqlx::SqlitePool,
    map_id: &str,
    file_path: &str,
    cols: u32,
    rows: u32,
) -> String {
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, map_name, version, frame_id, crs, origin,
            resolution, status, map_width, map_height, file_path, source, created_at)
         VALUES (?1, 1, 'CLEARANCE-MAP', 'v1.0', 'clearance_map', 'ROS map_server',
            '-1.0, -1.0, 0.0', '0.05 m/px', '已发布', ?2, ?3, ?4, 'backend',
            '2026-11-01 00:00:00')",
    )
    .bind(map_id)
    .bind(i64::from(cols))
    .bind(i64::from(rows))
    .bind(file_path)
    .execute(pool)
    .await
    .unwrap();
    map_id.to_string()
}

async fn insert_real_map(pool: &sqlx::SqlitePool, file_path: &str) -> String {
    insert_map(pool, "CLEARANCE-MAP@v1.0", file_path, MAP_CELLS, MAP_CELLS).await
}

/// 两点路线：起点离墙 0.5 m，终点离墙 `last_gap_m`（米）
async fn insert_route(pool: &sqlx::SqlitePool, code: &str, map_id: &str, last_gap_m: f64) -> i64 {
    // 墙在 x = 0.0：终点放在 x = -last_gap_m
    insert_route_points(pool, code, map_id, (-0.5, 0.0), (-last_gap_m, 0.0)).await
}

/// 两点路线（起终点任意），用于航段穿越回归
async fn insert_route_points(
    pool: &sqlx::SqlitePool,
    code: &str,
    map_id: &str,
    from: (f64, f64),
    to: (f64, f64),
) -> i64 {
    let data = serde_json::json!({
        "schemaVersion": 3,
        "coordFrame": "map",
        "loop": "往返",
        "points": [
            {"name": "P1", "x": from.0, "y": from.1, "dwell": 0, "radius": 0.3, "must": true,
             "heading": 0, "onFail": "跳过继续", "actions": []},
            {"name": "P2", "x": to.0, "y": to.1, "dwell": 5, "radius": 0.3, "must": true,
             "heading": 90, "onFail": "跳过继续", "actions": []}
        ],
        "legs": [{"mode": "沿道路", "speed": 1.2, "avoid": "标准", "actions": []}],
        "returnLeg": null
    });
    sqlx::query(
        "INSERT INTO routes (route_code, route_name, park_id, status, version, loop_mode,
            default_speed, route_data, point_count, distance_m, duration_s, battery_pct,
            map_version_id, created_at, updated_at, created_by)
         VALUES (?1, ?2, 1, '草稿', 'v1.0', '往返', 1.2, ?3, 2, 0.6, 12, 1, ?4,
            '2026-11-01 00:00:00', '2026-11-01 00:00:00', 1)",
    )
    .bind(code)
    .bind(format!("{code} 净空路线"))
    .bind(data.to_string())
    .bind(map_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
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
    json_body(response).await["data"]["token"]
        .as_str()
        .expect("登录响应缺少 token")
        .to_string()
}

async fn validate(app: &axum::Router, token: &str, route_id: i64) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/routes/{route_id}/validate"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await["data"].clone()
}

async fn publish(app: &axum::Router, token: &str, route_id: i64) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/routes/{route_id}/publish"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, json_body(response).await)
}

fn item<'a>(validation: &'a Value, title: &str) -> &'a Value {
    validation["items"]
        .as_array()
        .expect("校验结果应有 items")
        .iter()
        .find(|item| item["title"] == title)
        .unwrap_or_else(|| panic!("校验结果缺少「{title}」一项: {validation}"))
}

/// 终点离墙 0.1 m：「原地待命」的停车位进不去 → 校验要报、发布要拦
#[tokio::test]
async fn tight_parking_spot_fails_validation_and_publish() {
    let (state, _dir) = test_state().await;
    let pool = state.db.clone();
    let file_path = write_map_package(&state.config.backend_db_path);
    let map_id = insert_real_map(&pool, &file_path).await;
    let route_id = insert_route(&pool, "RT-TIGHT", &map_id, 0.1).await;

    let app = build_router(state);
    let token = login(&app).await;

    let validation = validate(&app, &token, route_id).await;
    let stop = item(&validation, "终点停靠净空");
    assert_eq!(
        stop["ok"], false,
        "终点离墙 0.1 m 必须体检不通过: {validation}"
    );
    let description = stop["description"].as_str().unwrap_or_default();
    assert!(
        description.contains("13377"),
        "文案要给出本体侧的现场现象: {description}"
    );
    assert!(
        description.contains("原地待命"),
        "文案要说明「最后一点就是停车位」: {description}"
    );

    let (status, body) = publish(&app, &token, route_id).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "贴障碍的终点不能发布: {body}"
    );
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("障碍"),
        "拒绝原因要说清楚是落在障碍上: {message}"
    );
}

/// 终点离墙 0.5 m：刚好达到现场实测的净空建议值 → 允许发布
#[tokio::test]
async fn roomy_parking_spot_passes_and_publishes() {
    let (state, _dir) = test_state().await;
    let pool = state.db.clone();
    let file_path = write_map_package(&state.config.backend_db_path);
    let map_id = insert_real_map(&pool, &file_path).await;
    let route_id = insert_route(&pool, "RT-ROOMY", &map_id, 0.5).await;

    let app = build_router(state);
    let token = login(&app).await;

    let validation = validate(&app, &token, route_id).await;
    assert_eq!(
        item(&validation, "终点停靠净空")["ok"],
        true,
        "离墙 0.5 m 应该够原地停靠: {validation}"
    );
    assert_eq!(item(&validation, "航点净空")["ok"], true);

    let (status, body) = publish(&app, &token, route_id).await;
    assert_eq!(status, StatusCode::OK, "净空达标的路线要能发布: {body}");
}

/// 没有地图包（演示基线）时不能因为「算不出净空」而拦发布 / 判校验失败
#[tokio::test]
async fn missing_map_package_skips_clearance_instead_of_failing() {
    let (state, _dir) = test_state().await;
    let pool = state.db.clone();
    // 真实几何 + 不存在的 zip
    let map_id = insert_real_map(&pool, "maps/does-not-exist.zip").await;
    let route_id = insert_route(&pool, "RT-NO-ZIP", &map_id, 0.1).await;

    let app = build_router(state);
    let token = login(&app).await;

    let validation = validate(&app, &token, route_id).await;
    let titles: Vec<&str> = validation["items"]
        .as_array()
        .expect("校验结果应有 items")
        .iter()
        .filter_map(|item| item["title"].as_str())
        .collect();
    assert!(
        !titles.contains(&"终点停靠净空"),
        "解不出栅格时不该出现净空项（否则没地图包就永远校验不过）: {titles:?}"
    );

    let (status, body) = publish(&app, &token, route_id).await;
    assert_eq!(status, StatusCode::OK, "净空算不出来时不能拦发布: {body}");
}

/// 2026-11-24 广汇二期 `TK-1124-17` 的缩影：
/// 55 m 直线航段里有一大段落在平台标的「未知 205」区。两个航点各自干净，
/// 平台以前完全看不出来，本体又把这个值当成可通行 → 狗直接开了进去。
#[tokio::test]
async fn long_unknown_leg_blocks_publish_with_actionable_message() {
    let (state, _dir) = test_state().await;
    let pool = state.db.clone();
    // 12 m × 2 m，0.0..7.0 m 是「未知 205」（140 列）
    let file_path = write_patched_map(
        &state.config.backend_db_path,
        "unknown-band",
        240,
        40,
        &[(20, 0, 160, 40, 205)],
    );
    let map_id = insert_map(&pool, "UNKNOWN-MAP@v1.0", &file_path, 240, 40).await;
    let route_id =
        insert_route_points(&pool, "RT-UNKNOWN", &map_id, (-0.8, 0.0), (10.9, 0.0)).await;

    let app = build_router(state);
    let token = login(&app).await;

    let validation = validate(&app, &token, route_id).await;
    let unknown = item(&validation, "航段穿越未知区域");
    assert_eq!(
        unknown["ok"], false,
        "航段穿过未知区必须体检不通过: {validation}"
    );
    let description = unknown["description"].as_str().unwrap_or_default();
    assert!(
        description.contains("205"),
        "文案要点出「205 本体当可通行」这个坑: {description}"
    );
    assert_eq!(
        item(&validation, "航段穿越障碍")["ok"],
        true,
        "205 不是障碍，障碍项不该跟着报错"
    );

    let (status, body) = publish(&app, &token, route_id).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "横穿未知区的航段不能发布: {body}"
    );
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("未知") && message.contains("205"),
        "拒绝原因要能让运维一眼看懂: {message}"
    );
    assert!(message.contains("1→2"), "拒绝原因要指出是哪一段: {message}");
}

/// 短距离路过未知区（< 5 m）：体检项报警，但不拦发布（现场地图大半是 unknown，
/// 一律硬拦会让正常路线全发不出去），只在发布事件里留痕。
#[tokio::test]
async fn short_unknown_leg_is_reported_but_still_publishable() {
    let (state, _dir) = test_state().await;
    let pool = state.db.clone();
    // 0.0..2.0 m 是未知 128（40 列）
    let file_path = write_patched_map(
        &state.config.backend_db_path,
        "unknown-sliver",
        240,
        40,
        &[(20, 0, 60, 40, 128)],
    );
    let map_id = insert_map(&pool, "UNKNOWN-SLIVER@v1.0", &file_path, 240, 40).await;
    let route_id = insert_route_points(&pool, "RT-SLIVER", &map_id, (-0.8, 0.0), (10.9, 0.0)).await;

    let app = build_router(state);
    let token = login(&app).await;

    let validation = validate(&app, &token, route_id).await;
    assert_eq!(
        item(&validation, "航段穿越未知区域")["ok"],
        false,
        "路过未知区就要提醒: {validation}"
    );

    let (status, body) = publish(&app, &token, route_id).await;
    assert_eq!(status, StatusCode::OK, "短距离未知不能拦发布: {body}");

    let note: Option<String> = sqlx::query_scalar(
        "SELECT description FROM task_events WHERE event_type = 'route' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    let note = note.unwrap_or_default();
    assert!(
        note.contains("穿未知"),
        "未到拦截阈值的也要在发布事件里留痕: {note}"
    );
}

/// 航段从障碍物里穿过去（连续 ≥ 0.5 m）：发布直接拒
#[tokio::test]
async fn leg_through_obstacle_blocks_publish() {
    let (state, _dir) = test_state().await;
    let pool = state.db.clone();
    // 2.0..2.6 m 是一堵 0.6 m 厚的墙
    let file_path = write_patched_map(
        &state.config.backend_db_path,
        "thick-wall",
        240,
        40,
        &[(60, 0, 72, 40, 0)],
    );
    let map_id = insert_map(&pool, "THICK-WALL@v1.0", &file_path, 240, 40).await;
    let route_id = insert_route_points(&pool, "RT-WALL", &map_id, (-0.8, 0.0), (10.9, 0.0)).await;

    let app = build_router(state);
    let token = login(&app).await;

    let validation = validate(&app, &token, route_id).await;
    assert_eq!(
        item(&validation, "航段穿越障碍")["ok"],
        false,
        "穿墙航段必须体检不通过: {validation}"
    );

    let (status, body) = publish(&app, &token, route_id).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "穿墙航段不能发布: {body}");
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("障碍"),
        "拒绝原因要说明是穿障碍: {message}"
    );
}
