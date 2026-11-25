//! 板端 Agent 隧道（设备客户端代调）的集成测试。
//!
//! 线上形态：业务平台在公网，背包 Agent（`192.168.x.x:8000`）只在现场内网，
//! 操作员的浏览器也常常不在同一个网段（换 WiFi / 出差 / 走公网入口）。这时
//! `/api/backpack/*` 全走设备客户端那条常驻 WS（`agent_tunnel.rs`）。
//!
//! 这里用真实 TCP + WebSocket 扮演「跑在背包板子上的设备客户端」，锁住四件事：
//! 1. `agent_hello` 之后 `status` 变成 `mode = tunnel`，六路源仍被归一化；
//! 2. `detect/start` 的请求体原样穿过隧道（平台不改写参数）；
//! 3. `agent_event` 能从 `/api/backpack/events` 读到（页面画框 / 刷新状态靠它）；
//! 4. 没有客户端时不能假装成功（`reachable=false` / SSE 503 并说明原因）。

use futures_util::{SinkExt, StreamExt};
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::{db, gateway, seed};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

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
        "robodog-ops-agent-tunnel-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

fn config_with(dir: &std::path::Path) -> Config {
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
        // 正式环境：服务端连不到板端（连地址都没有），浏览器也不在背包网段
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
    }
}

async fn test_state() -> (AppState, TempDb) {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let config = Arc::new(config_with(&dir));
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

async fn spawn_server(state: AppState) -> u16 {
    let router = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定随机端口失败");
    let port = listener.local_addr().expect("取端口失败").port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("测试服务运行失败");
    });
    port
}

async fn login_token(port: u16) -> String {
    let body = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/api/auth/login"))
        .json(&json!({"username": "admin", "password": "admin123"}))
        .send()
        .await
        .expect("登录请求失败")
        .json::<Value>()
        .await
        .expect("登录响应不是 JSON");
    body["data"]["token"]
        .as_str()
        .expect("登录没拿到 token")
        .to_string()
}

/* --------------------- 板端 Agent 的四个只读接口 --------------------- */

const CAMERAS: &str = r#"{
  "cameras": {
    "cam0": {"title": "左前", "stream": "cam0"},
    "cam1": {"title": "右前", "stream": "cam1"},
    "cam2": {"title": "右后", "stream": "cam2"},
    "cam3": {"title": "左后", "stream": "cam3"},
    "dogcam": {"title": "机器狗前置", "stream": "dogcam"}
  },
  "infrared": {"stream": "dog-ipcam"}
}"#;

const DETECT_STATUS: &str = r#"{
  "running": false, "state": "stopped", "task": "combined", "labels": null, "conf": 0.35,
  "cams": [], "fps": 6, "warnings": [], "stats": {}, "grabbers": {}, "error": "",
  "single_cam": true, "allowed_cams": ["cam0", "cam1", "cam2", "cam3", "dogcam"],
  "default_cam": "dogcam", "preview_stream": "preview",
  "osd": {"enabled": false, "running": false, "ready": false, "stream": "preview", "error": ""}
}"#;

/// 假客户端对 Agent 请求的回答（路径 → 载荷）
fn agent_reply(path: &str) -> Value {
    match path {
        "/api/cameras" => serde_json::from_str(CAMERAS).expect("CAMERAS 非法"),
        "/api/relay/status" => {
            json!({"enabled": true, "running": true, "restarts": 0, "last_error": "", "stream": "dogcam"})
        }
        "/api/detect/tasks" => json!({
            "models_dir": "/models", "count": 1, "ready": 1, "note": "",
            "tasks": [{"task": "combined", "name": "综合巡检", "desc": "", "nc": 1,
                       "labels": [{"id": 0, "name": "fire", "zh": "火焰", "color": "#E24B4A", "ignored": false}],
                       "conf": 0.35, "alarm": [], "target": "dogcam", "model_size": 1,
                       "ok": true, "issues": []}]
        }),
        "/api/detect/status" => serde_json::from_str(DETECT_STATUS).expect("DETECT_STATUS 非法"),
        _ => json!({"error": format!("未知接口 {path}")}),
    }
}

type DeviceWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 扮演设备客户端：注册 + 打招呼，然后像真客户端一样应答 `agent_request`
async fn connect_device(port: u16, code: &str) -> DeviceWs {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device"))
        .await
        .expect("设备连不上网关");
    let register = json!({
        "msgType": "register",
        "ts": "2026-11-18 10:00:00",
        "payload": {"deviceId": code, "secret": null, "model": "ZSL-2-Ultra", "firmware": "test-fw"},
    });
    ws.send(Message::Text(register.to_string()))
        .await
        .expect("发送注册帧失败");
    wait_for(&mut ws, "register_ack").await;
    // 板端客户端注册成功后打招呼：平台据此知道「这台设备能代调本机 Agent」
    ws.send(Message::Text(
        json!({
            "msgType": "agent_hello",
            "ts": "2026-11-18 10:00:01",
            "payload": {"base": "http://127.0.0.1:8000"},
        })
        .to_string(),
    ))
    .await
    .expect("发送 agent_hello 失败");
    ws
}

async fn wait_for(ws: &mut DeviceWs, msg_type: &str) -> Value {
    let deadline = Duration::from_secs(5);
    tokio::time::timeout(deadline, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).expect("消息不是合法 JSON");
                    if value["msgType"] == msg_type {
                        return value;
                    }
                }
                Some(Ok(_)) => continue,
                Some(Err(error)) => panic!("{msg_type} 连接异常: {error}"),
                None => panic!("{msg_type} 连接已关闭"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{msg_type} 在 {deadline:?} 内没有收到"))
}

/// 后台应答 N 次 `agent_request`（`status` 一次要问 4 个接口，连发 4 条）
fn serve_in_background(ws: DeviceWs, count: usize) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ws = ws;
        serve_requests(&mut ws, count).await;
    })
}

async fn serve_requests(ws: &mut DeviceWs, count: usize) {
    for _ in 0..count {
        let msg = wait_for(ws, "agent_request").await;
        let request = &msg["payload"];
        let reply = match request["action"].as_str().unwrap_or_default() {
            "get" => {
                let payload = agent_reply(request["path"].as_str().unwrap_or_default());
                json!({"requestId": request["requestId"], "ok": true, "payload": payload})
            }
            // AI 动作：把收到的 body 原样回显，断言平台没有篡改参数
            action => json!({
                "requestId": request["requestId"],
                "ok": true,
                "payload": {"ok": true, "action": action, "echo": request["body"]},
            }),
        };
        ws.send(Message::Text(
            json!({
                "msgType": "agent_result",
                "ts": "2026-11-18 10:00:02",
                "payload": reply,
            })
            .to_string(),
        ))
        .await
        .expect("发送 agent_result 失败");
    }
}

fn get(base: &str, path: &str, token: &str) -> reqwest::RequestBuilder {
    reqwest::Client::new()
        .get(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {token}"))
}

/* --------------------------------- 用例 --------------------------------- */

/// 没有设备客户端时不许假装成功：状态如实报不可达，SSE 直接 503 说原因
#[tokio::test]
async fn without_device_everything_reports_unavailable() {
    let (state, temp) = test_state().await;
    let port = spawn_server(state).await;
    let token = login_token(port).await;
    let base = format!("http://127.0.0.1:{port}");

    let body: Value = get(&base, "/api/backpack/status", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let data = &body["data"];
    assert_eq!(data["configured"], json!(false));
    assert_eq!(data["tunnel"]["available"], json!(false));
    assert!(
        data["agent"]["error"]
            .as_str()
            .unwrap_or_default()
            .contains("ROBODOG_BACKPACK_URL"),
        "未接入时要写清配哪个变量：{}",
        data["agent"]["error"]
    );

    // 直连模式探测失败后页面会带 ?transport=tunnel 重试：没有客户端时仍是不可达
    let forced: Value = get(&base, "/api/backpack/status?transport=tunnel", &token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(forced["data"]["agent"]["reachable"], json!(false));

    let response = get(&base, "/api/backpack/events", &token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);

    drop(temp);
}

/// 客户端上线后：status 走隧道，六路源照样被归一化，模型目录也能读到
#[tokio::test]
async fn status_comes_from_the_device_client() {
    let (state, temp) = test_state().await;
    let port = spawn_server(state).await;
    let token = login_token(port).await;
    let base = format!("http://127.0.0.1:{port}");
    let device = connect_device(port, "RD-TUNNEL-1").await;

    // status 并发问 4 个接口，设备侧逐条应答
    let serving = serve_in_background(device, 4);
    let body: Value = get(&base, "/api/backpack/status", &token)
        .send()
        .await
        .expect("status 请求失败")
        .json()
        .await
        .expect("status 不是 JSON");
    serving.await.expect("假客户端任务失败");
    let data = &body["data"];

    assert_eq!(data["mode"], json!("tunnel"));
    assert_eq!(data["configured"], json!(true));
    assert_eq!(data["agent"]["reachable"], json!(true));
    assert_eq!(data["agent"]["base"], json!("http://127.0.0.1:8000"));
    assert_eq!(data["tunnel"]["available"], json!(true));
    assert_eq!(data["tunnel"]["device"], json!("RD-TUNNEL-1"));

    let sources = data["sources"].as_array().expect("sources 必须是数组");
    let keys: Vec<&str> = sources.iter().map(|s| s["key"].as_str().unwrap()).collect();
    assert_eq!(keys, ["dogcam", "cam0", "cam1", "cam2", "cam3", "infrared"]);
    assert_eq!(sources[5]["ai"], json!(false));
    // 板端约束与模型目录必须一起透传：路线编辑器靠它出场景/类别勾选
    assert_eq!(data["ai"]["single_cam"], json!(true));
    assert_eq!(
        data["detect"]["tasks"]["tasks"][0]["task"],
        json!("combined")
    );
    assert_eq!(data["osd"]["preview_stream"], json!("preview"));

    drop(temp);
}

/// AI 启停：请求体原样穿过隧道（平台不改写场景/类别/阈值/相机）
#[tokio::test]
async fn detect_start_forwards_body_through_the_tunnel() {
    let (state, temp) = test_state().await;
    let port = spawn_server(state).await;
    let token = login_token(port).await;
    let base = format!("http://127.0.0.1:{port}");
    let device = connect_device(port, "RD-TUNNEL-2").await;

    let payload = json!({
        "task": "combined", "labels": ["fire"], "conf": 0.4, "cams": ["cam1"], "fps": 5
    });
    let serving = serve_in_background(device, 1);
    let response = reqwest::Client::new()
        .post(format!("{base}/api/backpack/detect/start"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&payload)
        .send()
        .await
        .expect("detect/start 请求失败");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    serving.await.expect("假客户端任务失败");
    assert_eq!(body["data"]["ok"], json!(true));
    assert_eq!(body["data"]["action"], json!("start"));
    assert_eq!(body["data"]["echo"]["cams"], json!(["cam1"]));
    assert_eq!(body["data"]["echo"]["conf"], json!(0.4));

    drop(temp);
}

/// 检出事件：客户端转发上来的 `detect` 事件要在页面 SSE 里出现
#[tokio::test]
async fn agent_events_reach_the_page_sse() {
    let (state, temp) = test_state().await;
    let port = spawn_server(state).await;
    let token = login_token(port).await;
    let base = format!("http://127.0.0.1:{port}");
    let mut device = connect_device(port, "RD-TUNNEL-3").await;

    // SSE 是长连接：拿到响应头之后设备再推事件，避免「推早了没人订阅」
    let mut response = get(
        &base,
        &format!("/api/backpack/events?token={token}"),
        &token,
    )
    .send()
    .await
    .expect("SSE 请求失败");
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    device
        .send(Message::Text(
            json!({
                "msgType": "agent_event",
                "ts": "2026-11-18 10:00:03",
                "payload": {
                    "type": "detect", "cam": "cam1", "ts": 1, "frame_w": 1280, "frame_h": 720,
                    "task": "combined", "infer_ms": 118,
                    "boxes": [{"label": "fire", "zh": "火焰", "color": "#E24B4A", "conf": 0.8,
                               "x1": 1, "y1": 2, "x2": 3, "y2": 4, "count": 1, "text": "火焰 1处"}],
                    "counts": {"fire": 1}
                },
            })
            .to_string(),
        ))
        .await
        .expect("发送 agent_event 失败");

    let chunk = tokio::time::timeout(Duration::from_secs(5), response.chunk())
        .await
        .expect("5s 内没有收到事件")
        .expect("读取事件失败")
        .expect("事件流已断开");
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains("\"type\":\"detect\""), "事件内容：{text}");
    assert!(text.contains("火焰 1处"), "框的文字要原样转发：{text}");

    drop(temp);
}
