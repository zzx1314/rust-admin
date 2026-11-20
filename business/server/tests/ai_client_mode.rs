//! 航点 AI 识别「客户端代调」模式的集成测试。
//!
//! 正式环境的业务平台在公网、背包 Agent 只在内网，服务端直连必然超时；设备客户端
//! 与 Agent 同机（`127.0.0.1:8000`），所以由它本地启停 Agent、把检出回传。这条链路上
//! 服务端只做两件事：用**同一套规则**算好参数下发 `ai_start`，以及把回传的检出落库。
//!
//! 这里用真实 TCP + WebSocket 扮演设备客户端，锁住四件容易回归的事：
//! 1. `ai_point` → `ai_start`：场景/类别/阈值/窗口秒数解析正确（`scene` 精确命中优先，
//!    `param` 关键词兜底），窗口秒数按 `dwell` 收敛到 [10, 30]；
//! 2. `ai_detect` → `alerts` + `task_events`：低于阈值不写，同一类别只写一次；
//! 3. `ai_end` → 窗口态清空 + 任务事件留痕；
//! 4. 任务终态 → 服务端主动下发 `ai_stop`（否则板端 NPU 会一直占着）；
//! 5. `point_ai = server` 时不接管客户端上报（避免与直连模式重复开窗、重复告警）。

use futures_util::{SinkExt, StreamExt};
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::{ai_patrol, db, gateway, seed};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// AI 窗口是进程级单例（Agent 单路推理），用例必须串行。
static TEST_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

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
        "robodog-ops-ai-client-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

fn config_with(dir: &std::path::Path, point_ai: &str) -> Config {
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
        // 正式环境形态：服务端连不到板端，只能靠客户端代调
        backpack_base_url: None,
        backpack_webrtc_url: None,
        backpack_tunnel_device: None,
        backpack_browser_url: Some("http://192.168.41.246:8000".to_string()),
        mediamtx_url: None,
        point_ai: point_ai.to_string(),
        ai_window_max_secs: 300,
        public_url: None,
        wecom_enabled: false,
        wecom_webhook: None,
        wecom_min_level: "警告".to_string(),
        wecom_mention_all: false,
    }
}

/// 板端 `GET /api/detect/tasks` 的最小快照（客户端会把它原样带上来说给服务端）
const TASKS: &str = r#"{
  "models_dir": "/models",
  "count": 2,
  "ready": 2,
  "note": "测试用",
  "tasks": [
    {"task": "fire", "name": "烟火检测", "desc": "火焰与烟雾", "nc": 2,
     "labels": [{"id": 0, "name": "smoke", "zh": "烟雾"}, {"id": 1, "name": "flame", "zh": "火焰"}],
     "conf": 0.4, "alarm": ["flame"], "target": "dogcam", "model_size": 1024, "ok": true, "issues": []},
    {"task": "firextinguisher", "name": "灭火器检测", "desc": "灭火器", "nc": 1,
     "labels": [{"id": 0, "name": "extinguisher", "zh": "灭火器"}],
     "conf": 0.25, "alarm": [], "target": "dogcam", "model_size": 1024, "ok": true, "issues": []}
  ]
}"#;

async fn test_state(point_ai: &str) -> (AppState, TempDb) {
    let (state, temp, _rx) = test_state_with_wecom(point_ai, false).await;
    (state, temp)
}

/// 同上，但可选打开企业微信推送（`true` 时返回入队接收端，只观察消息形状，不碰外网）。
async fn test_state_with_wecom(
    point_ai: &str,
    wecom: bool,
) -> (
    AppState,
    TempDb,
    Option<tokio::sync::mpsc::Receiver<robodog_ops_server::notify::Notice>>,
) {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let mut config = config_with(&dir, point_ai);
    if wecom {
        config.wecom_enabled = true;
        config.wecom_webhook = Some(
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=test-key".to_string(),
        );
    }
    let config = Arc::new(config);
    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");
    let (notifier, rx) = if wecom {
        let (notifier, rx) = robodog_ops_server::notify::Notifier::channel(&config, None);
        (notifier, rx)
    } else {
        (robodog_ops_server::notify::Notifier::disabled(), None)
    };
    let state = AppState {
        db: pool,
        notifier,
        config,
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
    };
    (state, temp, rx)
}

/// 假企业微信群机器人：数一下收到几条消息，`fail_image` 时图片那条回 `93000`。
async fn spawn_fake_wecom(fail_image: bool) -> (u16, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let router = axum::Router::new().route(
        "/send",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::Relaxed);
                if body["msgtype"] == "image" && fail_image {
                    axum::Json(json!({"errcode": 93000, "errmsg": "invalid image"}))
                } else {
                    axum::Json(json!({"errcode": 0, "errmsg": "ok"}))
                }
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假企微失败");
    let port = listener.local_addr().expect("取端口失败").port();
    tokio::spawn(async move { axum::serve(listener, router).await.expect("假企微运行失败") });

    (port, hits)
}

/// webhook 指向本机假企微，并**真的启动消费端**：只有推送真的成功才会有 `ai_cleanup`。
async fn state_with_live_wecom(point_ai: &str, webhook: String) -> (AppState, TempDb) {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let mut config = config_with(&dir, point_ai);
    config.wecom_enabled = true;
    config.wecom_webhook = Some(webhook);
    let config = Arc::new(config);
    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");
    let (notifier, rx) = robodog_ops_server::notify::Notifier::channel(&config, None);
    let state = AppState {
        db: pool,
        notifier,
        config,
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
    };
    robodog_ops_server::notify::spawn(state.clone(), rx.expect("开了企微就该有消费端"));
    (state, temp)
}

/// 起真实监听端口（WebSocket 必须走 TCP）
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

type DeviceWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 扮演设备客户端：连上网关并注册，返回（连接, 本机设备编号）
async fn register_device(port: u16) -> (DeviceWs, String) {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device"))
        .await
        .expect("设备连不上网关");
    let device_code = "RD-AI-1";
    let register = json!({
        "msgType": "register",
        "ts": "2026-11-16 10:00:00",
        "payload": {
            "deviceId": device_code,
            "secret": null,
            "model": "ZSL-2-Ultra",
            "firmware": "test-fw",
        },
    });
    ws.send(Message::Text(register.to_string()))
        .await
        .expect("发送注册帧失败");
    let ack = next_msg(&mut ws, "register_ack").await;
    assert_eq!(ack["payload"]["success"], true, "注册应成功: {ack}");
    (ws, device_code.to_string())
}

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

/// 断言在这段时间里**没有**收到指定类型的消息
async fn assert_no_msg(ws: &mut DeviceWs, msg_type: &str, within: Duration) {
    let result = tokio::time::timeout(within, async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(text))) => {
                    let value: Value = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
                    if value["msgType"] == msg_type {
                        return value;
                    }
                }
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return Value::Null,
            }
        }
    })
    .await;
    if let Ok(value) = result
        && !value.is_null()
    {
        panic!("不该收到 {msg_type}: {value}");
    }
}

/// 插一条「首点带 AI 识别动作」的路线 + 一条引用它的任务
///
/// `check_type` = 任务类型（`exist` / `count`，`None` = 老数据靠 expectCount 推断）；
/// `expect_count` = 动作里的「检查目标数量」（数量任务必填）。
async fn seed_ai_route(
    state: &AppState,
    scene: Option<&str>,
    labels: &[&str],
    check_type: Option<&str>,
    expect_count: Option<i64>,
) -> String {
    let device: (i64, Option<i64>) = sqlx::query_as("SELECT id, park_id FROM devices LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("播种数据里应至少有一台设备");
    let action = json!({
        "type": "AI 识别",
        "param": "灭火器 · 灭火器",
        "scene": scene,
        "labels": labels,
        "trigger": "到达即执行",
        "threshold": "置信度 ≥ 0.35",
        "retry": 1,
        // 任务类型 + 检查数量：路线编辑器写进动作，服务端解析后随 ai_start 下发
        "checkType": check_type,
        "expectCount": expect_count,
    });
    let route_data = json!({
        "schemaVersion": 3,
        "loop": "闭环",
        "coordFrame": "map",
        "points": [{
            "name": "新航点 1",
            "x": 10.8,
            "y": -0.6,
            "dwell": 100,
            "heading": 90,
            "radius": 0.4,
            "must": false,
            "onFail": "跳过继续",
            "actions": [action]
        }],
        "legs": []
    });
    let timestamp = "2026-11-16 10:00:00";
    sqlx::query(
        "INSERT INTO routes (route_code, route_name, park_id, status, loop_mode, default_speed,
            route_data, point_count, created_at, updated_at)
         VALUES ('RT-AI-C', '客户端代调测试路线', ?1, '已发布', '闭环', 1.2, ?2, 1, ?3, ?3)",
    )
    .bind(device.1)
    .bind(route_data.to_string())
    .bind(timestamp)
    .execute(&state.db)
    .await
    .expect("插入路线失败");
    let route_id: i64 = sqlx::query_scalar("SELECT id FROM routes WHERE route_code = 'RT-AI-C'")
        .fetch_one(&state.db)
        .await
        .expect("读路线 id 失败");

    sqlx::query(
        "INSERT INTO tasks (task_code, task_name, task_type, device_id, park_id, route_id,
            task_status, progress, created_at, updated_at)
         VALUES ('TK-AI-C', '客户端代调测试任务', '定时巡检', ?1, ?2, ?3, '执行中', 0, ?4, ?4)",
    )
    .bind(device.0)
    .bind(device.1)
    .bind(route_id)
    .bind(timestamp)
    .execute(&state.db)
    .await
    .expect("插入任务失败");
    "TK-AI-C".to_string()
}

/// 客户端上报的「到点开窗」请求（含板端模型目录，服务端据此解析场景）
fn ai_point(task_code: &str, agent_running: bool) -> Value {
    ai_point_with_dwell(task_code, 100, agent_running)
}

/// 同上，但指定航点停留时长：窗口时长 = `dwell + 8`，「看够了没有」的判据 = `dwell.max(4)`。
fn ai_point_with_dwell(task_code: &str, dwell: i64, agent_running: bool) -> Value {
    json!({
        "msgType": "ai_point",
        "ts": "2026-11-16 10:00:01",
        "payload": {
            "taskCode": task_code,
            "pointIndex": 0,
            "pointName": "新航点 1",
            "dwell": dwell,
            "hint": "灭火器 · 灭火器",
            "scene": "firextinguisher",
            "labels": ["extinguisher"],
            "threshold": "置信度 ≥ 0.35",
            "tasks": serde_json::from_str::<Value>(TASKS).unwrap(),
            "agentRunning": agent_running,
            "agentCam": "dogcam"
        }
    })
}

fn detect(window_id: &str, label: &str, conf: f64) -> Value {
    json!({
        "msgType": "ai_detect",
        "ts": "2026-11-16 10:00:02",
        "payload": { "windowId": window_id, "task": "firextinguisher", "label": label, "conf": conf }
    })
}

/// 最小 JPEG（只看文件头 `FF D8 FF`）：md5 = cbde0fdf7fab6fefd9fe12fec4967cf9
const JPEG_BASE64: &str = "/9j/4AAQSkZJRgAB";
const JPEG_MD5: &str = "cbde0fdf7fab6fefd9fe12fec4967cf9";

/// 带证据图的检出（客户端扫板端存证目录后随 `ai_detect` 带上）
fn detect_with_evidence(window_id: &str, label: &str, conf: f64) -> Value {
    json!({
        "msgType": "ai_detect",
        "ts": "2026-11-16 10:00:02",
        "payload": {
            "windowId": window_id,
            "task": "firextinguisher",
            "label": label,
            "conf": conf,
            "evidence": "/root/261113/backpack_dashboard/backend/detect_jobs/TK-AI-C/pt0/dogcam_1.jpg",
            "evidenceImage": JPEG_BASE64,
        }
    })
}

/// 补发命中帧（客户端在窗口收尾、板端 `_plan_flush` 写出图之后才报的那条）
fn late_evidence(window_id: &str) -> Value {
    json!({
        "msgType": "ai_evidence",
        "ts": "2026-11-18 19:35:58",
        "payload": {
            "windowId": window_id,
            "taskCode": "TK-AI-C",
            "planId": "TK-AI-C",
            "pointIndex": 0,
            "evidence": "/root/261113/backpack_dashboard/backend/detect_jobs/TK-AI-C/pt0/dogcam_20261118_193557.jpg",
            "evidenceImage": JPEG_BASE64,
        }
    })
}

/// 只数 AI 识别写出来的告警（播种数据里还有一批别的告警）
async fn alert_count(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE content LIKE 'AI 识别%'")
        .fetch_one(&state.db)
        .await
        .expect("查告警失败")
}

async fn alert_contents(state: &AppState) -> Vec<String> {
    sqlx::query_scalar("SELECT content FROM alerts WHERE content LIKE 'AI 识别%' ORDER BY id")
        .fetch_all(&state.db)
        .await
        .expect("查告警失败")
}

async fn event_descriptions(state: &AppState) -> Vec<String> {
    sqlx::query_scalar("SELECT description FROM task_events WHERE event_type = 'ai' ORDER BY id")
        .fetch_all(&state.db)
        .await
        .expect("查任务事件失败")
}

/// AI 事件标题（`AI 识别 · 航点「…」`）：文案里不该出现 0 基航点下标。
async fn event_titles(state: &AppState) -> Vec<String> {
    sqlx::query_scalar("SELECT title FROM task_events WHERE event_type = 'ai' ORDER BY id")
        .fetch_all(&state.db)
        .await
        .expect("查任务事件失败")
}

/// 客户端上报 `ai_point` → 服务端按同一套规则解析并下发 `ai_start`，
/// 检出落库（阈值 + 类别去重），`ai_end` 清空窗口态。
#[tokio::test]
async fn client_relayed_point_ai_starts_window_and_records_hits() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    // 1) 到点开窗：场景按编辑器选的模型包精确命中，dwell=100 → 窗口被夹到 30s
    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let start = &start["payload"];
    assert_eq!(start["action"], "start", "板端没在识别时应该 start: {start}");
    assert_eq!(start["task"], "firextinguisher");
    assert_eq!(start["labels"], json!(["extinguisher"]));
    assert!((start["conf"].as_f64().unwrap() - 0.35).abs() < 1e-6, "{start}");
    assert_eq!(start["cam"], "dogcam");
    assert_eq!(start["fps"], 3);
    assert_eq!(
        start["secs"], 108,
        "dwell=100 就该识别 108s（以前被截断成 30s）: {start}"
    );
    assert_eq!(start["wasRunning"], false);
    let window_id = start["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    // 2) 检出：同一类别只写一条告警（客户端已去重，服务端再兜一层）
    ws.send(Message::Text(detect(&window_id, "extinguisher", 0.92).to_string()))
        .await
        .expect("发送检出失败");
    tokio::time::sleep(Duration::from_millis(300)).await;
    ws.send(Message::Text(detect(&window_id, "extinguisher", 0.95).to_string()))
        .await
        .expect("发送重复检出失败");
    // 低于阈值（0.35）：不写告警
    ws.send(Message::Text(detect(&window_id, "extinguisher", 0.10).to_string()))
        .await
        .expect("发送低分检出失败");
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert_eq!(contents.len(), 1, "同一类别只该有一条告警: {contents:?}");
    assert!(
        contents[0].contains("检出 extinguisher") && contents[0].contains("0.92"),
        "告警内容要带任务/航点/类别/置信度: {}",
        contents[0]
    );
    assert_eq!(alert_count(&state).await, 1);

    // 窗口态：`/api/backpack/ai-window` 的数据源
    let status = ai_patrol::global().status().await;
    assert_eq!(status["active"], true, "{status}");
    assert_eq!(status["source"], "client", "{status}");
    assert_eq!(status["pointIndex"], 0);
    assert_eq!(status["scene"], "firextinguisher");

    // 3) 窗口结束：客户端回报后窗口态清空，任务事件留痕
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:00:40",
            "payload": { "windowId": window_id, "reason": "窗口到期（30s）", "hits": 1 }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        ai_patrol::global().status().await["active"],
        false,
        "ai_end 之后窗口态必须清空"
    );
    let events = event_descriptions(&state).await;
    assert!(
        events.iter().any(|text| text.contains("窗口结束")),
        "任务时间线应有窗口结束记录: {events:?}"
    );
}

/// 场景对不上时退回 `param` 关键词匹配（老路线 / 模型包改名），
/// 类别只保留该场景真实存在的。
#[tokio::test]
async fn unknown_scene_falls_back_to_keyword_and_filters_labels() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code = seed_ai_route(
        &state,
        Some("extinguisher"),
        &["extinguisher", "nope"],
        Some("exist"),
        None,
    )
    .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, true).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let start = &start["payload"];
    // 板端在识别（操作员手动开的）：只热更新，别硬停人家的会话
    assert_eq!(start["action"], "config", "{start}");
    assert_eq!(start["wasRunning"], true);
    // `scene=extinguisher` 不在模型列表里 → 关键词「灭火器」命中 firextinguisher
    assert_eq!(start["task"], "firextinguisher", "{start}");
    // 不存在的类别被过滤掉，只留下真实的 extinguisher
    assert_eq!(start["labels"], json!(["extinguisher"]), "{start}");
    ai_patrol::global().stop("用例结束").await;
}

/// 任务终态时必须由服务端主动下发 `ai_stop`：客户端窗口自己到期会停 Agent，
/// 但「任务被取消/提前结束」只有平台知道，不发就会一直占着 NPU。
#[tokio::test]
async fn terminal_task_sends_ai_stop_to_device() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    ws.send(Message::Text(
        json!({
            "msgType": "task_progress",
            "ts": "2026-11-16 10:01:00",
            "payload": {
                "taskCode": task_code,
                "status": "已完成",
                "progress": 100,
                "pointIndex": 0,
                "stage": "patrol.completed",
                "totalPoints": 1
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送终态失败");

    let stop = next_msg(&mut ws, "ai_stop").await;
    assert!(
        stop["payload"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("终态"),
        "ai_stop 要带原因: {stop}"
    );
    // 终态也是「先等客户端说完」：窗口标记收尾中，`ai_end` 到了才真正摘掉
    assert_eq!(
        ai_patrol::global()
            .status()
            .await["closing"]
            .as_str()
            .map(|s| s.contains("终态")),
        Some(true),
        "终态关窗也要进宽限期"
    );
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:01:05",
            "payload": { "windowId": window_id, "reason": "任务结束", "hits": 0, "expired": false }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(ai_patrol::global().status().await["active"], false);
}

/// 狗开始走向下一个点（`stage` 不再是 `patrol.point_*`）时必须立刻关窗：
/// 长停留窗口不再靠「30 s 截断」防误报，改成靠阶段收口，这条是它的回归保护。
#[tokio::test]
async fn leaving_the_point_closes_window_immediately() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"].as_str().unwrap().to_string();

    // 还剩 100 多秒窗口，但狗已经走了：立刻停
    ws.send(Message::Text(
        json!({
            "msgType": "task_progress",
            "ts": "2026-11-16 10:01:00",
            "payload": {
                "taskCode": task_code,
                "status": "执行中",
                "progress": 50,
                "pointIndex": 0,
                "stage": "patrol.navigating",
                "totalPoints": 1
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送导航阶段失败");

    let stop = next_msg(&mut ws, "ai_stop").await;
    assert!(
        stop["payload"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("离开"),
        "ai_stop 要说明是离开航点: {stop}"
    );
    // 窗口已经被平台「下班」了（不再识别），但还留一段宽限期等客户端把本点的结论说完：
    // 这几秒里到的 `ai_detect` / `ai_alert` / `ai_end` 必须还能归到它身上，
    // 否则就会出现 2026-11-20 13:38 那种「板端 found=1、平台记成 0」的假结论。
    let status = ai_patrol::global().status().await;
    assert_eq!(
        status["closing"].as_str().map(|s| s.contains("离开")),
        Some(true),
        "窗口要标成收尾中（{window_id}）: {status}"
    );
    // 客户端回报结束之后窗口才真正摘掉
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:01:05",
            "payload": {
                "windowId": window_id,
                "reason": "已离开航点",
                "hits": 0,
                "expired": false
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        ai_patrol::global().status().await["active"],
        false,
        "客户端回报结束后窗口必须清空（{window_id}）"
    );
}

/// `point_ai = server`（服务端与板端同网段）时不接管客户端上报：
/// 否则两条链路会各开一次窗口、各写一条告警。
#[tokio::test]
async fn server_mode_ignores_client_point_request() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("server").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    assert_no_msg(&mut ws, "ai_start", Duration::from_millis(600)).await;
    assert_eq!(ai_patrol::global().status().await["active"], false);
}

/// `auto` + 没配 `ROBODOG_BACKPACK_URL`（正式环境形态）→ 客户端代调
#[tokio::test]
async fn auto_mode_without_base_url_delegates_to_client() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("auto").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    assert_eq!(start["payload"]["task"], "firextinguisher", "{start}");
    ai_patrol::global().stop("用例结束").await;
}

/// 客户端掉线时窗口不能被永久占住（否则下次到点会被判成「这个点已在识别」而跳过）
#[tokio::test]
async fn window_is_cleared_when_client_never_reports_end() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    // 这个点 dwell=0 → 窗口 12s，看门狗在 22s 后清窗口；测试里不想等那么久，
    // 直接验证「没有 ai_end 时窗口仍在」+「stop 能清掉」这条兜底路径。
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;
    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    next_msg(&mut ws, "ai_start").await;
    assert_eq!(ai_patrol::global().status().await["active"], true);
    ai_patrol::global().stop("用例结束").await;
    assert_eq!(ai_patrol::global().status().await["active"], false);
}

/// 数量核对：动作里配了「检查目标数量」→ 服务端解析并随 `ai_start` 下发 `planId` +
/// `expectCount`；客户端转发板端的 `ai_alert` 后落一条告警（同一窗口只一条），
/// 任务时间线同步留痕。
#[tokio::test]
async fn count_mismatch_alert_is_recorded_once() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("count"), Some(3)).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    // 1) 开窗：检查目标数量与计划 id 一起下发（板端据此建存证目录 + 核对数量）
    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let start = &start["payload"];
    assert_eq!(start["planId"], task_code.as_str(), "{start}");
    assert_eq!(start["pointIndex"], 0);
    assert_eq!(start["expectCount"], 3, "检查目标数量要下发给板端: {start}");
    let window_id = start["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    // 2) 板端判定数量不一致 → 客户端转发 ai_alert
    ws.send(Message::Text(
        json!({
            "msgType": "ai_alert",
            "ts": "2026-11-16 10:00:02",
            "payload": {
                "windowId": window_id,
                "taskCode": task_code,
                "planId": task_code,
                "pointIndex": 0,
                "expectCount": 3,
                "foundCount": 2,
                "evidence": "/root/backpack_dashboard/backend/detect_jobs/TK-AI-C/pt0/dogcam_1.jpg"
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_alert 失败");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let contents = alert_contents(&state).await;
    assert_eq!(contents.len(), 1, "数量不一致要落一条告警: {contents:?}");
    assert!(
        contents[0].contains("数量核对不一致")
            && contents[0].contains("检查 3 个")
            && contents[0].contains("实际检出 2 个"),
        "告警内容要能看出检查数量与实际检出: {}",
        contents[0]
    );
    assert!(
        contents[0].contains("dogcam_1.jpg"),
        "证据图路径要带上，便于回溯: {}",
        contents[0]
    );
    // 文案里**不写**航点下标（页面上航点从 P1 开始编号，写「航点 0」会让人
    // 以为凭空多了一个点／少了航点），只写航点名。
    assert!(
        contents[0].contains("航点「新航点 1」") && !contents[0].contains("航点 0"),
        "告警文案只写航点名、不带 0 基下标: {}",
        contents[0]
    );

    // 3) 同一窗口重复上报：不重复落告警
    ws.send(Message::Text(
        json!({
            "msgType": "ai_alert",
            "ts": "2026-11-16 10:00:03",
            "payload": { "windowId": window_id, "expectCount": 3, "foundCount": 1 }
        })
        .to_string(),
    ))
    .await
    .expect("发送重复 ai_alert 失败");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        alert_contents(&state).await.len(),
        1,
        "同一个窗口只该有一条数量告警"
    );

    // 任务时间线也要有这条结论
    let events = event_descriptions(&state).await;
    assert!(
        events.iter().any(|text| text.contains("数量核对不一致")),
        "任务时间线应记录数量不一致: {events:?}"
    );
    // 时间线标题同理：只写航点名，不写 0 基下标
    let titles = event_titles(&state).await;
    assert!(
        titles.iter().any(|text| text == "AI 识别 · 航点「新航点 1」"),
        "事件标题只写航点名: {titles:?}"
    );

    ai_patrol::global().stop("用例结束").await;
}

/// 数量核对任务（`checkType=count`）只推「数量不一致」那一条。
///
/// 检出（`ai_detect`）是过程数据：一个航点检出 3 类目标就落 3 条告警 / 3 张工单 /
/// 3 条群消息，真正要处置的「检查 3 个、实际检出 2 个」反而被淹掉。所以数量任务的检出
/// 只进任务时间线留痕，**不落告警、不建工单、不推企业微信**。
#[tokio::test]
async fn count_task_detections_stay_out_of_alerts_and_wecom() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp, mut rx) = test_state_with_wecom("client", true).await;
    let rx = rx.as_mut().expect("开了企微就该有入队接收端");
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("count"), Some(2))
            .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    // 1) 检出（带证据图也一样）：不落告警、不推群
    ws.send(Message::Text(
        detect_with_evidence(&window_id, "extinguisher", 0.91).to_string(),
    ))
    .await
    .expect("发送检出失败");
    assert!(
        tokio::time::timeout(Duration::from_millis(800), rx.recv())
            .await
            .is_err(),
        "数量任务的检出不该推企业微信"
    );
    assert_eq!(
        alert_count(&state).await,
        0,
        "数量任务的检出不该落告警（也就不会建工单）: {:?}",
        alert_contents(&state).await
    );
    // 但任务时间线仍要留痕：现场还得能追溯「这一轮到底认出了什么」
    let events = event_descriptions(&state).await;
    assert!(
        events
            .iter()
            .any(|text| text.contains("检出 extinguisher")),
        "检出要留在任务时间线里: {events:?}"
    );

    // 2) 板端判定数量不一致 → 这一条才是要处置的告警（照旧推群）
    ws.send(Message::Text(
        json!({
            "msgType": "ai_alert",
            "ts": "2026-11-16 10:00:02",
            "payload": {
                "windowId": window_id,
                "taskCode": task_code,
                "planId": task_code,
                "pointIndex": 0,
                "expectCount": 2,
                "foundCount": 1
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_alert 失败");
    let notice = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("等数量告警入队超时")
        .expect("推送通道提前关闭");
    assert!(
        notice.body.as_deref().unwrap_or_default().contains("数量核对不一致"),
        "群里只该有数量不一致这一条: {notice:?}"
    );
    let contents = alert_contents(&state).await;
    assert_eq!(contents.len(), 1, "整个窗口只该有一条告警: {contents:?}");
    assert!(contents[0].contains("实际检出 1 个"), "{}", contents[0]);

    ai_patrol::global().stop("用例结束").await;
}

/// 数量任务：窗口正常到期却一个目标都没找到 = 数量不一致（0 ≠ N），
/// 也要落数量告警；存在性任务同等情形只算「没找到」，不告警。
#[tokio::test]
async fn count_task_reports_mismatch_when_window_expires_without_hits() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("count"), Some(2))
            .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();
    assert_eq!(start["payload"]["checkType"], "count", "{start}");
    assert_eq!(start["payload"]["expectCount"], 2, "{start}");

    // 窗口到期、一条检出都没有
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:01:48",
            "payload": {
                "windowId": window_id,
                "reason": "窗口到期（108s）",
                "hits": 0,
                "expired": true
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert_eq!(contents.len(), 1, "数量任务未命中要落数量告警: {contents:?}");
    assert!(
        contents[0].contains("数量核对不一致")
            && contents[0].contains("检查 2 个")
            && contents[0].contains("实际检出 0 个"),
        "告警要写清「一个都没找到」: {}",
        contents[0]
    );
    assert_eq!(ai_patrol::global().status().await["active"], false);
}

/// 2026-11-20 13:38 现场（TK-1112-11 PT2）：狗到点 13 s 后才第一次命中（置信度 0.91），
/// `dwell` 已满、平台刚收到「已离开航点」就摘了窗口并写下「检查 2 个，实际检出 0 个」；
/// 客户端 0.6 s 后送来的 `ai_alert`（`foundCount=1` + 证据图）被当成失效窗口丢掉 ——
/// 群里既拿到错的数量，也丢了一张命中帧（板端 `result.json` 写的是 `found_count: 1`）。
///
/// 关窗后的宽限期就是为这条缝加的：结论按板端给的 `foundCount` 落，不再凭空写 0。
#[tokio::test]
async fn late_count_alert_after_leaving_point_is_not_recorded_as_zero() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("count"), Some(2))
            .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    // dwell 4 s（「看够了」的判据 = 4 s）：狗停满就走，和现场一样
    ws.send(Message::Text(
        ai_point_with_dwell(&task_code, 4, false).to_string(),
    ))
    .await
    .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    assert_eq!(start["payload"]["checkType"], "count", "{start}");
    assert_eq!(start["payload"]["expectCount"], 2, "{start}");
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    tokio::time::sleep(Duration::from_millis(4300)).await;
    ws.send(Message::Text(
        task_progress(&task_code, "执行中", "patrol.returning").to_string(),
    ))
    .await
    .expect("发送离开航点失败");
    next_msg(&mut ws, "ai_stop").await;

    // 平台这一刻还不能下结论：客户端正在收尾（停识别 → 板端 `_plan_flush` → 扫盘）
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        alert_contents(&state).await.is_empty(),
        "关窗后不能立刻写「实际检出 0 个」: {:?}",
        alert_contents(&state).await
    );

    // 客户端把板端结论补上来：1 个命中（带图）+ 数量核对不一致 + 窗口结束
    ws.send(Message::Text(
        detect_with_evidence(&window_id, "extinguisher", 0.91).to_string(),
    ))
    .await
    .expect("发送检出失败");
    ws.send(Message::Text(
        json!({
            "msgType": "ai_alert",
            "ts": "2026-11-20 13:38:04",
            "payload": {
                "windowId": window_id,
                "taskCode": task_code,
                "planId": task_code,
                "pointIndex": 0,
                "expectCount": 2,
                "foundCount": 1,
                "evidence": "/root/261113/backpack_dashboard/backend/detect_jobs/TK-1112-11/pt1/dogcam_20261120_133803.jpg"
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送数量告警失败");
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-20 13:38:04",
            "payload": {
                "windowId": window_id,
                "reason": "已离开航点",
                "hits": 1,
                "expired": false
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert_eq!(
        contents.len(),
        1,
        "只该有一条数量告警（不能先来一条假的 0 个）: {contents:?}"
    );
    assert!(
        contents[0].contains("检查 2 个")
            && contents[0].contains("实际检出 1 个")
            && contents[0].contains("dogcam_20261120_133803.jpg"),
        "结论要按板端给的 foundCount 落，并带上证据路径: {}",
        contents[0]
    );
    assert_eq!(ai_patrol::global().status().await["active"], false);
}

/// 存在性任务：① 到期没找到不告警；② 误收到数量告警也不落库（不判数量）。
#[tokio::test]
async fn exist_task_never_judges_quantity() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None)
            .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    assert_eq!(start["payload"]["checkType"], "exist", "{start}");
    assert!(
        start["payload"]["expectCount"].is_null(),
        "存在性任务不该下发检查数量: {start}"
    );
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    // 即使客户端误报数量不一致，服务端也按「存在性任务不判数量」丢弃
    ws.send(Message::Text(
        json!({
            "msgType": "ai_alert",
            "ts": "2026-11-16 10:00:02",
            "payload": { "windowId": window_id, "expectCount": 3, "foundCount": 1 }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_alert 失败");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 到期仍未命中：存在性任务 = 没找到，不告警
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:01:48",
            "payload": {
                "windowId": window_id,
                "reason": "窗口到期（108s）",
                "hits": 0,
                "expired": true
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert!(contents.is_empty(), "存在性任务不该产生数量告警: {contents:?}");
    let events = event_descriptions(&state).await;
    assert!(
        events.iter().any(|text| text.contains("窗口结束")),
        "没找到也要在任务时间线留痕: {events:?}"
    );
}

/// 把已播种路线的 AI 动作改成「未检出也告警」（编辑器写 `alertWhenMissing`）。
///
/// 单独改 JSON 而不是给 `seed_ai_route` 加参数：那 9 个调用点都不需要这个开关。
async fn enable_alert_when_missing(state: &AppState) {
    let raw: (Option<String>,) =
        sqlx::query_as("SELECT route_data FROM routes WHERE route_code = 'RT-AI-C'")
            .fetch_one(&state.db)
            .await
            .expect("读路线失败");
    let mut data: Value =
        serde_json::from_str(raw.0.as_deref().unwrap_or("{}")).expect("路线不是合法 JSON");
    data["points"][0]["actions"][0]["alertWhenMissing"] = json!(true);
    sqlx::query("UPDATE routes SET route_data = ?1 WHERE route_code = 'RT-AI-C'")
        .bind(data.to_string())
        .execute(&state.db)
        .await
        .expect("更新路线失败");
}

/// `exist` + `alertWhenMissing`：窗口正常到期、一个目标都没找到 → 落「未检出」告警。
///
/// 这是 `exist` 缺的那半边语义（「灭火器不见了」）：不加开关时 `exist` 只在**检出**时
/// 告警，缺了只能靠 `count` + `expectCount`。
#[tokio::test]
async fn exist_with_alert_when_missing_reports_when_nothing_found() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None)
            .await;
    enable_alert_when_missing(&state).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    // 窗口正常到期，一条检出都没有
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:01:48",
            "payload": {
                "windowId": window_id,
                "reason": "窗口到期（108s）",
                "hits": 0,
                "expired": true
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert_eq!(contents.len(), 1, "未检出也要告警: {contents:?}");
    assert!(
        contents[0].contains("AI 识别未检出")
            && contents[0].contains("新航点 1")
            && contents[0].contains("extinguisher")
            && contents[0].contains("firextinguisher"),
        "告警要写清是哪个航点、找的是什么: {}",
        contents[0]
    );
    // 结构化去重键：同一航点的「未检出」跨轮巡检都算同一件事
    let key: Option<String> =
        sqlx::query_scalar("SELECT dedup_key FROM alerts WHERE content LIKE 'AI 识别未检出%'")
            .fetch_one(&state.db)
            .await
            .expect("查去重键失败");
    assert_eq!(key.as_deref(), Some("ai:RD-AI-1:TK-AI-C:0:missing"));

    let events = event_descriptions(&state).await;
    assert!(
        events.iter().any(|text| text.contains("AI 识别未检出")),
        "未检出也要进任务时间线: {events:?}"
    );
}

/// 没打开 `alertWhenMissing` 时，`exist` 到期没找到**不**告警（默认行为不变）。
#[tokio::test]
async fn exist_without_alert_when_missing_stays_quiet() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None)
            .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:01:48",
            "payload": {
                "windowId": window_id,
                "reason": "窗口到期（108s）",
                "hits": 0,
                "expired": true
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert!(contents.is_empty(), "没开开关就不该告警: {contents:?}");
}

/// 被任务结束/新航点打断（`expired=false`）时不算「未检出」：狗可能还没看够。
#[tokio::test]
async fn interrupted_window_does_not_report_missing() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None)
            .await;
    enable_alert_when_missing(&state).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:00:20",
            "payload": {
                "windowId": window_id,
                "reason": "任务结束 / 被新航点顶替",
                "hits": 0,
                "expired": false
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert!(contents.is_empty(), "被打断的窗口不记账: {contents:?}");
}

/// 现场形态：狗停满 `dwell` 就走，平台先收到「已离开航点」把窗口摘掉，
/// 客户端随后的 `ai_end` 已经找不到窗口 —— 「没找到也告警」不能因此丢掉。
///
/// 2026-11-18 办公室巡检 pt1「垃圾桶溢出」就是这么漏的：航点停 65 s、窗口 68 s，
/// 窗口永远差几秒到期，「AI 识别未检出」一次都没报出来。
#[tokio::test]
async fn leaving_the_point_after_full_dwell_reports_missing() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None)
            .await;
    enable_alert_when_missing(&state).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    // 判据 = dwell.max(4) = 4 s：等到狗「停满」再让它走
    ws.send(Message::Text(
        ai_point_with_dwell(&task_code, 4, false).to_string(),
    ))
    .await
    .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    assert!(
        start["payload"]["windowId"].as_str().is_some(),
        "ai_start 必须带 windowId: {start}"
    );
    assert_eq!(start["payload"]["secs"], 12, "窗口还是 dwell + 8: {start}");
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    tokio::time::sleep(Duration::from_millis(4300)).await;
    ws.send(Message::Text(
        json!({
            "msgType": "task_progress",
            "ts": "2026-11-16 10:01:00",
            "payload": {
                "taskCode": task_code,
                "status": "执行中",
                "progress": 50,
                "pointIndex": 0,
                "stage": "patrol.navigating",
                "totalPoints": 1
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送导航阶段失败");

    // 窗口被平台「下班」了，但「看够了」的结论还要结算 —— 只不过要等客户端说完话
    let stop = next_msg(&mut ws, "ai_stop").await;
    assert!(
        stop["payload"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("离开"),
        "ai_stop 要说明是离开航点: {stop}"
    );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        alert_contents(&state).await.is_empty(),
        "关窗那一刻还没收到客户端回报，不能先写「未检出」: {:?}",
        alert_contents(&state).await
    );
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-16 10:01:05",
            "payload": {
                "windowId": window_id,
                "reason": "已离开航点",
                "hits": 0,
                "expired": false
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert_eq!(contents.len(), 1, "停满了没找到，也要告警: {contents:?}");
    assert!(
        contents[0].contains("AI 识别未检出") && contents[0].contains("extinguisher"),
        "告警要写清找的是什么: {}",
        contents[0]
    );
}

/// 没停够 `dwell` 就走了（任务被中止 / 狗提前离开）：狗还没看够，不能报「未检出」。
#[tokio::test]
async fn leaving_the_point_early_stays_quiet() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None)
            .await;
    enable_alert_when_missing(&state).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    // dwell 20 s，却只看几百毫秒就走了
    ws.send(Message::Text(
        ai_point_with_dwell(&task_code, 20, false).to_string(),
    ))
    .await
    .expect("发送 ai_point 失败");
    next_msg(&mut ws, "ai_start").await;

    // 提前离开：dwell 20 s，实际只有几百毫秒
    ws.send(Message::Text(
        json!({
            "msgType": "task_progress",
            "ts": "2026-11-16 10:01:00",
            "payload": {
                "taskCode": task_code,
                "status": "执行中",
                "progress": 50,
                "pointIndex": 0,
                "stage": "patrol.navigating",
                "totalPoints": 1
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送导航阶段失败");
    next_msg(&mut ws, "ai_stop").await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let contents = alert_contents(&state).await;
    assert!(contents.is_empty(), "没看够就不算「未检出」: {contents:?}");
}

/// 同一航点同一类别的 AI 问题，跨轮巡检只挂**一张**在办工单。
///
/// 老行为按「设备号 + 告警内容前 60 字」比标题，而 AI 内容里带置信度（0.41 → 0.77），
/// 标题一变就新建一张单，一轮巡检攒一张。现在按 `dedup_key` 比。
#[tokio::test]
async fn same_ai_problem_across_patrols_reuses_one_work_order() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None)
            .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    // 两轮巡检：同一航点、同一类别、不同置信度（= 不同的告警内容/标题）
    for conf in [0.41_f64, 0.77_f64] {
        ws.send(Message::Text(ai_point(&task_code, false).to_string()))
            .await
            .expect("发送 ai_point 失败");
        let start = next_msg(&mut ws, "ai_start").await;
        let window_id = start["payload"]["windowId"]
            .as_str()
            .expect("ai_start 必须带 windowId")
            .to_string();
        ws.send(Message::Text(detect(&window_id, "extinguisher", conf).to_string()))
            .await
            .expect("发送检出失败");
        tokio::time::sleep(Duration::from_millis(300)).await;
        ws.send(Message::Text(
            json!({
                "msgType": "ai_end",
                "ts": "2026-11-16 10:01:48",
                "payload": { "windowId": window_id, "reason": "窗口到期（108s）", "hits": 1 }
            })
            .to_string(),
        ))
        .await
        .expect("发送 ai_end 失败");
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let contents = alert_contents(&state).await;
    assert_eq!(contents.len(), 2, "两轮各一条告警: {contents:?}");
    let key: Option<String> =
        sqlx::query_scalar("SELECT dedup_key FROM alerts WHERE content LIKE 'AI 识别%' LIMIT 1")
            .fetch_one(&state.db)
            .await
            .expect("查去重键失败");
    assert_eq!(key.as_deref(), Some("ai:RD-AI-1:TK-AI-C:0:extinguisher"));

    // 关键：两张告警挂的是同一张工单（标题不同也不会各建一张）
    let orders: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_orders WHERE source = '告警自动派单' AND dedup_key IS NOT NULL",
    )
    .fetch_one(&state.db)
    .await
    .expect("查工单失败");
    assert_eq!(orders, 1, "同一问题只能有一张在办工单");
    let assigned: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT work_order_id FROM alerts WHERE content LIKE 'AI 识别%' ORDER BY id",
    )
    .fetch_all(&state.db)
    .await
    .expect("查派单结果失败");
    assert_eq!(assigned.len(), 2);
    assert!(assigned[0].is_some(), "第一条告警应自动派单: {assigned:?}");
    assert_eq!(assigned[0], assigned[1], "两条告警应挂同一张单: {assigned:?}");
}

/// 检出告警也要证据图（存在性任务 `exist`：灭火器「在」就告警那一类）：
/// 客户端把命中帧（板端路径 + base64）随 `ai_detect` 带上来，
/// 服务端校验后作为**卡片之后的第二条**消息入队（`msgtype=image`），
/// 告警正文里也留一份路径给现场 ssh 复核。
#[tokio::test]
async fn hit_alert_carries_the_relayed_evidence_image() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp, mut rx) = test_state_with_wecom("client", true).await;
    let rx = rx.as_mut().expect("开了企微就该有入队接收端");
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    ws.send(Message::Text(
        detect_with_evidence(&window_id, "extinguisher", 0.64).to_string(),
    ))
    .await
    .expect("发送带证据图的检出失败");

    // 卡片先入队（图片是消费端在正文之后补发的第二条 HTTP 消息）
    let notice = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Some(notice) if notice.image.is_some() => return notice,
                Some(_) => continue,
                None => panic!("推送通道提前关闭"),
            }
        }
    })
    .await
    .expect("等带图告警入队超时");
    let image = notice.image.expect("检出告警应带证据图");
    assert_eq!(image.base64, JPEG_BASE64, "base64 不能带换行/空格");
    assert_eq!(image.md5, JPEG_MD5, "md5 算的是 base64 之前的字节");

    // 正文里带板端路径（运维回头能 ssh 上去看原图）
    let contents = alert_contents(&state).await;
    assert_eq!(contents.len(), 1, "{contents:?}");
    assert!(
        contents[0].contains("（证据：") && contents[0].contains("dogcam_1.jpg"),
        "告警正文要留证据图路径: {}",
        contents[0]
    );

    ai_patrol::global().stop("用例结束").await;
}

/// 「卡片已发、图晚到」补发一张图（2026-11-18 19:35 现场回归）：
/// 板端只命中一帧、采样（连采 3 帧）一直没跑完，`plan_hit` 到窗口收尾
/// `_plan_flush` 才发；客户端 8s 兜底先把卡片发了，停识别扫到图后单独报 `ai_evidence`。
/// 服务端这时**只补一张 `msgtype=image`**，不重发卡片、不新建告警。
#[tokio::test]
async fn late_evidence_is_pushed_as_an_image_only_message() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp, mut rx) = test_state_with_wecom("client", true).await;
    let rx = rx.as_mut().expect("开了企微就该有入队接收端");
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    // 检出没带图（客户端当时还没拿到命中帧；阈值 0.35，0.42 能过）
    ws.send(Message::Text(
        detect(&window_id, "extinguisher", 0.42).to_string(),
    ))
    .await
    .expect("发送检出失败");
    let card = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("等告警入队超时")
        .expect("推送通道提前关闭");
    assert!(card.image.is_none(), "这条卡片本来就没有图: {card:?}");
    assert!(!card.image_only, "卡片不是「只发图」");
    assert_eq!(alert_count(&state).await, 1, "先落一条检出告警");

    // 窗口收尾补发命中帧：只补图
    ws.send(Message::Text(late_evidence(&window_id).to_string()))
        .await
        .expect("发送 ai_evidence 失败");
    let follow = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Some(notice) if notice.image.is_some() => return notice,
                Some(_) => continue,
                None => panic!("推送通道提前关闭"),
            }
        }
    })
    .await
    .expect("等补发的图片入队超时");
    assert!(follow.image_only, "补发的是「只发图」消息，不能重发卡片");
    assert_eq!(follow.title, "", "只发图的消息不该有卡片标题");
    let image = follow.image.expect("补发消息必须带图");
    assert_eq!(image.base64, JPEG_BASE64);
    assert_eq!(image.md5, JPEG_MD5);
    assert_eq!(
        alert_count(&state).await,
        1,
        "补图不能重复落告警（群里会以为是第二条）"
    );

    // 重复补发要被吃掉（同一窗口只补一张）
    ws.send(Message::Text(late_evidence(&window_id).to_string()))
        .await
        .expect("发送重复 ai_evidence 失败");
    assert!(
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .is_err(),
        "重复补发不该再入队"
    );

    ai_patrol::global().stop("用例结束").await;
}

/// 现场常态：平台收到「已离开航点」先摘窗口并下发 `ai_stop`，客户端**才**开始收尾
/// （停识别 → 板端 `_plan_flush` 落盘 → 扫盘 → `ai_evidence`）。
///
/// 所以证据图永远落在「窗口已关」之后，只认 `active` 窗口会把每张晚到的图都丢掉
/// ——2026-11-18 21:34 办公室巡检 pt1「Trash over flow」卡片发了、图没发。
#[tokio::test]
async fn late_evidence_after_window_closed_is_still_pushed() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp, mut rx) = test_state_with_wecom("client", true).await;
    let rx = rx.as_mut().expect("开了企微就该有入队接收端");
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    // 1) 检出没带图（板端还在连采帧画框）→ 先发卡片
    ws.send(Message::Text(
        detect(&window_id, "extinguisher", 0.42).to_string(),
    ))
    .await
    .expect("发送检出失败");
    let card = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("等告警入队超时")
        .expect("推送通道提前关闭");
    assert!(card.image.is_none(), "这条卡片本来就没有图: {card:?}");

    // 2) 狗走了：平台摘窗口 + 下发 ai_stop（客户端收到后才开始扫盘）
    ws.send(Message::Text(
        json!({
            "msgType": "task_progress",
            "ts": "2026-11-16 10:01:00",
            "payload": {
                "taskCode": task_code,
                "status": "执行中",
                "progress": 50,
                "pointIndex": 0,
                "stage": "patrol.navigating",
                "totalPoints": 1
            }
        })
        .to_string(),
    ))
    .await
    .expect("发送导航阶段失败");
    next_msg(&mut ws, "ai_stop").await;
    // 窗口已进入收尾宽限（`closing`）：不再接受新的一轮，但迟到的结论/图仍要认领
    let status = ai_patrol::global().status().await;
    assert_eq!(
        status["closing"].as_str().map(|s| s.contains("离开")),
        Some(true),
        "窗口已收尾：这正是补图容易丢的那一刻: {status}"
    );

    // 3) 图才落盘：`ai_evidence` 晚到，仍然要补出去
    ws.send(Message::Text(late_evidence(&window_id).to_string()))
        .await
        .expect("发送 ai_evidence 失败");
    let follow = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Some(notice) if notice.image.is_some() => return notice,
                Some(_) => continue,
                None => panic!("推送通道提前关闭"),
            }
        }
    })
    .await
    .expect("窗口已关也要能补图，等入队超时");
    assert!(follow.image_only, "补发的是「只发图」消息，不能重发卡片");
    assert_eq!(
        alert_count(&state).await,
        1,
        "补图不能重复落告警（群里会以为是第二条）"
    );

    ai_patrol::global().stop("用例结束").await;
}

/// 客户端上报的进度帧（取消 = `status: "异常"` + `stage: patrol.navigating`）
fn task_progress(task_code: &str, status: &str, stage: &str) -> Value {
    json!({
        "msgType": "task_progress",
        "ts": "2026-11-19 11:09:09",
        "payload": {
            "taskCode": task_code,
            "status": status,
            "progress": if status == "异常" { 100 } else { 0 },
            "pointIndex": 0,
            "waypointIndex": 0,
            "stage": stage,
            "totalPoints": 1
        }
    })
}

/// 取下一条**告警**消息（取消任务本身会先入队一条 `task:` 任务异常通知，跳过）
async fn next_alert_notice(
    rx: &mut tokio::sync::mpsc::Receiver<robodog_ops_server::notify::Notice>,
) -> robodog_ops_server::notify::Notice {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Some(notice) if notice.ref_type == "alert" => return notice,
                Some(_) => continue,
                None => panic!("推送通道提前关闭"),
            }
        }
    })
    .await
    .expect("等告警入队超时")
}

/// 2026-11-19 现场回归：**取消后重跑，AI 命中照样进群**。
///
/// 那天 TK-1112-11 在 11:09:09 被取消、11:09:20 重新下发，11:09:44 / 11:10:09
/// 两个航点都命中了（AL-879/880/881 都已入库、还挂回了原工单 52/57/58），但
/// `notifications` 一条都没有：上一轮在 11:05:38 / 11:06:06 刚推过同一个问题，
/// 跟单消息共用 `alert-repeat:<工单>` 这个 5 分钟去重键 → 第二轮被静默合并，
/// 群里看着就是「重跑后 AI 没上报」。
///
/// 现在 AI 告警按 `ai:<设备>:<任务>:<航点>:<类别>` + 1 分钟推：「同一分钟内的重复
/// 上报」合并（防抖），跨轮次（几分钟一轮、取消后重跑）照推。这里用「重跑时走到
/// 另一个 AI 航点」验证推送通道没被上一轮的键锁死；同一航点 4 分钟后再命中的
/// 时间维度由 `notify::tests::ai_hit_window_lets_the_next_round_through` 钉住。
#[tokio::test]
async fn restart_after_cancel_still_pushes_the_next_round() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp, mut rx) = test_state_with_wecom("client", true).await;
    let rx = rx.as_mut().expect("开了企微就该有入队接收端");
    let task_code =
        seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"], Some("exist"), None).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    // ① 第一轮：航点 0 到点开窗 + 检出 → 告警入库 + 入队一条推送
    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let first = next_msg(&mut ws, "ai_start").await;
    let first_window = first["payload"]["windowId"].as_str().unwrap().to_string();
    ws.send(Message::Text(detect(&first_window, "extinguisher", 0.92).to_string()))
        .await
        .expect("发送检出失败");
    let first_notice = next_alert_notice(rx).await;
    assert_eq!(
        first_notice.dedupe_key, "ai:RD-AI-1:TK-AI-C:0:extinguisher",
        "AI 告警按结构化键推，不能跟设备故障共用 alert-repeat:<工单>"
    );
    assert_eq!(
        first_notice.dedupe_window,
        Some(Duration::from_secs(60)),
        "AI 告警的去重窗口是 1 分钟"
    );

    // ② 取消任务：窗口关掉 + 下发 ai_stop
    ws.send(Message::Text(task_progress(&task_code, "异常", "patrol.navigating").to_string()))
        .await
        .expect("发送取消失败");
    next_msg(&mut ws, "ai_stop").await;

    // ③ 重新下发（`push_task_dispatch` 会把任务切回执行中）
    sqlx::query("UPDATE tasks SET task_status = '执行中', progress = 0 WHERE task_code = ?1")
        .bind(&task_code)
        .execute(&state.db)
        .await
        .expect("把任务切回执行中失败");

    // ④ 重跑后走到**另一个** AI 航点（航点 1）：必须重新开窗、重新进群
    let mut point = ai_point(&task_code, false);
    point["payload"]["pointIndex"] = json!(1);
    point["payload"]["pointName"] = json!("新航点 2");
    ws.send(Message::Text(point.to_string()))
        .await
        .expect("发送第二次 ai_point 失败");
    let second = next_msg(&mut ws, "ai_start").await;
    let second_window = second["payload"]["windowId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(first_window, second_window, "新窗口要有新的 windowId");
    ws.send(Message::Text(detect(&second_window, "extinguisher", 0.93).to_string()))
        .await
        .expect("发送第二次检出失败");
    // 取消那一步会先入队一条「任务异常」通知，这里只等告警消息
    let second_notice = next_alert_notice(rx).await;
    assert_eq!(
        second_notice.dedupe_key, "ai:RD-AI-1:TK-AI-C:1:extinguisher",
        "重跑后的 AI 命中按自己的结构化键推（不能沿用上一轮已推过的键）"
    );

    // ⑤ 同一航点一分钟内再命中：告警照常入库，只是不再推第二条（防抖）
    ws.send(Message::Text(
        json!({
            "msgType": "ai_end",
            "ts": "2026-11-19 11:10:20",
            "payload": { "windowId": second_window, "reason": "已离开航点", "hits": 1 }
        })
        .to_string(),
    ))
    .await
    .expect("发送 ai_end 失败");
    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送第三次 ai_point 失败");
    let third = next_msg(&mut ws, "ai_start").await;
    let third_window = third["payload"]["windowId"].as_str().unwrap().to_string();
    ws.send(Message::Text(detect(&third_window, "extinguisher", 0.94).to_string()))
        .await
        .expect("发送第三次检出失败");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        alert_count(&state).await,
        3,
        "三轮都要落告警: {:?}",
        alert_contents(&state).await
    );

    ai_patrol::global().stop("用例结束").await;
}

/// 证据图进了企业微信群之后，服务端在**已有的设备 WS 链路**上下发 `ai_cleanup`：
/// 存证在板端磁盘（`detect_jobs/<计划>/pt<n>/` 的画框 JPG + `result.json`），
/// 平台读不到也删不掉，只有与 Agent 同机的客户端能动手。
///
/// 删除不可逆，所以只在**图片真的进群**之后才发：这条用例把 webhook 指向本机假企微，
/// 走真的 `Notifier::enqueue → 消费端 → deliver → msgtype=image` 链路。
#[tokio::test]
async fn pushed_evidence_image_triggers_board_cleanup() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (wecom_port, hits) = spawn_fake_wecom(false).await;
    let (state, _temp) =
        state_with_live_wecom("client", format!("http://127.0.0.1:{wecom_port}/send")).await;
    let task_code = seed_ai_route(
        &state,
        Some("firextinguisher"),
        &["extinguisher"],
        Some("exist"),
        None,
    )
    .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"]
        .as_str()
        .expect("ai_start 必须带 windowId")
        .to_string();

    ws.send(Message::Text(
        detect_with_evidence(&window_id, "extinguisher", 0.64).to_string(),
    ))
    .await
    .expect("发送带证据图的检出失败");

    let cleanup = next_msg(&mut ws, "ai_cleanup").await;
    assert_eq!(
        cleanup["payload"]["planId"].as_str(),
        Some(task_code.as_str()),
        "计划 id 就是业务任务号（板端拿它建 `detect_jobs/<计划>/`）"
    );
    assert_eq!(
        cleanup["payload"]["pointIndex"].as_i64(),
        Some(0),
        "只删这个航点的目录"
    );
    assert!(
        hits.load(Ordering::Relaxed) >= 2,
        "卡片 + 图片两条都送到了企微才允许删存证"
    );

    ai_patrol::global().stop("用例结束").await;
}

/// 反例：卡片上没图（客户端当时还没扫到命中帧）→ 群里没有图，板端存证不能删。
///
/// 那种情况下客户端收尾时还会扫一次盘补图（`ai_evidence`），删早了那张图就没了。
#[tokio::test]
async fn alert_without_image_keeps_the_board_evidence() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (wecom_port, hits) = spawn_fake_wecom(false).await;
    let (state, _temp) =
        state_with_live_wecom("client", format!("http://127.0.0.1:{wecom_port}/send")).await;
    let task_code = seed_ai_route(
        &state,
        Some("firextinguisher"),
        &["extinguisher"],
        Some("exist"),
        None,
    )
    .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"].as_str().unwrap().to_string();
    ws.send(Message::Text(
        detect(&window_id, "extinguisher", 0.42).to_string(),
    ))
    .await
    .expect("发送检出失败");

    // 卡片发出去了（假企微收到至少一条），但没有图 → 不能发清理指令
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hits.load(Ordering::Relaxed) == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(hits.load(Ordering::Relaxed), 1, "只有卡片一条消息");
    assert_no_msg(&mut ws, "ai_cleanup", Duration::from_millis(800)).await;

    ai_patrol::global().stop("用例结束").await;
}

/// 反例：图片那条被企微拒了（`errcode 93000`）→ 图没进群，板端存证必须留着，能人工补推。
#[tokio::test]
async fn rejected_evidence_image_keeps_the_board_evidence() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (wecom_port, hits) = spawn_fake_wecom(true).await;
    let (state, _temp) =
        state_with_live_wecom("client", format!("http://127.0.0.1:{wecom_port}/send")).await;
    let task_code = seed_ai_route(
        &state,
        Some("firextinguisher"),
        &["extinguisher"],
        Some("exist"),
        None,
    )
    .await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    let start = next_msg(&mut ws, "ai_start").await;
    let window_id = start["payload"]["windowId"].as_str().unwrap().to_string();
    ws.send(Message::Text(
        detect_with_evidence(&window_id, "extinguisher", 0.64).to_string(),
    ))
    .await
    .expect("发送带证据图的检出失败");

    // 卡片 + 图片两条都发过（图片被拒），但没有 ai_cleanup
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while hits.load(Ordering::Relaxed) < 2 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(hits.load(Ordering::Relaxed) >= 2, "应尝试发过卡片 + 图片");
    assert_no_msg(&mut ws, "ai_cleanup", Duration::from_millis(800)).await;

    ai_patrol::global().stop("用例结束").await;
}
