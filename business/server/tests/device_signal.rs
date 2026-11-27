//! 设备 WiFi 信号上报的回归测试。
//!
//! 现场背景：`devices.signal` 曾经是**配置里的假值**（模拟信号 0~4），页面显示的是
//! 拍脑袋的数字，2026-11-11 直接摘掉了这一列。现在信号有了真实来源——板端 Agent
//! 的 `/api/network`（WiFi 的 `nmcli` SIGNAL 优先，没连 WiFi 时用 4G 的 `AT+CSQ` 折算，
//! 都是 0-100），由同机的设备客户端随心跳 `signal` / `signalSource` 上报，
//! 平台「设备管理」按 优/良/中/差 + WiFi / 4G 展示。
//!
//! 这个用例守四条：
//! 1. 心跳带 `signal` + `signalSource` → 落库并能从 `GET /api/devices` 读回；
//! 2. 心跳**不带** `signal`（老客户端 / 没连网）→ 保留库里原值，不刷成 0；
//! 3. 越界值夹到 0~100（nmcli 偶尔给负数 / 100+）；
//! 4. 不认识的来源（写错 / 老客户端只带 `signal`）→ `signal_source` 置空，不瞎标。

use axum::Router;
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

const DEVICE_CODE: &str = "RD-002";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct TempDb {
    dir: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn test_state() -> (AppState, TempDb) {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-signal-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    let temp = TempDb { dir: dir.clone() };

    let config = Arc::new(Config {
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
    (state, temp)
}

async fn spawn_server(state: AppState) -> u16 {
    let router: Router = build_router(state);
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

type DeviceWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_msg(ws: &mut DeviceWs, msg_type: &str) -> Value {
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

async fn register_device(port: u16) -> DeviceWs {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device"))
        .await
        .expect("设备连不上网关");
    ws.send(Message::Text(
        json!({
            "msgType": "register",
            "ts": "2026-01-01 00:00:00",
            "payload": {
                "deviceId": DEVICE_CODE,
                "secret": null,
                "model": "X30-Pro",
                "firmware": "test-fw",
            },
        })
        .to_string(),
    ))
    .await
    .expect("发送注册帧失败");
    let ack = next_msg(&mut ws, "register_ack").await;
    assert_eq!(ack["payload"]["success"], true, "注册应成功: {ack}");
    ws
}

/// 上报一帧心跳（`signal` / `source` 由调用方决定是否带，模拟老客户端不报信号）
async fn heartbeat_with_signal(ws: &mut DeviceWs, signal: Option<i64>, source: Option<&str>) {
    let mut payload = json!({
        "battery": 80,
        "bodyOnline": true,
        "mode": "robot",
        "robotMode": "remote",
    });
    if let Some(signal) = signal {
        payload["signal"] = json!(signal);
    }
    if let Some(source) = source {
        payload["signalSource"] = json!(source);
    }
    ws.send(Message::Text(
        json!({
            "msgType": "heartbeat",
            "ts": "2026-01-01 00:00:00",
            "payload": payload,
        })
        .to_string(),
    ))
    .await
    .expect("发送心跳失败");
    // 心跳回执在处理完之后才发，等到它就能保证库已写完
    next_msg(ws, "heartbeat_ack").await;
}

async fn signal_of(state: &AppState) -> (i64, Option<String>) {
    sqlx::query_as("SELECT signal, signal_source FROM devices WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .fetch_one(&state.db)
        .await
        .expect("设备应在台账里")
}

/// 设备列表接口也要能读出 `signal`（`DEVICE_SELECT` 漏列时这里会 500）
async fn signal_from_api(port: u16) -> Value {
    let client = reqwest::Client::new();
    let login: Value = client
        .post(format!("http://127.0.0.1:{port}/api/auth/login"))
        .json(&json!({ "username": "admin", "password": "admin123" }))
        .send()
        .await
        .expect("登录请求失败")
        .json()
        .await
        .expect("登录响应不是 JSON");
    let token = login["data"]["token"].as_str().expect("缺少 token");
    let body: Value = client
        .get(format!("http://127.0.0.1:{port}/api/devices"))
        .bearer_auth(token)
        .send()
        .await
        .expect("设备列表请求失败")
        .json()
        .await
        .expect("设备列表响应不是 JSON");
    body["data"]
        .as_array()
        .expect("设备列表应是数组")
        .iter()
        .find(|device| device["deviceId"] == DEVICE_CODE)
        .cloned()
        .expect("设备列表里应有该设备")
}

#[tokio::test]
async fn heartbeat_signal_lands_in_the_device_ledger() {
    let (state, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    // WiFi：72 → 良
    heartbeat_with_signal(&mut ws, Some(72), Some("wifi")).await;
    assert_eq!(
        signal_of(&state).await,
        (72, Some("wifi".to_string())),
        "板端 WiFi 信号应随心跳落库"
    );
    let device = signal_from_api(port).await;
    assert_eq!(device["signal"], json!(72), "设备列表应带 signal");
    assert_eq!(device["signalSource"], json!("wifi"), "设备列表应带来源");

    // 未连 WiFi → 板端退 4G：45 → 中
    heartbeat_with_signal(&mut ws, Some(45), Some("cellular")).await;
    assert_eq!(signal_of(&state).await, (45, Some("cellular".to_string())));

    // 没连网 / 老客户端：不带 signal，库里保留上一次的真实值（不刷成 0）
    heartbeat_with_signal(&mut ws, None, None).await;
    assert_eq!(signal_of(&state).await, (45, Some("cellular".to_string())));

    // 老客户端只带 signal 不带来源：来源置空（不让旧标签配新数值）
    heartbeat_with_signal(&mut ws, Some(61), None).await;
    assert_eq!(signal_of(&state).await, (61, None));

    // 不认识的来源不瞎标
    heartbeat_with_signal(&mut ws, Some(70), Some("satellite")).await;
    assert_eq!(signal_of(&state).await, (70, None));

    // 越界值夹到 0~100（nmcli 偶尔给负数或 100+）
    heartbeat_with_signal(&mut ws, Some(130), Some("wifi")).await;
    assert_eq!(signal_of(&state).await, (100, Some("wifi".to_string())));
    heartbeat_with_signal(&mut ws, Some(-3), Some("wifi")).await;
    assert_eq!(signal_of(&state).await, (0, Some("wifi".to_string())));
}
