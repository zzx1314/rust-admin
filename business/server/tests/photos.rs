//! 航点拍照回传 / 查看接口契约。
//!
//! 覆盖：
//! - 板端 JPEG 回传（`POST /api/photos/ingest`）→ 落盘 + 一条 `task_photos`；
//! - 同一张（sha256 相同）重传不新增行（弱网重传是常态）；
//! - 任务详情列表（`GET /api/tasks/{id}/photos`）；
//! - 原图（`GET /api/photos/{id}/file`，`?download=1` 走附件）；
//! - 设备不是这条任务的执行设备时拒收；
//! - 非 JPEG 内容拒收（客户端不可信，按文件头判）；
//! - 删除（行 + 文件一起收）。

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

struct TempDir {
    dir: PathBuf,
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_dir() -> TempDir {
    let dir = std::env::temp_dir().join(format!(
        "robodog-photos-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    TempDir { dir }
}

async fn test_state() -> (AppState, TempDir) {
    let temp = temp_dir();
    let dir = temp.dir.join("db");
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    let config = Arc::new(Config {
        port: 0,
        database_path: dir.join("test.db"),
        jwt_secret: "test-secret-at-least-32-bytes-long!!".to_string(),
        jwt_ttl_hours: 1,
        web_dist: temp.dir.join("web-dist"),
        backend_db_path: temp.dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: temp.dir.join("map_ingest"),
        recording_dir: temp.dir.join("recordings"),
        recording_keep_days: 30,
        photo_dir: temp.dir.join("photos"),
        photo_keep_days: 90,
        service_token: Some("test-service-token".to_string()),
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
        ai_patrol: Default::default(),
        caches: Default::default(),
    };
    (state, temp)
}

/// 插一条设备 + 一条绑定的巡检任务，返回 `(task_id, task_code)`。
async fn insert_task(state: &AppState, device_code: &str) -> (i64, String) {
    sqlx::query(
        "INSERT INTO devices (device_id, status, battery, signal, temperature, runtime_minutes,
            odometer, created_at, updated_at)
         VALUES (?1, '在线', 90, 60, 32, 0, 0, '2026-11-24 09:00:00', '2026-11-24 09:00:00')",
    )
    .bind(device_code)
    .execute(&state.db)
    .await
    .unwrap();
    let device_pk: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE device_id = ?1")
        .bind(device_code)
        .fetch_one(&state.db)
        .await
        .unwrap();
    let task_code = "TK-PHOTO-1".to_string();
    sqlx::query(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, device_id, task_status,
            progress, created_at, updated_at)
         VALUES (?1, '拍照回传测试', '定时巡检', '周期任务', ?2, '执行中', 10,
            '2026-11-24 09:00:00', '2026-11-24 09:00:00')",
    )
    .bind(&task_code)
    .bind(device_pk)
    .execute(&state.db)
    .await
    .unwrap();
    let task_id: i64 = sqlx::query_scalar("SELECT id FROM tasks WHERE task_code = ?1")
        .bind(&task_code)
        .fetch_one(&state.db)
        .await
        .unwrap();
    (task_id, task_code)
}

async fn login(app: &axum::Router) -> String {
    let request = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"username":"admin","password":"admin123"}"#))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    body["data"]["token"]
        .as_str()
        .expect("缺少 token")
        .to_string()
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("响应不是 JSON")
}

/// 构造 multipart：`file`（jpg）+ `meta`（JSON）
fn multipart(file_name: &str, data: &[u8], meta: &Value) -> (String, Vec<u8>) {
    let boundary = "----robodog-photo-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{file_name}\"\r\nContent-Type: image/jpeg\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"meta\"\r\n\r\n{}\r\n",
            meta
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

/// 一个「像 JPEG」的最小字节串：`FF D8 FF` 开头（handler 只认这个头）
fn fake_jpeg(marker: u8) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0];
    bytes.extend_from_slice(&[0u8; 64]);
    bytes.push(marker);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

async fn ingest(
    app: &axum::Router,
    token: &str,
    file_name: &str,
    data: &[u8],
    meta: &Value,
) -> axum::response::Response {
    let (content_type, body) = multipart(file_name, data, meta);
    let request = Request::post("/api/photos/ingest")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

fn meta(task_code: &str, device: &str, point: i64, camera: &str) -> Value {
    serde_json::json!({
        "taskCode": task_code,
        "deviceCode": device,
        "planId": task_code,
        "pointIndex": point,
        "pointName": if point >= 0 { format!("P{}", point + 1) } else { String::new() },
        "camera": camera,
        "capturedAt": "2026-11-24 10:12:00",
    })
}

#[tokio::test]
async fn photo_round_trip_dedupes_and_serves_the_jpeg() {
    let (state, _temp) = test_state().await;
    let (task_id, task_code) = insert_task(&state, "DOG-PHOTO-1").await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    // 1) 回传一张：落盘 + 一行
    let response = ingest(
        &app,
        &token,
        "pt0-dogcam.jpg",
        &fake_jpeg(1),
        &meta(&task_code, "DOG-PHOTO-1", 0, "dogcam"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "回传应成功");
    let body = json_body(response).await;
    assert_eq!(body["data"]["created"], true);
    let photo_id = body["data"]["photo"]["id"].as_i64().expect("缺少 id");
    assert_eq!(body["data"]["photo"]["camera"], "dogcam");
    assert_eq!(body["data"]["photo"]["pointIndex"], 0);

    // 2) 同一张重传：sha256 命中，不新增行
    let response = ingest(
        &app,
        &token,
        "pt0-dogcam.jpg",
        &fake_jpeg(1),
        &meta(&task_code, "DOG-PHOTO-1", 0, "dogcam"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"]["created"], false, "重传不该新增记录");

    // 3) 另一个航点（内容不同）：新行
    let response = ingest(
        &app,
        &token,
        "pt1-cam2.jpg",
        &fake_jpeg(2),
        &meta(&task_code, "DOG-PHOTO-1", 1, "cam2"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // 4) 任务详情列表：两张，按航点升序
    let request = Request::get(format!("/api/tasks/{task_id}/photos"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let rows = body["data"].as_array().expect("列表应为数组");
    assert_eq!(rows.len(), 2, "{body}");
    assert_eq!(rows[0]["pointIndex"], 0, "按航点升序：{body}");
    assert_eq!(rows[1]["camera"], "cam2");
    assert!(
        rows[0].get("filePath").is_none(),
        "接口不能把服务器绝对目录发给前端：{body}"
    );

    // 5) 原图：image/jpeg + 字节原样
    let request = Request::get(format!("/api/photos/{photo_id}/file"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.len(), fake_jpeg(1).len());

    // 6) `?download=1`：附件下载
    let request = Request::get(format!("/api/photos/{photo_id}/file?download=1"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap()
            .starts_with("attachment"),
        "download=1 要按附件给：{:?}",
        response.headers()[header::CONTENT_DISPOSITION]
    );

    // 7) 删除：行 + 文件一起收
    let request = Request::delete(format!("/api/photos/{photo_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_photos")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 1, "只删掉一张");
}

#[tokio::test]
async fn photo_ingest_rejects_other_devices_and_non_jpeg() {
    let (state, _temp) = test_state().await;
    let (_, task_code) = insert_task(&state, "DOG-PHOTO-1").await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    // 别的设备往这条任务里塞图：拒收（否则任何在线设备都能污染别人的任务照片）
    let response = ingest(
        &app,
        &token,
        "pt0.jpg",
        &fake_jpeg(3),
        &meta(&task_code, "DOG-OTHER", 0, "dogcam"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 文件头不是 JPEG（客户端不可信）
    let response = ingest(
        &app,
        &token,
        "pt0.jpg",
        b"not a jpeg at all",
        &meta(&task_code, "DOG-PHOTO-1", 0, "dogcam"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 扩展名不是 jpg：拒收（板端只会写 jpg）
    let response = ingest(
        &app,
        &token,
        "pt0.png",
        &fake_jpeg(5),
        &meta(&task_code, "DOG-PHOTO-1", 0, "dogcam"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 未知任务号
    let response = ingest(
        &app,
        &token,
        "pt0.jpg",
        &fake_jpeg(4),
        &meta("TK-NOT-EXIST", "DOG-PHOTO-1", 0, "dogcam"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_photos")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "被拒的请求不该留下记录");
    // 失败的临时文件也不该留在 .incoming 里
    let incoming = state.config.photo_dir.join(".incoming");
    let leftovers = std::fs::read_dir(&incoming)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "失败的临时文件要收掉");
}

#[tokio::test]
async fn expired_photos_are_swept_with_their_rows() {
    let (state, _temp) = test_state().await;
    let (_, task_code) = insert_task(&state, "DOG-PHOTO-1").await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    let response = ingest(
        &app,
        &token,
        "pt0.jpg",
        &fake_jpeg(7),
        &meta(&task_code, "DOG-PHOTO-1", 0, "dogcam"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // 把 created_at 改到 200 天前：90 天留存应当把它连同文件一起收掉
    sqlx::query("UPDATE task_photos SET created_at = '2025-01-01 00:00:00'")
        .execute(&state.db)
        .await
        .unwrap();
    let removed = robodog_ops_server::photos::sweep_expired(&state)
        .await
        .expect("清理不该失败");
    assert_eq!(removed, 1);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_photos")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
