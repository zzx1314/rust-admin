//! 航点到点 AI 识别的集成测试。
//!
//! 链路：客户端 `task_progress`（`patrol.point_arrived` + `waypointIndex`）
//!   → 服务端查路线该点的 `actions[]`，命中「AI 识别」
//!   → 调背包 Agent 开识别 → 读 SSE 的 `detect` 事件 → 写 `alerts` + `task_events`
//!   → 窗口到期/任务终态时停识别。
//!
//! 这里用假 Agent 把整条链路跑一遍，锁住四件容易回归的事：
//! 1. 只有带 AI 动作的航点才开识别（配可见光拍照的点不开）；
//! 2. 低于阈值的框不告警，同一类别在窗口内只报一次；
//! 3. 告警落到 `alerts`（告警中心）与 `task_events`（任务时间线）；
//! 4. 窗口结束后（任务终态）状态清空，并真的把 Agent 的识别停掉。

use axum::Json;
use axum::Router;
use axum::routing::{get, post};
use futures_util::StreamExt;
use robodog_ops_server::app::AppState;
use robodog_ops_server::config::Config;
use robodog_ops_server::gateway_types::TaskProgressPayload;
use robodog_ops_server::{db, gateway, seed};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 每个用例有自己的 `AppState`（也就有自己的 AI 窗口），但都会起假 Agent / 真端口，

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
        "robodog-ops-ai-patrol-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

fn config_with(dir: &std::path::Path, backpack_base_url: Option<String>) -> Config {
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

/// 假 Agent 记下来的调用（`start` / `stop` / `config`），用于断言「真的停了识别」
#[derive(Default)]
struct AgentCalls {
    log: Mutex<Vec<String>>,
}

const TASKS: &str = r#"{
  "models_dir": "/models",
  "count": 1,
  "ready": 1,
  "note": "测试用",
  "tasks": [
    {"task": "fire", "name": "烟火检测", "desc": "火焰与烟雾", "nc": 2,
     "labels": [{"id": 0, "name": "smoke", "zh": "烟雾"}, {"id": 1, "name": "flame", "zh": "火焰"}],
     "conf": 0.4, "alarm": ["flame"], "target": "dogcam", "model_size": 1024, "ok": true, "issues": []}
  ]
}"#;

const STATUS: &str = r#"{
  "running": false,
  "state": "stopped",
  "task": "",
  "labels": null,
  "conf": 0.4,
  "cams": ["dogcam"],
  "fps": 3,
  "warnings": [],
  "stats": {},
  "grabbers": {},
  "error": "",
  "single_cam": true,
  "allowed_cams": ["dogcam", "cam0", "cam1", "cam2", "cam3"],
  "default_cam": "dogcam",
  "preview_stream": "preview"
}"#;

/// 三帧 `detect`：两帧都是「火焰 0.91」（用来验窗口内去重）、
/// 一帧「烟雾 0.5」低于阈值 0.8（用来验阈值过滤）。
const EVENTS: &str = concat!(
    "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"ts\":1,\"task\":\"fire\",",
    "\"boxes\":[{\"label\":\"flame\",\"zh\":\"火焰\",\"conf\":0.91,\"count\":1}]}\n\n",
    "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"ts\":2,\"task\":\"fire\",",
    "\"boxes\":[{\"label\":\"flame\",\"zh\":\"火焰\",\"conf\":0.95,\"count\":1}]}\n\n",
    "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"ts\":3,\"task\":\"fire\",",
    "\"boxes\":[{\"label\":\"smoke\",\"zh\":\"烟雾\",\"conf\":0.5,\"count\":1}]}\n\n",
    "data: {\"type\":\"telemetry\",\"battery\":80}\n\n",
);

async fn spawn_fake_agent(calls: Arc<AgentCalls>) -> String {
    let app = Router::new()
        .route("/api/detect/tasks", get(|| async { TASKS }))
        .route("/api/detect/status", get(|| async { STATUS }))
        .route(
            "/api/detect/start",
            post(
                |axum::extract::State(calls): axum::extract::State<Arc<AgentCalls>>| async move {
                    calls.log.lock().unwrap().push("start".to_string());
                    Json(json!({"ok": true}))
                },
            ),
        )
        .route(
            "/api/detect/stop",
            post(
                |axum::extract::State(calls): axum::extract::State<Arc<AgentCalls>>| async move {
                    calls.log.lock().unwrap().push("stop".to_string());
                    Json(json!({"ok": true}))
                },
            ),
        )
        .route(
            "/api/detect/config",
            post(
                |axum::extract::State(calls): axum::extract::State<Arc<AgentCalls>>| async move {
                    calls.log.lock().unwrap().push("config".to_string());
                    Json(json!({"ok": true}))
                },
            ),
        )
        .route(
            "/api/events",
            get(|| async {
                // 真机 Agent 的 SSE 是长连接：先把事件推完，再挂着不断开。
                // 这样「窗口收尾」只能由任务终态/到期触发，才能覆盖那条路径。
                let stream = futures_util::stream::once(async {
                    Ok::<_, std::io::Error>(EVENTS.as_bytes().to_vec())
                })
                .chain(futures_util::stream::pending());
                (
                    [("content-type", "text/event-stream")],
                    axum::body::Body::from_stream(stream),
                )
            }),
        )
        .with_state(calls);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假 Agent 端口失败");
    let port = listener.local_addr().expect("取端口失败").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

async fn state_with(agent: Option<String>, dir: PathBuf) -> AppState {
    state_with_point_ai(agent, "auto", dir).await
}

/// 指定 `point_ai` 的部署形态（`server` = 服务端直连 Agent）
async fn state_with_point_ai(agent: Option<String>, point_ai: &str, dir: PathBuf) -> AppState {
    let mut raw = config_with(&dir, agent);
    raw.point_ai = point_ai.to_string();
    let config = Arc::new(raw);
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

/// 插一条「首点带 AI 识别动作」的路线 + 一条引用它的任务，返回任务号
async fn seed_ai_route(state: &AppState, ai_action: bool) -> String {
    let device: (i64, Option<i64>) = sqlx::query_as("SELECT id, park_id FROM devices LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("播种数据里应至少有一台设备");
    let route_data = json!({
        "schemaVersion": 1,
        "loop": "往返",
        "coordFrame": "map",
        "points": [{
            "name": "配电柜",
            "x": 1.0,
            "y": 2.0,
            "dwell": 6,
            "heading": 90,
            "radius": 0.3,
            "must": true,
            "onFail": "跳过继续",
            "actions": if ai_action {
                json!([{ "type": "AI 识别", "param": "烟火 / 人员", "trigger": "到达即执行",
                         "threshold": "置信度 > 0.8", "retry": 1 }])
            } else {
                json!([{ "type": "可见光拍照", "param": "1080P", "trigger": "到达即执行",
                         "threshold": "—", "retry": 1 }])
            }
        }],
        "legs": []
    });
    let timestamp = "2026-11-15 10:00:00";
    sqlx::query(
        "INSERT INTO routes (route_code, route_name, park_id, status, loop_mode, default_speed,
            route_data, point_count, created_at, updated_at)
         VALUES ('RT-AI-1', 'AI 识别测试路线', ?1, '已发布', '往返', 1.2, ?2, 1, ?3, ?3)",
    )
    .bind(device.1)
    .bind(route_data.to_string())
    .bind(timestamp)
    .execute(&state.db)
    .await
    .expect("插入路线失败");
    let route_id: i64 = sqlx::query_scalar("SELECT id FROM routes WHERE route_code = 'RT-AI-1'")
        .fetch_one(&state.db)
        .await
        .expect("读路线 id 失败");

    sqlx::query(
        "INSERT INTO tasks (task_code, task_name, task_type, device_id, park_id, route_id,
            task_status, progress, created_at, updated_at)
         VALUES ('TK-AI-1', 'AI 识别测试任务', '定时巡检', ?1, ?2, ?3, '执行中', 0, ?4, ?4)",
    )
    .bind(device.0)
    .bind(device.1)
    .bind(route_id)
    .bind(timestamp)
    .execute(&state.db)
    .await
    .expect("插入任务失败");
    "TK-AI-1".to_string()
}

fn arrival(task_code: &str, stage: &str) -> TaskProgressPayload {
    arrival_at(task_code, stage, 0)
}

/// 5009 驱动的到点载荷：`pointIndex` 与 `waypointIndex` 都是 5009 的 `detail_index`
/// （0 基航点下标）——注意定时摘要里的 `pointIndex` 却是「已完成航点数」，两边语义不同。
fn arrival_at(task_code: &str, stage: &str, waypoint_index: i64) -> TaskProgressPayload {
    serde_json::from_value(json!({
        "taskCode": task_code,
        "status": "执行中",
        "progress": 30,
        "pointIndex": waypoint_index,
        "waypointIndex": waypoint_index,
        "stage": stage,
        "totalPoints": 2,
    }))
    .expect("构造进度载荷失败")
}

/// 客户端每 5s 的定时摘要（`已完成 n/N 个航点`）：
/// `pointIndex` 是**已完成航点数**，不是航点下标——这正是 TK-1112-11 事故的起因。
fn interval_summary(
    task_code: &str,
    completed: i64,
    waypoint_index: i64,
    total: i64,
) -> TaskProgressPayload {
    serde_json::from_value(json!({
        "taskCode": task_code,
        "status": "执行中",
        "progress": 50,
        "pointIndex": completed,
        "waypointIndex": waypoint_index,
        "message": format!("已完成 {completed}/{total} 个航点"),
        "stage": "patrol.waiting",
        "totalPoints": total,
    }))
    .expect("构造定时摘要载荷失败")
}

fn terminal(task_code: &str) -> TaskProgressPayload {
    serde_json::from_value(json!({
        "taskCode": task_code,
        "status": "已完成",
        "progress": 100,
        "pointIndex": 2,
        "waypointIndex": 1,
        "stage": "patrol.completed",
        "totalPoints": 1,
    }))
    .expect("构造终态载荷失败")
}

#[tokio::test]
async fn ai_action_at_waypoint_starts_detect_and_records_alert() {
    let calls = Arc::new(AgentCalls::default());
    let agent = spawn_fake_agent(calls.clone()).await;
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let state = state_with(Some(agent), dir).await;
    let task_code = seed_ai_route(&state, true).await;

    state
        .ai_patrol
        .on_progress(
            &state,
            "RD-TEST",
            &arrival(&task_code, "patrol.point_arrived"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(800)).await;

    // 告警中心：只报「火焰」一类（烟雾 0.5 低于阈值 0.8；两帧火焰只算一次）
    // 播种数据里本来就有 5 条演示告警，只看 AI 写的那条
    let alerts: Vec<(String, String)> = sqlx::query_as(
        "SELECT alert_code, content FROM alerts WHERE content LIKE 'AI 识别：%' ORDER BY id",
    )
    .fetch_all(&state.db)
    .await
    .expect("查告警失败");
    assert_eq!(alerts.len(), 1, "应当只写一条告警，实际 {alerts:?}");
    assert!(
        alerts[0].0.starts_with("AL-"),
        "告警编号格式不对: {}",
        alerts[0].0
    );
    assert!(
        alerts[0].1.contains("火焰"),
        "告警内容缺类别: {}",
        alerts[0].1
    );
    assert!(
        alerts[0].1.contains("配电柜"),
        "告警内容缺航点名: {}",
        alerts[0].1
    );
    // 文案不写航点下标（页面上航点从 P1 编号，`航点 0` 会被误读成「多了个点」）
    assert!(
        alerts[0].1.contains("航点「配电柜」") && !alerts[0].1.contains("航点 0"),
        "告警文案只写航点名、不带 0 基下标: {}",
        alerts[0].1
    );

    // 任务时间线也留一条，现场排查「这条告警属于哪个点」
    let events: Vec<(String, String)> =
        sqlx::query_as("SELECT event_type, title FROM task_events ORDER BY id")
            .fetch_all(&state.db)
            .await
            .expect("查任务事件失败");
    let ai_events: Vec<&(String, String)> =
        events.iter().filter(|(kind, _)| kind == "ai").collect();
    assert_eq!(ai_events.len(), 1, "应写一条 ai 事件，实际 {events:?}");
    assert!(ai_events[0].1.contains("配电柜"), "事件标题缺航点名");
    assert_eq!(
        ai_events[0].1, "AI 识别 · 航点「配电柜」",
        "事件标题只写航点名，不写 0 基下标"
    );

    // 状态接口能看到窗口与已上报类别
    let status = state.ai_patrol.status().await;
    assert_eq!(status["active"], json!(true), "窗口应当是开着的: {status}");
    assert_eq!(
        status["scene"],
        json!("fire"),
        "场景应命中烟火模型: {status}"
    );
    assert_eq!(
        status["threshold"],
        json!(0.8),
        "阈值应取自动作配置: {status}"
    );
    assert_eq!(status["hits"].as_array().map(Vec::len), Some(1));

    // 任务终态 → 窗口关闭 + 真的停掉 Agent 的识别
    state
        .ai_patrol
        .on_progress(&state, "RD-TEST", &terminal(&task_code))
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(state.ai_patrol.status().await["active"], json!(false));
    let log = calls.log.lock().unwrap().clone();
    assert!(
        log.contains(&"start".to_string()),
        "应当启动过识别: {log:?}"
    );
    assert!(
        log.contains(&"stop".to_string()),
        "窗口结束应当停掉识别: {log:?}"
    );
    drop(temp);
}

/// 长停留窗口不再按 30 s 截断，离开航点（`stage` 变成 `patrol.navigating`）时
/// 必须立刻停识别——这是取代「截断时长」的防误报机制。
#[tokio::test]
async fn leaving_the_point_stops_detect_in_server_mode() {
    let calls = Arc::new(AgentCalls::default());
    let agent = spawn_fake_agent(calls.clone()).await;
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let state = state_with_point_ai(Some(agent), "server", dir).await;
    let task_code = seed_ai_route(&state, true).await;

    state
        .ai_patrol
        .on_progress(
            &state,
            "RD-TEST",
            &arrival(&task_code, "patrol.point_arrived"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let status = state.ai_patrol.status().await;
    assert_eq!(status["active"], json!(true), "到点应开窗: {status}");
    // dwell=6 → 14s 窗口（不再是 30s 上限，也不该被别的规则改回去）
    assert_eq!(status["windowSecs"], json!(14), "{status}");

    let leaving: TaskProgressPayload = serde_json::from_value(json!({
        "taskCode": task_code,
        "status": "执行中",
        "progress": 50,
        "pointIndex": 0,
        "stage": "patrol.navigating",
        "totalPoints": 1,
    }))
    .expect("构造导航阶段载荷失败");
    state
        .ai_patrol
        .on_progress(&state, "RD-TEST", &leaving)
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    assert_eq!(
        state.ai_patrol.status().await["active"],
        json!(false),
        "离开航点后窗口必须关闭"
    );
    let log = calls.log.lock().unwrap().clone();
    assert!(
        log.contains(&"start".to_string()) && log.contains(&"stop".to_string()),
        "开过窗就一定要停: {log:?}"
    );
    drop(temp);
}

#[tokio::test]
async fn point_without_ai_action_never_starts_detect() {
    let calls = Arc::new(AgentCalls::default());
    let agent = spawn_fake_agent(calls.clone()).await;
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let state = state_with(Some(agent), dir).await;
    let task_code = seed_ai_route(&state, false).await;

    state
        .ai_patrol
        .on_progress(&state, "RD-TEST", &arrival(&task_code, "patrol.waiting"))
        .await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert_eq!(state.ai_patrol.status().await["active"], json!(false));
    assert!(
        calls.log.lock().unwrap().is_empty(),
        "没有 AI 动作就不该碰 Agent"
    );
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE content LIKE 'AI 识别：%'")
            .fetch_one(&state.db)
            .await
            .expect("查告警失败");
    assert_eq!(count, 0);
    drop(temp);
}

/// 服务端直连模式（`point_ai=server`）却没配 `ROBODOG_BACKPACK_URL` 时，
/// 不能把窗口留在状态里：否则 `/api/backpack/ai-window` 会一直说「正在识别」，现场会误判。
///
/// 注意 `auto` 形态下这种部署已经改走客户端代调（见 `tests/ai_client_mode.rs`），
/// 这条只覆盖「显式配成 server 但漏了地址」的错配。
#[tokio::test]
async fn missing_agent_does_not_leave_stale_window() {
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let state = state_with_point_ai(None, "server", dir).await;
    let task_code = seed_ai_route(&state, true).await;

    state
        .ai_patrol
        .on_progress(
            &state,
            "RD-TEST",
            &arrival(&task_code, "patrol.point_arrived"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(state.ai_patrol.status().await["active"], json!(false));
    drop(temp);
}

/// 插一条「0 号点带 AI 识别、1 号点是普通拍照」的两点路线 + 任务。
///
/// 这个形状就是 2026-11-17 现场 `TK-1112-11`（`RT-005 华郅办公室`）的缩样：
/// AI 点后面紧跟一个非 AI 点，是「AI 窗口被 5s 摘要关掉」那条回归的复现条件。
async fn seed_ai_route_two_points(state: &AppState) -> String {
    let device: (i64, Option<i64>) = sqlx::query_as("SELECT id, park_id FROM devices LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("播种数据里应至少有一台设备");
    let route_data = json!({
        "schemaVersion": 3,
        "loop": "闭环",
        "coordFrame": "map",
        "points": [
            {
                "name": "新航点 1", "x": 1.0, "y": 2.0, "dwell": 10, "radius": 0.4,
                "must": false, "onFail": "跳过继续",
                "actions": [{ "type": "AI 识别", "param": "灭火器检测 · 灭火器",
                              "scene": "firextinguisher", "labels": ["extinguisher"],
                              "threshold": "置信度 ≥ 0.4", "trigger": "到达即执行" }]
            },
            {
                "name": "新航点 2", "x": 8.0, "y": 2.0, "dwell": 5, "radius": 0.4,
                "must": false, "onFail": "跳过继续",
                "actions": [{ "type": "可见光拍照", "param": "1080P", "trigger": "到达即执行" }]
            }
        ],
        "legs": []
    });
    let timestamp = "2026-11-17 18:26:00";
    sqlx::query(
        "INSERT INTO routes (route_code, route_name, park_id, status, loop_mode, default_speed,
            route_data, point_count, created_at, updated_at)
         VALUES ('RT-AI-2', 'AI 识别两点测试路线', ?1, '已发布', '闭环', 1.2, ?2, 2, ?3, ?3)",
    )
    .bind(device.1)
    .bind(route_data.to_string())
    .bind(timestamp)
    .execute(&state.db)
    .await
    .expect("插入路线失败");
    let route_id: i64 = sqlx::query_scalar("SELECT id FROM routes WHERE route_code = 'RT-AI-2'")
        .fetch_one(&state.db)
        .await
        .expect("读路线 id 失败");

    sqlx::query(
        "INSERT INTO tasks (task_code, task_name, task_type, device_id, park_id, route_id,
            task_status, progress, created_at, updated_at)
         VALUES ('TK-AI-2', 'AI 识别两点测试任务', '定时巡检', ?1, ?2, ?3, '执行中', 0, ?4, ?4)",
    )
    .bind(device.0)
    .bind(device.1)
    .bind(route_id)
    .bind(timestamp)
    .execute(&state.db)
    .await
    .expect("插入任务失败");
    "TK-AI-2".to_string()
}

/// 回归（2026-11-17 现场 `TK-1112-11`）：客户端的 5s 定时摘要把「已完成航点数」放在
/// `pointIndex` 里，服务端曾拿它当航点下标 → 站在 0 号点却查到 1 号点（拍照，无 AI 动作）
/// → 每 5s 关一次窗再被客户端重新开起来。
///
/// 现场特征：AI 窗口只活 3~5s（配置是 18s）、Agent 反复起停 3 次、零检出零告警。
/// 现在只认 `waypointIndex`，摘要载荷不能再动窗口；真到 1 号点仍要收口。
#[tokio::test]
async fn interval_summary_does_not_kill_the_window() {
    let calls = Arc::new(AgentCalls::default());
    let agent = spawn_fake_agent(calls.clone()).await;
    let dir = temp_dir();
    let temp = TempDb { dir: dir.clone() };
    let state = state_with_point_ai(Some(agent), "server", dir).await;
    let task_code = seed_ai_route_two_points(&state).await;

    // 到点 0（有 AI 动作）→ 开窗
    state
        .ai_patrol
        .on_progress(
            &state,
            "RD-TEST",
            &arrival(&task_code, "patrol.point_arrived"),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let status = state.ai_patrol.status().await;
    assert_eq!(status["active"], json!(true), "到点应开窗: {status}");
    assert_eq!(status["pointIndex"], json!(0), "{status}");
    // dwell=10 → 窗口 18s：现场被 5s 摘要打断的就是这个窗口
    assert_eq!(status["windowSecs"], json!(18), "{status}");

    // 第一次 5s 摘要：已完成 1/2、当前点仍是 0
    state
        .ai_patrol
        .on_progress(&state, "RD-TEST", &interval_summary(&task_code, 1, 0, 2))
        .await;
    // 再来几次，模拟长停留时每 5s 一条
    for _ in 0..3 {
        state
            .ai_patrol
            .on_progress(&state, "RD-TEST", &interval_summary(&task_code, 1, 0, 2))
            .await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let status = state.ai_patrol.status().await;
    assert_eq!(
        status["active"],
        json!(true),
        "定时摘要不能关掉窗口（pointIndex 是完成数，不是航点下标）: {status}"
    );
    assert_eq!(
        status["pointIndex"],
        json!(0),
        "窗口应仍属于 0 号点: {status}"
    );
    let log = calls.log.lock().unwrap().clone();
    assert!(
        !log.contains(&"stop".to_string()),
        "摘要不该让 Agent 停识别: {log:?}"
    );

    // 真的走到 1 号点（无 AI 动作）：窗口必须收口，别让相机继续把别的画面当 0 号点的目标
    state
        .ai_patrol
        .on_progress(
            &state,
            "RD-TEST",
            &arrival_at(&task_code, "patrol.point_arrived", 1),
        )
        .await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        state.ai_patrol.status().await["active"],
        json!(false),
        "到达无 AI 动作的航点后窗口必须关闭"
    );
    let log = calls.log.lock().unwrap().clone();
    assert!(
        log.contains(&"stop".to_string()),
        "窗口收口要真的停掉识别: {log:?}"
    );
    drop(temp);
}
