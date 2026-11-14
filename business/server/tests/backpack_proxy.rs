//! 数据采集背包代理集成测试。
//!
//! 背包 Agent 是板端的 FastAPI（没有 CORS），平台只做**同源代理**：
//!
//! ```text
//! 浏览器 ──/api/backpack/*──► 业务平台（backpack.rs）──HTTP/SSE──► 背包 Agent
//! ```
//!
//! 这里用假 Agent 复现四个上游接口，锁住三件现场最容易坏的事：
//! 1. 六路源被归一化成**固定顺序 + 固定字段**（页面不猜键名），且红外 `ai=false`；
//! 2. 未带 Authorization 的 SSE 能靠 `?token=` 通过鉴权（`EventSource` 不能带请求头）；
//! 3. Agent 不可达时返回 503 且文案说明配哪个变量，而不是 500「服务器错误」。

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::{db, gateway, seed};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::TcpListener;
use tower::ServiceExt;

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct TempDb {
    dir: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-backpack-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

fn config_of(dir: &Path, backpack_base_url: Option<String>) -> Config {
    config_with(dir, backpack_base_url, None)
}

fn config_with(
    dir: &Path,
    backpack_base_url: Option<String>,
    backpack_browser_url: Option<String>,
) -> Config {
    Config {
        port: 0,
        database_path: dir.join("test.db"),
        jwt_secret: "test-secret-at-least-32-bytes-long!!".to_string(),
        jwt_ttl_hours: 1,
        web_dist: dir.join("web-dist"),
        backend_db_path: dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: dir.join("map_ingest"),
        service_token: None,
        zentai_ws_url: None,
        device_timeout_secs: 90,
        task_timeout_secs: 120,
        task_stuck_secs: 600,
        backpack_base_url,
        backpack_webrtc_url: None,
        backpack_browser_url,
    }
}

async fn state_with(url: Option<String>, dir: PathBuf) -> AppState {
    state_with_browser(url, None, dir).await
}

async fn state_with_browser(
    url: Option<String>,
    browser: Option<String>,
    dir: PathBuf,
) -> AppState {
    let config = Arc::new(config_with(&dir, url, browser));
    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");
    AppState {
        db: pool,
        config,
        device_connections: gateway::new_device_connections(),
        video_hub: Default::default(),
    }
}

async fn login_token(router: &Router) -> String {
    let request = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"username":"admin","password":"admin123"}"#))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    body["data"]["token"].as_str().unwrap().to_string()
}

/* ------------------------------ 假背包 Agent ------------------------------ */

const CAMERAS: &str = r#"{
  "cameras": {
    "cam0": {"title": "左前", "stream": "cam0"},
    "cam1": {"title": "右前", "stream": "cam1"},
    "cam2": {"title": "右后", "stream": "cam2"},
    "cam3": {"title": "左后", "stream": "cam3"},
    "dogcam": {"title": "机器狗前置", "stream": "dogcam"}
  },
  "infrared": {"rtsp": "rtsp://127.0.0.1:8554/dog-ipcam", "stream": "dog-ipcam"},
  "webrtc": {"host": "127.0.0.1", "port": 8889}
}"#;

const DETECT_STATUS: &str = r#"{
  "running": true, "state": "running", "task": "combined", "labels": null, "conf": 0.35,
  "cams": ["dogcam"], "fps": 6, "warnings": [],
  "stats": {"dogcam": {"frames": 10, "hits": 3, "fps": 5.9, "last_ms": 118, "ts": 1}},
  "grabbers": {"dogcam": {"has_frame": true, "error": ""}},
  "error": "", "single_cam": true,
  "allowed_cams": ["cam0", "cam1", "cam2", "cam3", "dogcam"],
  "default_cam": "dogcam", "preview_stream": "preview",
  "osd": {"enabled": true, "running": true, "ready": true, "stream": "preview", "error": "",
          "cam": "dogcam", "frames": 30, "write_fps": 7.9, "render_ms": 9, "missing_glyphs": []},
  "label_format": {"lang": "zh", "show_count": true, "show_conf": false, "unit": "处", "example": "火焰 3处"}
}"#;

async fn fake_cameras() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], CAMERAS)
}

async fn fake_relay() -> impl IntoResponse {
    Json(json!({"enabled": true, "running": true, "restarts": 0, "last_error": "", "stream": "dogcam"}))
}

async fn fake_tasks() -> impl IntoResponse {
    Json(json!({
        "models_dir": "/root/rknn/models", "count": 1, "ready": 1, "stage": 2, "note": "",
        "tasks": [{"task": "combined", "name": "综合巡检", "desc": "", "nc": 1,
                   "labels": [{"id": 0, "name": "fire", "zh": "火焰", "color": "#E24B4A", "ignored": false}],
                   "conf": 0.35, "alarm": [], "target": "rk3576", "model_size": 1,
                   "ok": true, "issues": []}]
    }))
}

async fn fake_detect_status() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], DETECT_STATUS)
}

async fn fake_start(Json(body): Json<Value>) -> impl IntoResponse {
    // 把收到的 cams 原样回显，断言代理没有篡改请求体
    Json(json!({"ok": true, "echo": body}))
}

async fn fake_events() -> impl IntoResponse {
    const EVENT: &str = "data: {\"type\": \"detect\", \"cam\": \"dogcam\", \"boxes\": [], \"counts\": {}}\n\n";
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        EVENT.to_string(),
    )
}

/// 起一个假 Agent，返回它的端口
async fn spawn_fake_agent() -> u16 {
    let router = Router::new()
        .route("/api/cameras", get(fake_cameras))
        .route("/api/relay/status", get(fake_relay))
        .route("/api/detect/tasks", get(fake_tasks))
        .route("/api/detect/status", get(fake_detect_status))
        .route("/api/detect/start", post(fake_start))
        .route("/api/events", get(fake_events))
        .with_state(());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定假 Agent 失败");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("假 Agent 运行失败");
    });
    port
}

/* --------------------------------- 用例 --------------------------------- */

#[tokio::test]
async fn status_requires_login_and_normalizes_six_sources() {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let port = spawn_fake_agent().await;
    let state = state_with(Some(format!("http://127.0.0.1:{port}")), dir).await;
    let router = build_router(state);
    let token = login_token(&router).await;

    // 未登录必须 401：背包代理暴露的是板端控制面，不能匿名读
    let anonymous = router
        .clone()
        .oneshot(Request::get("/api/backpack/status").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let response = router
        .clone()
        .oneshot(
            Request::get("/api/backpack/status")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let data = &body["data"];

    assert_eq!(data["configured"], json!(true));
    assert_eq!(data["agent"]["reachable"], json!(true));
    // 没有显式配置 webrtc_url 时，由 Agent 地址推导出同主机 :8889（浏览器直连 mediamtx）
    assert_eq!(data["webrtc"]["base"], json!("http://127.0.0.1:8889"));

    let sources = data["sources"].as_array().expect("sources 必须是数组");
    let keys: Vec<&str> = sources.iter().map(|s| s["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["dogcam", "cam0", "cam1", "cam2", "cam3", "infrared"]);
    // 红外不支持 AI，且用 mediamtx 的 path 名
    assert_eq!(sources[5]["ai"], json!(false));
    assert_eq!(sources[5]["stream"], json!("dog-ipcam"));
    assert_eq!(sources[0]["available"], json!(true));

    // 板端约束也一起透传，页面据此禁用多选
    assert_eq!(data["ai"]["single_cam"], json!(true));
    assert_eq!(data["osd"]["ready"], json!(true));
    assert_eq!(data["osd"]["preview_stream"], json!("preview"));
    assert_eq!(data["label_format"]["example"], json!("火焰 3处"));

    drop(temp);
}

#[tokio::test]
async fn events_stream_accepts_token_in_query_string() {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let port = spawn_fake_agent().await;
    let state = state_with(Some(format!("http://127.0.0.1:{port}")), dir).await;
    let router = build_router(state);
    let token = login_token(&router).await;

    // EventSource 无法自定义请求头，只带 ?token=（与视频 WS 端点同一套约定）
    let response = router
        .clone()
        .oneshot(
            Request::get(format!("/api/backpack/events?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream; charset=utf-8"
    );
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("\"type\": \"detect\""), "SSE 内容必须原样透传");

    drop(temp);
}

#[tokio::test]
async fn detect_start_forwards_body_untouched() {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let port = spawn_fake_agent().await;
    let state = state_with(Some(format!("http://127.0.0.1:{port}")), dir).await;
    let router = build_router(state);
    let token = login_token(&router).await;

    let payload = json!({
        "task": "combined", "labels": ["fire"], "conf": 0.4, "cams": ["cam1"], "fps": 5
    });
    let response = router
        .clone()
        .oneshot(
            Request::post("/api/backpack/detect/start")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    // 业务错误也返回 200 + ok=false（前端在面板内就地提示，不弹全局 toast）
    assert_eq!(body["data"]["ok"], json!(true));
    assert_eq!(body["data"]["echo"]["cams"], json!(["cam1"]));
    assert_eq!(body["data"]["echo"]["fps"], json!(5));

    drop(temp);
}

#[tokio::test]
async fn unreachable_agent_returns_503_with_address_hint() {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    // 127.0.0.1:1 一定连不上，模拟「现场网线没插 / 板子没上电」
    let state = state_with(Some("http://127.0.0.1:1".to_string()), dir).await;
    let router = build_router(state);
    let token = login_token(&router).await;

    let response = router
        .oneshot(
            Request::get("/api/backpack/events")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("连不上背包 Agent"),
        "现场要能一眼看出是链路问题，实际：{message}"
    );

    drop(temp);
}

#[tokio::test]
async fn unconfigured_deployment_reports_configured_false() {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let state = state_with(None, dir).await;
    let router = build_router(state);
    let token = login_token(&router).await;

    // 未接入返回 200 + configured=false：页面据此渲染「怎么配」的指引，
    // 而不是一个会闪红的报错（这不算错误，是部署形态差异）。
    let response = router
        .clone()
        .oneshot(
            Request::get("/api/backpack/status")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["data"]["configured"], json!(false));
    assert_eq!(body["data"]["sources"], json!([]));
    assert!(
        body["data"]["agent"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("ROBODOG_BACKPACK_URL"),
        "指引必须写清配哪个变量"
    );

    // 真正做不了的操作仍然 503 并给出配置方法
    let blocked = router
        .oneshot(
            Request::post("/api/backpack/detect/stop")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::SERVICE_UNAVAILABLE);

    drop(temp);
}

/// 直连模式（正式环境）：平台在公网、背包在现场内网，业务服务连不上 Agent。
/// 这时后端只下发「浏览器该往哪里调」，不再尝试聚合，也不许把它当报错。
#[tokio::test]
async fn direct_mode_reports_browser_endpoints_without_calling_upstream() {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    // base_url 故意填一个连不上的地址：直连模式不应该去碰它
    let state = state_with_browser(
        Some("http://127.0.0.1:1".to_string()),
        Some("http://192.168.41.246:8000".to_string()),
        dir,
    )
    .await;
    let router = build_router(state);
    let token = login_token(&router).await;

    let response = router
        .oneshot(
            Request::get("/api/backpack/status")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let data = &body["data"];

    assert_eq!(data["mode"], json!("direct"));
    assert_eq!(data["configured"], json!(true));
    assert_eq!(data["direct"]["apiBase"], json!("http://192.168.41.246:8000"));
    // 浏览器直连的 mediamtx 地址由 apiBase 主机推导，端口 8889
    assert_eq!(
        data["direct"]["webrtcBase"],
        json!("http://192.168.41.246:8889")
    );
    assert_eq!(data["webrtc"]["base"], json!("http://192.168.41.246:8889"));
    // 直连模式下聚合结果为空（页面自己取），但字段结构仍在，前端不必分支判空
    assert_eq!(data["sources"], json!([]));

    drop(temp);
}
