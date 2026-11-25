//! 起一个假板端 Agent（HTTP + SSE），把「客户端代调」整条链路跑一遍：
//! `ai_point` 上报 → `ai_start` → 启停 Agent → 收 SSE → 按阈值去重回传 → 收尾。

use super::*;
use axum::Router;
use axum::routing::{get, post};
use futures_util::StreamExt as _;
use tokio::net::TcpListener;

const TASKS: &str = r#"{"count":1,"ready":1,"tasks":[{"task":"firextinguisher","name":"灭火器检测",
    "labels":[{"id":0,"name":"extinguisher","zh":"灭火器"}],"conf":0.25,"ok":true}]}"#;

const STATUS: &str =
    r#"{"running":false,"state":"stopped","default_cam":"dogcam","cams":["dogcam"]}"#;

/// 两帧同一类别（窗口内只报一次）、一帧低于阈值、一帧遥测（忽略），
/// 最后一帧是板端「数量核对不一致」结论（应转成 ai_alert）
const EVENTS: &str = concat!(
    "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
    "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.92}]}\n\n",
    "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
    "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.95}]}\n\n",
    "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
    "\"boxes\":[{\"label\":\"smoke\",\"zh\":\"烟雾\",\"conf\":0.10}]}\n\n",
    "data: {\"type\":\"telemetry\",\"battery\":80}\n\n",
    "data: {\"type\":\"plan_hit\",\"plan_id\":\"TK-1\",\"point_index\":0,\"found\":true,",
    "\"found_count\":2,\"expect_count\":3,\"count_ok\":false,\"count_alert\":true,",
    "\"evidence\":\"/root/backpack_dashboard/backend/detect_jobs/TK-1/pt0/dogcam_1.jpg\"}\n\n",
);

/// 只有检出、没有 `plan_hit` 的事件流：模拟「命中帧还在采样、还没落盘」的那几秒
const DETECT_ONLY: &str = concat!(
    "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
    "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.92}]}\n\n",
);

/// 假 Agent 收到的调用：`(路径, body)`，body 用来验证计划参数确实透传了
type Calls = Arc<Mutex<Vec<(String, Value)>>>;

async fn spawn_agent(calls: Calls) -> String {
    spawn_agent_with(calls, EVENTS.to_string(), None, None).await
}

/// 可定制的假 Agent：
///
/// - `events`：SSE 正文；
/// - `delayed`：（多久之后, 又一段 SSE 正文）——用来模拟板端「命中帧落盘后才发
///   `plan_hit`」：现场实测比第一帧检出晚 4~5s；
/// - `on_stop`：`/api/detect/stop` 时要写一个 JPEG 到哪里——模拟板端
///   `_plan_flush` 在停识别时把还在采样中的命中收尾存证。
async fn spawn_agent_with(
    calls: Calls,
    events: String,
    delayed: Option<(Duration, String)>,
    on_stop: Option<std::path::PathBuf>,
) -> String {
    let app = Router::new()
        .route("/api/detect/tasks", get(|| async { TASKS }))
        .route("/api/detect/status", get(|| async { STATUS }))
        .route(
            "/api/detect/start",
            post(
                |axum::extract::State(calls): axum::extract::State<Calls>,
                 axum::Json(body): axum::Json<Value>| async move {
                    calls.lock().await.push(("start".to_string(), body));
                    axum::Json(json!({"ok": true}))
                },
            ),
        )
        .route(
            "/api/detect/config",
            post(
                |axum::extract::State(calls): axum::extract::State<Calls>,
                 axum::Json(body): axum::Json<Value>| async move {
                    calls.lock().await.push(("config".to_string(), body));
                    axum::Json(json!({"ok": true}))
                },
            ),
        )
        .route(
            "/api/detect/plan/clear",
            post(
                |axum::extract::State(calls): axum::extract::State<Calls>,
                 axum::Json(body): axum::Json<Value>| async move {
                    calls.lock().await.push(("plan/clear".to_string(), body));
                    axum::Json(json!({"ok": true}))
                },
            ),
        )
        .route(
            "/api/detect/stop",
            post(
                move |axum::extract::State(calls): axum::extract::State<Calls>,
                      axum::Json(body): axum::Json<Value>| {
                    let on_stop = on_stop.clone();
                    async move {
                        calls.lock().await.push(("stop".to_string(), body));
                        // 板端停识别时会 `_plan_flush`：命中帧在这一刻才落盘
                        if let Some(path) = on_stop {
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            let _ = std::fs::write(path, [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10]);
                        }
                        axum::Json(json!({"ok": true}))
                    }
                },
            ),
        )
        .route(
            "/api/events",
            get(move || {
                let events = events.clone();
                let delayed = delayed.clone();
                async move {
                    // 真机 SSE 是长连接：推完事件挂着不断开，收尾只能靠窗口到期/打断
                    let head = futures_util::stream::once(async move {
                        Ok::<_, std::io::Error>(events.into_bytes())
                    });
                    let tail = match delayed {
                        Some((after, body)) => futures_util::stream::once(async move {
                            tokio::time::sleep(after).await;
                            Ok::<_, std::io::Error>(body.into_bytes())
                        })
                        .boxed(),
                        None => futures_util::stream::empty().boxed(),
                    };
                    let stream = head.chain(tail).chain(futures_util::stream::pending());
                    (
                        [("content-type", "text/event-stream")],
                        axum::body::Body::from_stream(stream),
                    )
                }
            }),
        )
        .with_state(calls);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假 Agent 失败");
    let port = listener.local_addr().expect("取端口失败").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

fn point() -> AiPoint {
    AiPoint {
        index: 0,
        name: "新航点 1".to_string(),
        dwell: 100,
        hint: "灭火器 · 灭火器".to_string(),
        scene: Some("firextinguisher".to_string()),
        labels: vec!["extinguisher".to_string()],
        threshold: "置信度 ≥ 0.35".to_string(),
        expect_count: Some(3),
        check_type: "count".to_string(),
        alert_when_missing: false,
        notify_when_count_ok: false,
    }
}

fn start_payload(secs: u64, was_running: bool) -> AiStart {
    AiStart::from_value(&json!({
        "windowId": "AIW-1-0-1",
        "action": if was_running { "config" } else { "start" },
        "task": "firextinguisher",
        "taskCode": "TK-1",
        "pointIndex": 0,
        "planId": "TK-1",
        "checkType": "count",
        "expectCount": 3,
        "labels": ["extinguisher"],
        "conf": 0.35,
        "cam": "dogcam",
        "fps": 3,
        "secs": secs,
        "wasRunning": was_running,
    }))
    .expect("构造开窗指令失败")
}

/// 存在性任务（`exist`，没有 `expectCount`）的开窗指令：
/// 「存在就告警」这类航点同样要带证据图，与数量任务走同一条 `ai_detect`。
fn exist_start_payload() -> AiStart {
    exist_start_payload_secs(1)
}

fn exist_start_payload_secs(secs: u64) -> AiStart {
    AiStart::from_value(&json!({
        "windowId": "AIW-1-0-2",
        "action": "start",
        "task": "firextinguisher",
        "taskCode": "TK-1",
        "pointIndex": 0,
        "planId": "TK-1",
        "checkType": "exist",
        "labels": ["extinguisher"],
        "conf": 0.35,
        "cam": "dogcam",
        "fps": 3,
        "secs": secs,
        "wasRunning": false,
    }))
    .expect("构造开窗指令失败")
}

/// 收消息直到收到 `ai_end`（或超时），返回期间收到的 (msgType, payload)
async fn drain_until_end(
    rx: &mut mpsc::Receiver<WsMessage>,
    timeout: Duration,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            panic!("等 ai_end 超时，已收到: {out:?}");
        }
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(msg)) => {
                let end = msg.msg_type == "ai_end";
                out.push((msg.msg_type, msg.payload));
                if end {
                    return out;
                }
            }
            Ok(None) => return out,
            Err(_) => panic!("等 ai_end 超时，已收到: {out:?}"),
        }
    }
}

/// 到点不是马上开始计数：`patrol.point_arrived` 之后本体还要原地转身对准 heading
/// （现场实测 2~3s），那几秒画面在转。板端可以先预热（`/api/detect/start`），
/// 但**计划登记 + 读 SSE**要等 `patrol.waiting`（`mark_settled`）。
#[tokio::test]
async fn window_waits_for_the_body_to_settle_before_counting() {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent(calls.clone()).await;
    let mut patrol = AiPatrol::new(agent, true);
    patrol.set_settle_grace(Duration::from_millis(300));
    let ai = Arc::new(patrol);
    let (tx, mut rx) = mpsc::channel(16);

    ai.start(&tx, start_payload(3, false)).await;
    // 预热已经打下去（模型 / 抓帧 / 画框与转身重叠），但计划还没登记
    let kinds = || {
        let calls = calls.clone();
        async move {
            calls
                .lock()
                .await
                .iter()
                .map(|(kind, _)| kind.clone())
                .collect::<Vec<String>>()
        }
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline && kinds().await.is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(kinds().await, ["start"], "到点先预热，不带计划");

    // 转身还没结束：既不能登记计划，也不能把检出报出去
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(kinds().await, ["start"], "停稳前不该登记航点计划");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), rx.recv())
            .await
            .is_err(),
        "停稳前不该上报检出 / 收尾"
    );

    // 停稳了（`patrol.waiting`）：注册计划 + 开始收事件
    ai.mark_settled("TK-1", 0).await;
    let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;
    assert!(
        messages.iter().any(|(kind, _)| kind == "ai_detect"),
        "停稳后要正常上报检出: {messages:?}"
    );
    let registered = calls
        .lock()
        .await
        .iter()
        .find(|(kind, _)| kind == "config")
        .map(|(_, body)| body.clone())
        .expect("停稳后要用 /api/detect/config 登记计划");
    assert_eq!(registered["plan_id"], "TK-1");
    assert_eq!(registered["point_index"], 0);
}

/// 老本体 / `dwell=0` 的航点可能压根不报 `patrol.waiting`：兜底 grace 到期要按
/// **老行为**开始计数（先预热、再登记计划、再收事件），不能让识别整条消失。
#[tokio::test]
async fn settle_grace_falls_back_to_the_old_behaviour() {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent(calls.clone()).await;
    let mut patrol = AiPatrol::new(agent, true);
    patrol.set_settle_grace(Duration::from_millis(120));
    let ai = Arc::new(patrol);
    let (tx, mut rx) = mpsc::channel(16);

    ai.start(&tx, start_payload(3, false)).await;
    let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;
    assert!(messages.iter().any(|(kind, _)| kind == "ai_detect"));
    let kinds: Vec<String> = calls
        .lock()
        .await
        .iter()
        .map(|(kind, _)| kind.clone())
        .collect();
    assert_eq!(
        kinds,
        ["start", "config", "stop"],
        "预热 → 停稳（兜底）后登记计划 → 收尾停识别"
    );
}

#[tokio::test]
async fn point_arrived_reports_agent_catalog_to_platform() {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent(calls).await;
    let ai = Arc::new(AiPatrol::new(agent, true));
    let (tx, mut rx) = mpsc::channel(8);

    ai.point_arrived(&tx, "TK-1", &point()).await;
    let msg = rx.recv().await.expect("应上报 ai_point");
    assert_eq!(msg.msg_type, "ai_point");
    assert_eq!(msg.payload["taskCode"], "TK-1");
    assert_eq!(msg.payload["pointIndex"], 0);
    assert_eq!(msg.payload["scene"], "firextinguisher");
    assert_eq!(msg.payload["labels"], json!(["extinguisher"]));
    assert_eq!(msg.payload["threshold"], "置信度 ≥ 0.35");
    assert_eq!(msg.payload["expectCount"], 3, "检查目标数量要一起报给平台");
    assert_eq!(msg.payload["checkType"], "count", "任务类型要一起报给平台");
    assert_eq!(msg.payload["agentRunning"], false);
    assert_eq!(msg.payload["agentCam"], "dogcam");
    assert_eq!(
        msg.payload["tasks"]["tasks"][0]["task"], "firextinguisher",
        "必须把板端模型目录原样带上去，否则服务端算不出场景"
    );

    // 同一个点的后续帧（5009 每 500ms 一帧）不能反复上报
    ai.point_arrived(&tx, "TK-1", &point()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), rx.recv())
            .await
            .is_err(),
        "同一个点只能请求一次"
    );
}

#[tokio::test]
async fn window_starts_agent_streams_hits_and_stops_it() {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent(calls.clone()).await;
    let ai = Arc::new(AiPatrol::new(agent, true));
    let (tx, mut rx) = mpsc::channel(16);

    ai.start(&tx, start_payload(1, false)).await;
    let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;

    let detects: Vec<&Value> = messages
        .iter()
        .filter(|(kind, _)| kind == "ai_detect")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(detects.len(), 1, "同一类别只回传一次: {messages:?}");
    assert_eq!(detects[0]["label"], "extinguisher");
    assert_eq!(detects[0]["windowId"], "AIW-1-0-1");
    assert_eq!(detects[0]["planId"], "TK-1", "检出也要带计划 id");
    assert!((detects[0]["conf"].as_f64().unwrap() - 0.92).abs() < 1e-6);

    // 数量核对不一致：板端 plan_hit → 客户端转发 ai_alert（含计划/数量/证据）
    let alerts: Vec<&Value> = messages
        .iter()
        .filter(|(kind, _)| kind == "ai_alert")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(alerts.len(), 1, "数量不一致必须上报一条告警: {messages:?}");
    assert_eq!(alerts[0]["windowId"], "AIW-1-0-1");
    assert_eq!(alerts[0]["planId"], "TK-1");
    assert_eq!(alerts[0]["expectCount"], 3);
    assert_eq!(alerts[0]["foundCount"], 2);
    assert!(
        alerts[0]["evidence"]
            .as_str()
            .unwrap_or_default()
            .ends_with(".jpg"),
        "{}:{}",
        alerts[0],
        "告警要带上板端证据图路径"
    );

    let end = messages
        .iter()
        .find(|(kind, _)| kind == "ai_end")
        .map(|(_, payload)| payload)
        .expect("窗口结束必须回报 ai_end");
    assert_eq!(end["hits"], 1);
    assert!(
        end["reason"].as_str().unwrap_or_default().contains("到期"),
        "{end}"
    );
    assert_eq!(end["expired"], true, "正常到期要标 expired=true: {end}");

    // 窗口结束后要把 Agent 停掉（否则 NPU 一直占着）。顺序：
    // `start`（到点预热，**不带计划**）→ `config`（停稳后登记计划）→ `stop`。
    let kinds: Vec<String> = calls
        .lock()
        .await
        .iter()
        .map(|(kind, _)| kind.clone())
        .collect();
    assert_eq!(kinds, ["start", "config", "stop"]);

    // 预热那一步不能带计划：转身期的命中不能进数量核对 / 不能写存证
    let started = calls
        .lock()
        .await
        .iter()
        .find(|(kind, _)| kind == "start")
        .map(|(_, body)| body.clone())
        .expect("应调用 /api/detect/start");
    assert!(
        started.get("plan_id").is_none(),
        "预热阶段不能带 plan_id: {started}"
    );

    // 计划参数（计划 id / 航点下标 / 检查目标数量）在停稳后那份 config 里透传给板端
    let registered = calls
        .lock()
        .await
        .iter()
        .find(|(kind, _)| kind == "config")
        .map(|(_, body)| body.clone())
        .expect("停稳后应调用 /api/detect/config");
    assert_eq!(registered["plan_id"], "TK-1");
    assert_eq!(registered["point_index"], 0);
    assert_eq!(registered["expect_count"], 3);
    assert!(!ai.status_active().await, "窗口结束后不该还挂着窗口");
}

/// 数量核对**一致** + 动作勾了「一致也通知」：板端 `plan_hit.count_ok=true` →
/// 客户端转一条 `ai_count`（平台只推企业微信群，不落告警/工单）。
#[tokio::test]
async fn matched_count_is_reported_as_a_notification_only() {
    const MATCHED: &str = concat!(
        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
        "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.92}]}\n\n",
        "data: {\"type\":\"plan_hit\",\"plan_id\":\"TK-1\",\"point_index\":0,\"found\":true,",
        "\"found_count\":3,\"expect_count\":3,\"count_ok\":true,\"count_alert\":false,",
        "\"evidence\":\"/root/backpack_dashboard/backend/detect_jobs/TK-1/pt0/dogcam_1.jpg\"}\n\n",
    );
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent_with(calls.clone(), MATCHED.to_string(), None, None).await;
    let ai = Arc::new(AiPatrol::new(agent, true));
    let (tx, mut rx) = mpsc::channel(16);

    let mut start = start_payload(1, false);
    start.notify_when_count_ok = true;
    ai.start(&tx, start).await;
    let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;

    let counted: Vec<&Value> = messages
        .iter()
        .filter(|(kind, _)| kind == "ai_count")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(counted.len(), 1, "数量一致要上报一条通知: {messages:?}");
    assert_eq!(counted[0]["windowId"], "AIW-1-0-1");
    assert_eq!(counted[0]["planId"], "TK-1");
    assert_eq!(counted[0]["expectCount"], 3);
    assert_eq!(counted[0]["foundCount"], 3);
    assert!(
        counted[0]["evidence"]
            .as_str()
            .unwrap_or_default()
            .ends_with(".jpg"),
        "通知也要带证据图路径: {}",
        counted[0]
    );
    assert!(
        messages.iter().all(|(kind, _)| kind != "ai_alert"),
        "一致不能走告警那条路（告警会建工单）: {messages:?}"
    );
}

/// 检出告警要证据图：板端命中帧落在 `detect_jobs/<计划>/pt<航点>/`，
/// 客户端自己扫目录取出最新一张，随 `ai_detect` 上报路径 + base64。
///
/// 用**存在性任务**（`checkType=exist`）跑：现场「灭火器在不在」这类航点就是它，
/// 与数量任务共用同一条 `ai_detect`，图不能只挂在数量告警上。
#[tokio::test]
async fn hit_alert_carries_the_board_evidence_frame() {
    let root = std::env::temp_dir().join(format!(
        "robodog-jobs-hit-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let point_dir = root.join("TK-1").join("pt0");
    std::fs::create_dir_all(&point_dir).expect("建存证目录失败");
    let frame = point_dir.join("dogcam_1.jpg");
    let bytes = [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    std::fs::write(&frame, bytes).expect("写命中帧失败");

    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent(calls).await;
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        agent,
        true,
        Some(root.to_string_lossy().to_string()),
    ));
    let (tx, mut rx) = mpsc::channel(16);
    // 存在性任务（checkType=exist）也要图：现场配的灭火器巡检就是这一类
    ai.start(&tx, exist_start_payload()).await;
    let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&root);

    let detects: Vec<&Value> = messages
        .iter()
        .filter(|(kind, _)| kind == "ai_detect")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(detects.len(), 1, "{messages:?}");
    let evidence = detects[0]["evidence"].as_str().unwrap_or_default();
    assert!(
        evidence.ends_with("TK-1/pt0/dogcam_1.jpg"),
        "检出要带命中帧路径: {}",
        detects[0]
    );
    let encoded = detects[0]["evidenceImage"]
        .as_str()
        .expect("检出要带证据图字节");
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .expect("证据图应是合法 base64");
    assert_eq!(decoded, bytes, "发出去的必须是命中帧原图字节");
}

/// 板端命中帧比 SSE `detect` 晚落盘（现场实测作业收尾时才写）：客户端要等它一会儿，
/// 而不是「那一刻没文件就不带图」。
#[tokio::test]
async fn evidence_frame_that_lands_late_is_still_picked_up() {
    let root = std::env::temp_dir().join(format!(
        "robodog-jobs-late-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let point_dir = root.join("TK-1").join("pt0");
    std::fs::create_dir_all(&point_dir).expect("建存证目录失败");
    // 后台 600ms 后才写命中帧：早于/晚于第一个重试都要能等到
    let late = point_dir.join("dogcam_20261118_174010.jpg");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        std::fs::write(&late, [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).expect("写命中帧失败");
    });

    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent(calls).await;
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        agent,
        true,
        Some(root.to_string_lossy().to_string()),
    ));
    let (tx, mut rx) = mpsc::channel(16);
    ai.start(&tx, exist_start_payload()).await;
    let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;
    let _ = std::fs::remove_dir_all(&root);

    let detect = messages
        .iter()
        .find(|(kind, _)| kind == "ai_detect")
        .map(|(_, payload)| payload)
        .expect("应上报检出");
    assert!(
        detect["evidenceImage"].as_str().is_some(),
        "晚落盘的命中帧也要带上: {detect}"
    );
}

/// 现场事故回归（2026-11-18 18:54，任务 TK-1112-11 航点 1「新航点 2」）：
/// 板端 `_plan_frame` 要连采 3 帧命中才 `_plan_write` 落盘，现场那次命中帧比第一帧
/// 检出晚 **4.6s**；客户端以前命中后只等 2.4s 就上报，群里那条「检出 Broken trash can」
/// 只有卡片、没有图。现在命中先挂起、等 `plan_hit` 带 `evidence` 来（图在它之前已写好）
/// 再上报，晚几秒也能带上图。
#[tokio::test]
async fn evidence_landing_seconds_late_still_rides_with_the_alert() {
    let root = std::env::temp_dir().join(format!(
        "robodog-jobs-plan-hit-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let point_dir = root.join("TK-1").join("pt0");
    std::fs::create_dir_all(&point_dir).expect("建存证目录失败");
    let frame = point_dir.join("dogcam_20261118_185453.jpg");
    // 板端先写图（18:54:53.7）再发 `plan_hit`（带 evidence）；这里 4s 落盘、4.2s 才来事件
    let writer = frame.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        std::fs::write(&writer, [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).expect("写命中帧失败");
    });
    let delayed = format!(
        concat!(
            "data: {{\"type\":\"plan_hit\",\"plan_id\":\"TK-1\",\"point_index\":0,",
            "\"found\":true,\"found_count\":1,\"count_alert\":false,",
            "\"evidence\":\"{}\"}}\n\n"
        ),
        frame.display()
    );

    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent_with(
        calls,
        DETECT_ONLY.to_string(),
        Some((Duration::from_secs(4), delayed)),
        None,
    )
    .await;
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        agent,
        true,
        Some(root.to_string_lossy().to_string()),
    ));
    let (tx, mut rx) = mpsc::channel(16);
    let started = tokio::time::Instant::now();
    ai.start(&tx, exist_start_payload_secs(30)).await;

    let mut detects = Vec::new();
    while detects.is_empty() {
        let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("等检出超时")
            .expect("通道关闭");
        if msg.msg_type == "ai_detect" {
            detects.push(msg.payload);
        }
    }
    let waited = started.elapsed();
    ai.stop("测试结束").await;
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        waited >= Duration::from_secs(3),
        "要等到板端落盘才上报（旧实现 2.4s 就放弃带图了）: {waited:?}"
    );
    assert!(
        detects[0]["evidenceImage"].as_str().is_some(),
        "晚几秒落盘的命中帧也要跟着告警发出去: {}",
        detects[0]
    );
    assert!(
        detects[0]["evidence"]
            .as_str()
            .unwrap_or_default()
            .ends_with("pt0/dogcam_20261118_185453.jpg"),
        "证据路径要用板端 `plan_hit.evidence`: {}",
        detects[0]
    );
}

/// 窗口结束前板端还在采样（没等到 `plan_hit`）：停识别会让板端 `_plan_flush`
/// 把采样中的命中画框存证，客户端要**先停再扫**，把图带上再发告警。
#[tokio::test]
async fn evidence_written_on_plan_flush_is_picked_up_at_window_end() {
    let root = std::env::temp_dir().join(format!(
        "robodog-jobs-flush-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    let frame = root
        .join("TK-1")
        .join("pt0")
        .join("dogcam_20261118_185500.jpg");

    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    // 停识别时才写图（=板端 `_plan_flush`），此前目录里什么都没有
    let agent = spawn_agent_with(calls, DETECT_ONLY.to_string(), None, Some(frame.clone())).await;
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        agent,
        true,
        Some(root.to_string_lossy().to_string()),
    ));
    let (tx, mut rx) = mpsc::channel(16);
    ai.start(&tx, exist_start_payload()).await;
    let messages = drain_until_end(&mut rx, Duration::from_secs(8)).await;
    let _ = std::fs::remove_dir_all(&root);

    let detects: Vec<&Value> = messages
        .iter()
        .filter(|(kind, _)| kind == "ai_detect")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(
        detects.len(),
        1,
        "挂起的检出不能丢（也不能重复发）: {messages:?}"
    );
    assert!(
        detects[0]["evidenceImage"].as_str().is_some(),
        "停识别后补扫到的命中帧要跟着告警发: {}",
        detects[0]
    );
    // 顺序：先停 Agent（板端才会 flush），再发检出，最后 ai_end
    let kinds: Vec<&str> = messages.iter().map(|(kind, _)| kind.as_str()).collect();
    let detect_at = kinds.iter().position(|kind| *kind == "ai_detect").unwrap();
    let end_at = kinds.iter().position(|kind| *kind == "ai_end").unwrap();
    assert!(detect_at < end_at, "检出要赶在 ai_end 之前上报: {kinds:?}");
}

/// 现场事故回归（2026-11-18 19:35，任务 TK-1112-11 航点 1「新航点 2」检出
/// `Trash over flow` 0.29）：板端整个窗口只命中 **1 帧**，采样（连采 3 帧命中）
/// 一直没跑完，`plan_hit` 要到窗口收尾 `_plan_flush` 才发。客户端 8s 兜底先把
/// 卡片发了（没图），停识别扫到图后**单独补一条 `ai_evidence`**——平台只补一张
/// 图，不重发卡片（群里不会出现两条告警）。
#[tokio::test]
async fn card_sent_without_image_is_followed_by_ai_evidence() {
    let root = std::env::temp_dir().join(format!(
        "robodog-jobs-follow-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    // 目录先建好，图要等停识别（板端 `_plan_flush`）时才写
    let point_dir = root.join("TK-1").join("pt0");
    std::fs::create_dir_all(&point_dir).expect("建存证目录失败");
    let frame = point_dir.join("dogcam_20261118_193557.jpg");

    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent_with(calls, DETECT_ONLY.to_string(), None, Some(frame.clone())).await;
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        agent,
        true,
        Some(root.to_string_lossy().to_string()),
    ));
    let (tx, mut rx) = mpsc::channel(16);
    ai.start(&tx, exist_start_payload_secs(60)).await;

    // ① 等 8s 兜底：卡片先发出去，这时还没有图
    let detect = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            match rx.recv().await {
                Some(msg) if msg.msg_type == "ai_detect" => return msg.payload,
                Some(_) => continue,
                None => panic!("通道关闭"),
            }
        }
    })
    .await
    .expect("等兜底上报告警超时");
    assert!(
        detect["evidenceImage"].as_str().is_none(),
        "兜底那条本来就没有图: {detect}"
    );

    // ② 窗口收尾：停识别 → 板端 flush 写出图 → 补 ai_evidence
    ai.stop("测试结束").await;
    let rest = drain_until_end(&mut rx, Duration::from_secs(8)).await;
    let _ = std::fs::remove_dir_all(&root);

    let evidence: Vec<&Value> = rest
        .iter()
        .filter(|(kind, _)| kind == "ai_evidence")
        .map(|(_, payload)| payload)
        .collect();
    assert_eq!(evidence.len(), 1, "补发一条图就够: {rest:?}");
    assert!(
        evidence[0]["evidenceImage"].as_str().is_some(),
        "补发必须带图字节: {}",
        evidence[0]
    );
    assert_eq!(evidence[0]["windowId"], "AIW-1-0-2");
    assert_eq!(evidence[0]["planId"], "TK-1");
    assert!(
        !rest.iter().any(|(kind, _)| kind == "ai_detect"),
        "补图不能重复发卡片: {rest:?}"
    );
}

#[tokio::test]
async fn running_agent_is_hot_updated_and_left_alone() {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let agent = spawn_agent(calls.clone()).await;
    let ai = Arc::new(AiPatrol::new(agent, true));
    let (tx, mut rx) = mpsc::channel(16);

    ai.start(&tx, start_payload(1, true)).await;
    let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;
    assert!(messages.iter().any(|(kind, _)| kind == "ai_detect"));

    // 操作员手动开着识别：只热更新（预热改写参数 → 停稳后带上计划），窗口结束不停人家的会话
    let kinds: Vec<String> = calls
        .lock()
        .await
        .iter()
        .map(|(kind, _)| kind.clone())
        .collect();
    assert_eq!(
        kinds,
        ["config", "config", "plan/clear"],
        "识别留着时必须把计划摘掉，否则后续命中会继续写这个航点的存证"
    );
    let registered = calls
        .lock()
        .await
        .iter()
        .filter(|(kind, _)| kind == "config")
        .map(|(_, body)| body.clone())
        .find(|body| body.get("plan_id").is_some())
        .expect("停稳后应带计划热更新一次");
    assert_eq!(registered["plan_id"], "TK-1");
    assert_eq!(registered["expect_count"], 3);
}

#[tokio::test]
async fn unreachable_agent_is_skipped_without_noise() {
    let ai = Arc::new(AiPatrol::new("http://127.0.0.1:9", true));
    let (tx, mut rx) = mpsc::channel(4);
    ai.point_arrived(&tx, "TK-1", &point()).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .is_err(),
        "连不上 Agent 时不该往平台发消息（只在本地打 WARN）"
    );
}

/// 平台把证据图推进企业微信群 → 删掉板端这一轮的存证（画框 JPG + `result.json`）。
///
/// 只删「这个计划的这个航点」：同一计划别的航点还在跑；整个计划目录空了才顺手删掉。
#[tokio::test]
async fn ai_cleanup_removes_the_point_dir_after_the_image_is_pushed() {
    let root = temp_jobs_dir("cleanup");
    let pt0 = root.join("TK-1").join("pt0");
    let pt1 = root.join("TK-1").join("pt1");
    for dir in [&pt0, &pt1] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF]).unwrap();
        std::fs::write(dir.join("result.json"), b"{}").unwrap();
    }
    let ai = AiPatrol::with_jobs_dir(
        "http://127.0.0.1:9",
        true,
        Some(root.to_string_lossy().to_string()),
    );

    ai.cleanup_evidence("TK-1", 0, "单测").await;
    assert!(!pt0.exists(), "命中的那个航点存证要删干净");
    assert!(pt1.exists(), "同一计划的其它航点不能动");
    assert!(root.join("TK-1").exists(), "计划目录还有 pt1，不能删");

    ai.cleanup_evidence("TK-1", 1, "单测").await;
    assert!(!pt1.exists());
    assert!(
        !root.join("TK-1").exists(),
        "计划目录空了就一起删，别在 detect_jobs 下留空壳"
    );
    assert!(root.exists(), "存证根本身不能动");

    let _ = std::fs::remove_dir_all(&root);
}

/// 窗口还开着时不能当场删：板端可能还在写图，客户端收尾还要扫目录补图（`ai_evidence`）。
/// 先记账，收尾（[`AiPatrol::finish`]）后再删。
#[tokio::test]
async fn ai_cleanup_while_the_window_is_open_is_deferred() {
    let root = temp_jobs_dir("cleanup-defer");
    let pt0 = root.join("TK-1").join("pt0");
    std::fs::create_dir_all(&pt0).unwrap();
    std::fs::write(pt0.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF]).unwrap();
    let ai = AiPatrol::with_jobs_dir(
        "http://127.0.0.1:9",
        true,
        Some(root.to_string_lossy().to_string()),
    );
    *ai.window.lock().await = Some(Window {
        id: "AIW-1".to_string(),
        task_code: "TK-1".to_string(),
        plan_id: "TK-1".to_string(),
        point_index: 0,
        cancel: Arc::new(Notify::new()),
    });

    ai.cleanup_evidence("TK-1", 0, "单测").await;
    assert!(pt0.exists(), "窗口还开着就不能删（板端可能还在写图）");
    assert_eq!(
        ai.pending_cleanup.lock().await.clone(),
        vec![("TK-1".to_string(), 0)],
        "要记账，等收尾再删"
    );

    // 不是当前窗口的航点就可以当场删
    let pt2 = root.join("TK-1").join("pt2");
    std::fs::create_dir_all(&pt2).unwrap();
    ai.cleanup_evidence("TK-1", 2, "单测").await;
    assert!(!pt2.exists());

    let _ = std::fs::remove_dir_all(&root);
}

/// 删目录不可逆：平台下发的 `planId` 只认安全字符，路径穿越一律拒绝
#[test]
fn evidence_paths_refuse_unsafe_plan_ids() {
    let root = "/root/detect_jobs";
    assert_eq!(
        point_dir(root, "TK-1112-11", 3),
        Some(std::path::PathBuf::from("/root/detect_jobs/TK-1112-11/pt3"))
    );
    assert!(
        point_dir(root, "巡检计划-1", 0).is_some(),
        "汉字任务号要能用"
    );
    for bad in ["", "  ", "../etc", "a/b", "a\\b", "TK-1/../..", "TK:1"] {
        assert!(point_dir(root, bad, 0).is_none(), "{bad:?} 不能被拼进路径");
    }
    assert!(point_dir(root, "TK-1", -1).is_none(), "航点下标不能是负数");
}

/// 任务跑完：整个计划目录都删，包括**从没推过图**的航点（数量任务的检出不建单、
/// 一个都没命中的航点只留 `result.json`）—— 它们永远等不到 `ai_cleanup`。
#[tokio::test]
async fn finish_plan_removes_the_whole_plan_dir() {
    let root = temp_jobs_dir("plan-end");
    let pt0 = root.join("TK-1").join("pt0");
    let pt1 = root.join("TK-1").join("pt1");
    let other = root.join("TK-2").join("pt0");
    for dir in [&pt0, &pt1, &other] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("result.json"), b"{}").unwrap();
    }
    std::fs::write(pt0.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF]).unwrap();
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        "http://127.0.0.1:9",
        true,
        Some(root.to_string_lossy().to_string()),
    ));

    ai.finish_plan("TK-1", "单测").await;
    assert!(!root.join("TK-1").exists(), "整个计划目录要删干净");
    assert!(other.exists(), "别的计划不能动");
    assert!(root.exists(), "存证根本身不能动");
    assert!(
        ai.ending_plans.lock().await.is_empty(),
        "删完要把待清理标记摘掉"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// 窗口还在收尾（停识别 → 板端 flush 落盘 → 扫盘补图）时不能当场删：
/// 那几张图还要读出来推群。先记账，等窗口跑完再删。
#[tokio::test]
async fn finish_plan_while_the_window_is_open_defers_to_the_reaper() {
    let root = temp_jobs_dir("plan-defer");
    let pt0 = root.join("TK-1").join("pt0");
    std::fs::create_dir_all(&pt0).unwrap();
    std::fs::write(pt0.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF]).unwrap();
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        "http://127.0.0.1:9",
        true,
        Some(root.to_string_lossy().to_string()),
    ));
    *ai.window.lock().await = Some(Window {
        id: "AIW-1".to_string(),
        task_code: "TK-1".to_string(),
        plan_id: "TK-1".to_string(),
        point_index: 0,
        cancel: Arc::new(Notify::new()),
    });

    ai.finish_plan("TK-1", "单测").await;
    assert!(pt0.exists(), "窗口还开着就不能删（图还没推出去）");
    assert_eq!(
        ai.ending_plans.lock().await.clone(),
        vec!["TK-1".to_string()],
        "要记账，等收尾再删"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// 启动兜底：只清「早就该删」的残留，正在跑的 / 刚建的不能动
#[tokio::test]
async fn sweep_removes_only_stale_plan_dirs() {
    let root = temp_jobs_dir("sweep");
    let stale = root.join("TK-OLD").join("pt0");
    let fresh = root.join("TK-NEW").join("pt0");
    for dir in [&stale, &fresh] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("result.json"), b"{}").unwrap();
    }
    let ai = AiPatrol::with_jobs_dir(
        "http://127.0.0.1:9",
        true,
        Some(root.to_string_lossy().to_string()),
    );

    assert_eq!(
        ai.sweep_plans_older_than(Duration::from_secs(3600)).await,
        0
    );
    assert!(stale.exists() && fresh.exists(), "刚写下的存证不能被当残留");

    assert_eq!(ai.sweep_plans_older_than(Duration::ZERO).await, 2);
    assert!(!stale.exists() && !fresh.exists());
    assert!(root.exists(), "存证根本身不能动");

    let _ = std::fs::remove_dir_all(&root);
}

fn temp_jobs_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "robodog-jobs-{tag}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
