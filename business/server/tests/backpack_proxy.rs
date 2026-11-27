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

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
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
        service_token: None,
        zentai_ws_url: None,
        device_timeout_secs: 90,
        task_timeout_secs: 120,
        task_stuck_secs: 600,
        backpack_base_url,
        backpack_webrtc_url: None,
        backpack_browser_url,
        // 平台侧 mediamtx 反代（这里不涉及视频，保持未配置）
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
        notifier: robodog_ops_server::notify::Notifier::disabled(),
        config,
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
        phone_audio: Default::default(),
        ai_patrol: Default::default(),
        caches: Default::default(),
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
    Json(
        json!({"enabled": true, "running": true, "restarts": 0, "last_error": "", "stream": "dogcam"}),
    )
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
    const EVENT: &str =
        "data: {\"type\": \"detect\", \"cam\": \"dogcam\", \"boxes\": [], \"counts\": {}}\n\n";
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
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假 Agent 失败");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .await
            .expect("假 Agent 运行失败");
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
        .oneshot(
            Request::get("/api/backpack/status")
                .body(Body::empty())
                .unwrap(),
        )
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
    assert!(
        text.contains("\"type\": \"detect\""),
        "SSE 内容必须原样透传"
    );

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

/// 多设备：按 `?device=` 切设备时，Agent 地址 / 隧道目标 / WHEP 基址都按那台解析。
///
/// 三条红线：
/// 1. 设备编号不合法（会拼进 mediamtx path / 房间名）→ 400；
/// 2. 清单里没配的设备**不报 500**、也不回退到默认设备：`configured=true`（视频
///    仍能看）+ `agent.reachable=false` + 错误里点名这台设备；
/// 3. AI 动作同样拒绝落回默认设备（否则「看 RD-002、AI 控 RD-001」）。
#[tokio::test]
async fn device_query_switches_target_without_falling_back_to_the_default() {
    use robodog_ops_server::config::BackpackDevice;

    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let mut state = state_with(None, dir).await;
    let mut config = (*state.config).clone();
    config.backpack_devices = vec![BackpackDevice {
        id: "RD-002".to_string(),
        label: Some("2号背包".to_string()),
        ..Default::default()
    }];
    state.config = Arc::new(config);
    let router = build_router(state);
    let token = login_token(&router).await;

    let call = |path: String, method: &'static str| {
        let router = router.clone();
        let token = token.clone();
        async move {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap();
            let response = router.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };

    // 清单里配了名字但没配 Agent 地址：视频照样能看，AI 用不了
    let (status, body) = call("/api/backpack/status?device=RD-002".to_string(), "GET").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["configured"], json!(true));
    assert_eq!(body["data"]["device"], json!("RD-002"));
    assert_eq!(body["data"]["deviceLabel"], json!("2号背包"));
    assert_eq!(body["data"]["agent"]["reachable"], json!(false));
    assert_eq!(body["data"]["webrtc"]["base"], Value::Null);

    // 清单里完全没有的设备：也不能回退到默认设备
    let (status, body) = call("/api/backpack/status?device=RD-999".to_string(), "GET").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["device"], json!("RD-999"));
    assert_eq!(body["data"]["mode"], json!("none"));
    let hint = body["data"]["agent"]["error"].as_str().unwrap_or_default();
    assert!(hint.contains("RD-999"), "错误里要点名是哪台设备: {hint}");

    // 设备编号会拼进 URL / 房间名 / mediamtx path，非法值直接 400
    let (status, _) = call("/api/backpack/status?device=a%3Ab".to_string(), "GET").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // AI 动作：没配 Agent 地址 + 隧道不匹配 → 503，且不静默控到默认设备
    let (status, body) = call(
        "/api/backpack/detect/stop?device=RD-002".to_string(),
        "POST",
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("RD-002"),
        "拒绝时要说清是哪台设备：{message}"
    );

    drop(temp);
}

/// 「现场直连」的板端 mediamtx 地址是**部署级固定值**，不按设备配。
///
/// 平台中继那条路（默认）用的是固定同源路径 `/api/backpack/whep/<stream>/whep`，
/// 连这个值都不需要；只有现场入口的「直连板端」才用到它，而且一个部署一个值就够了。
#[tokio::test]
async fn named_device_uses_the_fixed_deployment_webrtc_base() {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    // 部署级：浏览器可达的 Agent 地址 → 推导出同主机 :8889
    let state = state_with_browser(
        None,
        Some("http://192.168.41.246:8000".to_string()),
        dir,
    )
    .await;
    let router = build_router(state);
    let token = login_token(&router).await;

    let response = router
        .oneshot(
            Request::get("/api/backpack/status?device=0202222672400230")
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
    assert_eq!(
        body["data"]["webrtc"]["base"],
        json!("http://192.168.41.246:8889"),
        "没配 per-device webrtc_url 时用部署级固定值"
    );
    // 设备编号没配 Agent 地址 → 不会误用默认设备的直连地址（那会把 AI 起在别的狗上）
    assert_eq!(body["data"]["mode"], json!("none"));
    assert_eq!(body["data"]["agent"]["reachable"], json!(false));

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
    assert_eq!(
        data["direct"]["apiBase"],
        json!("http://192.168.41.246:8000")
    );
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
