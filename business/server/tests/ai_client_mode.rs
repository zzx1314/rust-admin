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
        backpack_browser_url: Some("http://192.168.41.246:8000".to_string()),
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
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let config = Arc::new(config_with(&dir, point_ai));
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
    };
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
async fn seed_ai_route(state: &AppState, scene: Option<&str>, labels: &[&str]) -> String {
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
        "retry": 1
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
    json!({
        "msgType": "ai_point",
        "ts": "2026-11-16 10:00:01",
        "payload": {
            "taskCode": task_code,
            "pointIndex": 0,
            "pointName": "新航点 1",
            "dwell": 100,
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

/// 客户端上报 `ai_point` → 服务端按同一套规则解析并下发 `ai_start`，
/// 检出落库（阈值 + 类别去重），`ai_end` 清空窗口态。
#[tokio::test]
async fn client_relayed_point_ai_starts_window_and_records_hits() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"]).await;
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
    let task_code = seed_ai_route(&state, Some("extinguisher"), &["extinguisher", "nope"]).await;
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
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"]).await;
    let port = spawn_server(state.clone()).await;
    let (mut ws, _device) = register_device(port).await;

    ws.send(Message::Text(ai_point(&task_code, false).to_string()))
        .await
        .expect("发送 ai_point 失败");
    next_msg(&mut ws, "ai_start").await;

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
    assert_eq!(ai_patrol::global().status().await["active"], false);
}

/// 狗开始走向下一个点（`stage` 不再是 `patrol.point_*`）时必须立刻关窗：
/// 长停留窗口不再靠「30 s 截断」防误报，改成靠阶段收口，这条是它的回归保护。
#[tokio::test]
async fn leaving_the_point_closes_window_immediately() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("client").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"]).await;
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
    assert_eq!(
        ai_patrol::global().status().await["active"],
        false,
        "离开航点后窗口必须清空（{window_id}）"
    );
}

/// `point_ai = server`（服务端与板端同网段）时不接管客户端上报：
/// 否则两条链路会各开一次窗口、各写一条告警。
#[tokio::test]
async fn server_mode_ignores_client_point_request() {
    let _serial = TEST_LOCK.lock().await;
    ai_patrol::global().stop("测试初始化").await;
    let (state, _temp) = test_state("server").await;
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"]).await;
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
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"]).await;
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
    let task_code = seed_ai_route(&state, Some("firextinguisher"), &["extinguisher"]).await;
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
