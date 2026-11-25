//! 巡检录像回传 / 回放接口契约。
//!
//! 覆盖：
//! - 板端 mp4 回传（`POST /api/recordings/ingest`）→ 落盘 + 一条 `task_recordings`；
//! - 同一段（sha256 相同）重传不新增行（弱网重传是常态）；
//! - 任务详情列表（`GET /api/tasks/{id}/recordings`）；
//! - Range 播放（`GET /api/recordings/{id}/file`，浏览器边下边播）；
//! - 设备不是这条任务的执行设备时拒收；
//! - 非 mp4 内容拒收（客户端不可信，按文件头判）。

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
        "robodog-recordings-test-{}-{}",
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
        https_port: 0,
        tls_cert: None,
        tls_key: None,
        web_dist: temp.dir.join("web-dist"),
        backend_db_path: temp.dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: temp.dir.join("map_ingest"),
        recording_dir: temp.dir.join("recordings"),
        photo_dir: temp.dir.join("photos"),
        recording_keep_days: 30,
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
        phone_audio: Default::default(),
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
         VALUES (?1, '在线', 90, 60, 32, 0, 0, '2026-11-23 09:00:00', '2026-11-23 09:00:00')",
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
    let task_code = "TK-REC-1".to_string();
    sqlx::query(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, device_id, task_status,
            progress, created_at, updated_at)
         VALUES (?1, '录像回传测试', '定时巡检', '周期任务', ?2, '执行中', 10,
            '2026-11-23 09:00:00', '2026-11-23 09:00:00')",
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

/// 构造 multipart：`file`（mp4）+ `meta`（JSON）
fn multipart(file_name: &str, data: &[u8], meta: &Value) -> (String, Vec<u8>) {
    let boundary = "----robodog-recording-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"{file_name}\"\r\nContent-Type: video/mp4\r\n\r\n"
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

/// 一个「像 mp4」的最小字节串：`ftyp` 在偏移 4（handler 只认这个头）
fn fake_mp4(marker: u8) -> Vec<u8> {
    let mut bytes = vec![0u8, 0, 0, 0x18];
    bytes.extend_from_slice(b"ftypmp42");
    bytes.extend_from_slice(&[0u8; 64]);
    bytes.push(marker);
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
    let request = Request::post("/api/recordings/ingest")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

fn meta(task_code: &str, device: &str, scope: &str, point: i64, duration: f64) -> Value {
    // 老客户端不带 `camera`：板端只可能录默认源，入库按 dogcam 算
    meta_with_camera(task_code, device, scope, point, duration, None)
}

fn meta_with_camera(
    task_code: &str,
    device: &str,
    scope: &str,
    point: i64,
    duration: f64,
    camera: Option<&str>,
) -> Value {
    let mut value = serde_json::json!({
        "taskCode": task_code,
        "deviceCode": device,
        "planId": task_code,
        "pointIndex": point,
        "pointName": if point >= 0 { format!("P{}", point + 1) } else { String::new() },
        "scope": scope,
        "startedAt": "2026-11-23 09:10:00",
        "endedAt": "2026-11-23 09:10:12",
        "durationSec": duration,
    });
    if let Some(camera) = camera {
        value["camera"] = serde_json::json!(camera);
    }
    value
}

#[tokio::test]
async fn recording_round_trip_dedupes_and_streams_ranges() {
    let (state, _temp) = test_state().await;
    let (task_id, task_code) = insert_task(&state, "DOG-REC-1").await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    // 1) 航点录像回传（多选相机里的「左前」这一路）
    let response = ingest(
        &app,
        &token,
        "pt0-cam0-1.mp4",
        &fake_mp4(1),
        &meta_with_camera(&task_code, "DOG-REC-1", "point", 0, 12.5, Some("cam0")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "回传应成功");
    let body = json_body(response).await;
    assert_eq!(body["data"]["created"], true);
    let recording_id = body["data"]["recording"]["id"].as_i64().expect("缺少 id");
    assert_eq!(
        body["data"]["recording"]["camera"], "cam0",
        "相机要入库存下来（同一航点多路录像靠它区分）：{body}"
    );

    // 2) 同一段重传：sha256 命中，不新增行
    let response = ingest(
        &app,
        &token,
        "pt0-cam0-1.mp4",
        &fake_mp4(1),
        &meta_with_camera(&task_code, "DOG-REC-1", "point", 0, 12.5, Some("cam0")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["data"]["created"], false, "重传不该新增记录");

    // 3) 全程录像（另一段内容）：新行
    let response = ingest(
        &app,
        &token,
        "task-1.mp4",
        &fake_mp4(2),
        &meta(&task_code, "DOG-REC-1", "task", -1, 300.0),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // 4) 任务详情列表：两段，新的在前
    let request = Request::get(format!("/api/tasks/{task_id}/recordings"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let rows = body["data"].as_array().expect("列表应为数组");
    assert_eq!(rows.len(), 2, "{body}");
    assert!(
        rows.iter()
            .any(|row| row["scope"] == "task" && row["pointIndex"] == -1),
        "全程录像要出现在任务录像里：{body}"
    );
    // 时长要真落库：客户端 meta 里发的是 `durationSec`（板端边车同名）。
    // 以前 `IngestMeta` 用 camelCase 派生成了 `durationS`，一直匹配不上，
    // 入库恒为 0，任务详情「时长」显示 `—`。
    let point = rows
        .iter()
        .find(|row| row["scope"] == "point")
        .expect("应该有航点录像");
    assert_eq!(point["durationSec"], 12.5, "durationSec 要入库：{body}");
    assert_eq!(point["camera"], "cam0", "列表要带相机：{body}");
    let task = rows
        .iter()
        .find(|row| row["scope"] == "task")
        .expect("应该有全程录像");
    assert_eq!(task["durationSec"], 300.0, "全程录像时长也要入库：{body}");
    assert_eq!(
        task["camera"], "dogcam",
        "老客户端没传 camera 时按默认源入库：{body}"
    );

    // 5) 整段下载
    let request = Request::get(format!("/api/recordings/{recording_id}/file"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "video/mp4",
        "播放器要靠 content-type 认容器"
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.len(), fake_mp4(1).len());

    // 6) Range：浏览器边下边播（206 + Content-Range）
    let request = Request::get(format!("/api/recordings/{recording_id}/file"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::RANGE, "bytes=4-7")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        response.headers()[header::CONTENT_RANGE],
        format!("bytes 4-7/{}", fake_mp4(1).len())
    );
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"ftyp");
}

#[tokio::test]
async fn recording_ingest_rejects_other_devices_and_non_mp4() {
    let (state, _temp) = test_state().await;
    let (_, task_code) = insert_task(&state, "DOG-REC-1").await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    // 别的设备往这条任务里塞视频：拒收（否则任何在线设备都能污染别人的任务录像）
    let response = ingest(
        &app,
        &token,
        "pt0-1.mp4",
        &fake_mp4(3),
        &meta(&task_code, "DOG-OTHER", "point", 0, 5.0),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 文件头不是 mp4（客户端不可信）
    let response = ingest(
        &app,
        &token,
        "pt0-1.mp4",
        b"not an mp4 at all",
        &meta(&task_code, "DOG-REC-1", "point", 0, 5.0),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 未知任务号
    let response = ingest(
        &app,
        &token,
        "pt0-1.mp4",
        &fake_mp4(4),
        &meta("TK-NOT-EXIST", "DOG-REC-1", "point", 0, 5.0),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_recordings")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 0, "被拒的请求不该留下记录");
    // 失败的临时文件也不该留在 .incoming 里
    let incoming = state.config.recording_dir.join(".incoming");
    let leftovers = std::fs::read_dir(&incoming)
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "失败的上传不该留残片");
}

/// 播放令牌：`<video src>` 带不上 Bearer，用 `?pt=` 直接按 Range 拉流，
/// 而且只对「换令牌的那一段」有效（拿 A 段的令牌放 B 段必须 401）。
#[tokio::test]
async fn play_token_streams_without_bearer_and_is_scoped_to_one_recording() {
    let (state, _temp) = test_state().await;
    let (_, task_code) = insert_task(&state, "DOG-REC-1").await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    let body = json_body(
        ingest(
            &app,
            &token,
            "task-1.mp4",
            &fake_mp4(7),
            &meta(&task_code, "DOG-REC-1", "task", -1, 30.0),
        )
        .await,
    )
    .await;
    let first = body["data"]["recording"]["id"].as_i64().expect("缺少 id");
    let body = json_body(
        ingest(
            &app,
            &token,
            "pt0-1.mp4",
            &fake_mp4(8),
            &meta(&task_code, "DOG-REC-1", "point", 0, 15.0),
        )
        .await,
    )
    .await;
    let second = body["data"]["recording"]["id"].as_i64().expect("缺少 id");

    // 换令牌（走 Bearer）
    let request = Request::get(format!("/api/recordings/{first}/play"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let play = body["data"]["token"]
        .as_str()
        .expect("缺少播放令牌")
        .to_string();
    assert!(
        body["data"]["expiresAt"].as_i64().unwrap_or(0) > 0,
        "要回过期时间：{body}"
    );

    // 不带任何请求头，只用 `?pt=`：200 + 正确的 content-type
    let request = Request::get(format!("/api/recordings/{first}/file?pt={play}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "令牌应能直接播");
    assert_eq!(response.headers()[header::CONTENT_TYPE], "video/mp4");
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.len(), fake_mp4(7).len());

    // Range 也要能用（播放器拖进度条）
    let request = Request::get(format!("/api/recordings/{first}/file?pt={play}"))
        .header(header::RANGE, "bytes=4-7")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);

    // 拿第一段的令牌去放第二段：拒
    let request = Request::get(format!("/api/recordings/{second}/file?pt={play}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "令牌不该跨录像通用"
    );

    // 没有令牌也没有 Bearer：拒
    let request = Request::get(format!("/api/recordings/{first}/file"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 改一个字符（签名失效）：拒
    let mut tampered = play.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'a' { 'b' } else { 'a' });
    let request = Request::get(format!("/api/recordings/{first}/file?pt={tampered}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// 计划被删之后录像还在：列表 / 播放 / 删除都不能因为「任务不存在」变 404，
/// 否则这些行是既看不到又删不掉的孤儿（2026-11-24 现场 TK-1124-17）。
#[tokio::test]
async fn orphan_recording_survives_plan_deletion() {
    let (state, _temp) = test_state().await;
    let (task_id, task_code) = insert_task(&state, "DOG-REC-1").await;
    let app = build_router(state.clone());
    let token = login(&app).await;

    let body = json_body(
        ingest(
            &app,
            &token,
            "task-1.mp4",
            &fake_mp4(9),
            &meta(&task_code, "DOG-REC-1", "task", -1, 60.0),
        )
        .await,
    )
    .await;
    let recording_id = body["data"]["recording"]["id"].as_i64().expect("缺少 id");

    // 计划被删（只删 tasks / task_events，录像行留在原地）
    sqlx::query("DELETE FROM tasks WHERE id = ?1")
        .bind(task_id)
        .execute(&state.db)
        .await
        .unwrap();

    let request = Request::get(format!("/api/tasks/{task_id}/recordings"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "孤儿录像要还能列出来");
    let body = json_body(response).await;
    assert_eq!(body["data"].as_array().map(Vec::len), Some(1), "{body}");

    let request = Request::get(format!("/api/recordings/{recording_id}/play"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "孤儿录像要还能播");

    let request = Request::delete(format!("/api/recordings/{recording_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "孤儿录像要还能删");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_recordings")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
