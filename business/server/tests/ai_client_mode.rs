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
