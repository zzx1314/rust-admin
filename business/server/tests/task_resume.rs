//! 中断续跑（修复后从断点继续）的回归测试。
//!
//! 现场背景：巡检跑到一半本体报故障（定位丢失 / 规划失败 / 底盘报错），平台把任务判成
//! 「异常」。修好之后只能整条重跑——前面已经巡检过的点（AI 识别 / 拍照 / 停留）
//! 又要再来一遍，一条长路线就是十几分钟。
//!
//! 这里钉住四件事：
//! 1. 中断时把「从哪继续」记进 `tasks.resume_index`（航点下标，0 基）；
//! 2. `POST /tasks/{id}/dispatch {"resume": true}` 的下发载荷带 `startIndex`；
//! 3. 普通的「开始执行 / 重新下发」（不带 `resume`）仍然从起点跑，并把断点清掉；
//! 4. 所有航点都走过（中断在返程）时没有可续跑的点，直接 400 而不是偷偷从起点跑。

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
const TASK_CODE: &str = "TK-RESUME-1";

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
        "robodog-ops-resume-test-{}-{}",
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

    // 演示设备直接当成「可下发」状态：园区已绑 + sim 模式（跳过本体就绪门禁）
    sqlx::query(
        "UPDATE devices SET park_id = COALESCE(park_id, 1), run_mode = 'sim',
            body_online = 1, estop_software = 0, sdk_node_available = 1, ctrl_source = 0
         WHERE device_id = ?1",
    )
    .bind(DEVICE_CODE)
    .execute(&pool)
    .await
    .expect("设备置为可下发失败");

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

type DeviceWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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
    next_msg_type(&mut ws, "register_ack").await;
    ws
}

/// 等指定类型的帧；`None` 表示超时
async fn next_msg(ws: &mut DeviceWs, msg_type: &str, wait: Duration) -> Option<Value> {
    tokio::time::timeout(wait, async {
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
    .ok()
}

async fn next_msg_type(ws: &mut DeviceWs, msg_type: &str) -> Value {
    next_msg(ws, msg_type, Duration::from_secs(5))
        .await
        .unwrap_or_else(|| panic!("{msg_type} 在 5s 内没有收到"))
}

/// 设备上报一条进度；等服务端 ack（保证写库已经发生）
async fn report(ws: &mut DeviceWs, payload: Value) {
    ws.send(Message::Text(
        json!({ "msgType": "task_progress", "payload": payload }).to_string(),
    ))
    .await
    .expect("发送进度帧失败");
    next_msg_type(ws, "task_progress_ack").await;
}

/// 插入一张「真实 ROS 地图」基线（带 file_path + 米制原点），返回地图 ID
async fn insert_real_map(pool: &sqlx::SqlitePool) -> String {
    let map_id = "TEST-MAP@v1.0";
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, map_name, version, frame_id, crs, origin,
            resolution, status, map_width, map_height, file_path, source, created_at)
         VALUES (?1, 1, 'TEST-MAP', 'v1.0', 'test_map', 'ROS map_server', '-14.85, -13.45, 0.0',
            '0.05 m/px', '已发布', 1088, 940, 'maps/test-model.zip', 'backend', '2026-11-01 00:00:00')",
    )
    .bind(map_id)
    .execute(pool)
    .await
    .unwrap();
    map_id.to_string()
}

/// 4 个航点的可下发路线（全部落在图内）
async fn insert_route(pool: &sqlx::SqlitePool, map_id: &str) -> i64 {
    let data = json!({
        "schemaVersion": 3,
        "coordFrame": "map",
        "loop": "往返",
        "points": [
            {"name": "P1", "x": -10.0, "y": -10.0, "dwell": 0, "radius": 0.3, "must": true,
             "heading": 0, "onFail": "跳过继续", "actions": []},
            {"name": "P2", "x": -5.0, "y": -5.0, "dwell": 5, "radius": 0.3, "must": true,
             "heading": 0, "onFail": "跳过继续", "actions": []},
            {"name": "P3", "x": 0.0, "y": 0.0, "dwell": 5, "radius": 0.3, "must": true,
             "heading": 90, "onFail": "跳过继续", "actions": []},
            {"name": "P4", "x": 5.0, "y": 5.0, "dwell": 0, "radius": 0.3, "must": true,
             "heading": 0, "onFail": "跳过继续", "actions": []}
        ],
        "legs": [{"mode": "沿道路", "speed": 1.2, "avoid": "标准", "actions": []}],
        "returnLeg": null
    });
    sqlx::query(
        "INSERT INTO routes (route_code, route_name, park_id, status, version, loop_mode,
            default_speed, route_data, point_count, distance_m, duration_s, battery_pct,
            map_version_id, created_at, updated_at, created_by)
         VALUES ('RT-RESUME-1', '断点续跑路线', 1, '已发布', 'v1.0', '往返', 1.2, ?1, 4, 14.1,
            12, 0, ?2, '2026-11-01 00:00:00', '2026-11-01 00:00:00', 1)",
    )
    .bind(data.to_string())
    .bind(map_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid()
}

async fn insert_task(pool: &sqlx::SqlitePool, route_id: i64) -> i64 {
    let device_pk: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .fetch_one(pool)
        .await
        .unwrap();
    let stamp = robodog_ops_server::db::now();
    sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, level, device_id,
            park_id, route_id, plan_start, plan_end, task_status, progress, publish_status,
            created_at, updated_at)
         VALUES (?1, '断点续跑', '定时巡检', '临时下发', '常规', ?2, 1, ?3, '00:00', '23:59',
            '待执行', 0, '已发布', ?4, ?4)
         RETURNING id",
    )
    .bind(TASK_CODE)
    .bind(device_pk)
    .bind(route_id)
    .bind(&stamp)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn login(port: u16) -> String {
    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://127.0.0.1:{port}/api/auth/login"))
        .json(&json!({"username": "admin", "password": "admin123"}))
        .send()
        .await
        .expect("登录请求失败");
    let body: Value = response.json().await.expect("登录响应不是 JSON");
    body["data"]["token"]
        .as_str()
        .expect("登录响应缺少 token")
        .to_string()
}

/// 调 `POST /tasks/{id}/dispatch`，返回 (HTTP 状态码, 响应体)
async fn dispatch_task(port: u16, token: &str, id: i64, body: Option<Value>) -> (u16, Value) {
    let client = reqwest::Client::new();
    let mut request = client
        .post(format!("http://127.0.0.1:{port}/api/tasks/{id}/dispatch"))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.expect("下发请求失败");
    let status = response.status().as_u16();
    let value: Value = response.json().await.unwrap_or(Value::Null);
    (status, value)
}

async fn resume_index(state: &AppState) -> Option<i64> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT resume_index FROM tasks WHERE task_code = ?1")
        .bind(TASK_CODE)
        .fetch_one(&state.db)
        .await
        .expect("任务应存在")
}

async fn last_waypoint(state: &AppState) -> Option<i64> {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT last_waypoint_index FROM tasks WHERE task_code = ?1",
    )
    .bind(TASK_CODE)
    .fetch_one(&state.db)
    .await
    .expect("任务应存在")
}

async fn task_status(state: &AppState) -> String {
    sqlx::query_scalar("SELECT task_status FROM tasks WHERE task_code = ?1")
        .bind(TASK_CODE)
        .fetch_one(&state.db)
        .await
        .expect("任务应存在")
}

/// 断点记录 + 续跑载荷 + 「重新下发」清断点 + 返程中断不可续跑。
#[tokio::test]
async fn task_resumes_from_the_breakpoint_waypoint() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;
    let token = login(port).await;
    let map_id = insert_real_map(&state.db).await;
    let route_id = insert_route(&state.db, &map_id).await;
    let id = insert_task(&state.db, route_id).await;

    // ① 第一次下发：从起点跑，载荷里没有 startIndex
    let (status, _) = dispatch_task(port, &token, id, None).await;
    assert_eq!(status, 200, "首次下发应该成功");
    let frame = next_msg_type(&mut ws, "task_dispatch").await;
    assert!(
        frame["payload"].get("startIndex").is_none(),
        "从起点跑的下发载荷不该带 startIndex：{frame}"
    );

    // ② 跑到 P3（下标 2）停下时报异常 → 断点 = P3（在点上中断，作业可能没做完）
    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "执行中", "progress": 50,
            "pointIndex": 3, "waypointIndex": 2, "stage": "patrol.waiting",
            "message": "已到达航点",
        }),
    )
    .await;
    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "异常", "progress": 50,
            "pointIndex": 3, "waypointIndex": 2, "stage": "patrol.waiting",
            "message": "本体导航失败（code=13446）",
        }),
    )
    .await;
    assert_eq!(task_status(&state).await, "异常");
    assert_eq!(
        resume_index(&state).await,
        Some(2),
        "断点应记在 P3（0 基下标 2）"
    );

    // 时间线里要写清楚「从哪继续」，现场不用自己数航点
    let hint: Option<String> = sqlx::query_scalar(
        "SELECT description FROM task_events
          WHERE task_id = ?1 AND title LIKE '%中断于%' ORDER BY id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .unwrap();
    let hint = hint.expect("异常时要留一条断点事件");
    assert!(
        hint.contains("P3"),
        "断点事件要写明从 P3 继续，实际：{hint}"
    );

    // ③ 「从断点继续」：载荷带 startIndex=2（前两个点不再执行）
    let (status, body) = dispatch_task(port, &token, id, Some(json!({ "resume": true }))).await;
    assert_eq!(status, 200, "续跑下发应该成功：{body}");
    assert_eq!(body["data"]["resumeIndex"], json!(2));
    let frame = next_msg_type(&mut ws, "task_dispatch").await;
    assert_eq!(
        frame["payload"]["startIndex"],
        json!(2),
        "续跑载荷必须带 startIndex：{frame}"
    );
    assert_eq!(
        frame["payload"]["routeData"]["points"]
            .as_array()
            .map(Vec::len),
        Some(4),
        "routeData 保持整条路线（切片在客户端做，AI / 灯光 / 拍照的动作表还按原始下标索引）"
    );

    // ④ 普通的「重新下发」（不带 resume）= 从起点重跑，断点清掉
    let (status, body) = dispatch_task(port, &token, id, None).await;
    assert_eq!(status, 200, "重新下发应该成功：{body}");
    assert_eq!(body["data"]["resumeIndex"], Value::Null);
    let frame = next_msg_type(&mut ws, "task_dispatch").await;
    assert!(
        frame["payload"].get("startIndex").is_none(),
        "重新下发必须从起点跑：{frame}"
    );

    // ⑤ 人工指定起点：`startIndex` 优先于断点
    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "执行中", "progress": 25,
            "pointIndex": 2, "waypointIndex": 1, "stage": "patrol.point_completed",
        }),
    )
    .await;
    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "异常", "progress": 25,
            "pointIndex": 2, "waypointIndex": 1, "stage": "patrol.point_completed",
            "message": "定位丢失",
        }),
    )
    .await;
    assert_eq!(resume_index(&state).await, Some(1));
    let (status, body) = dispatch_task(port, &token, id, Some(json!({ "startIndex": 2 }))).await;
    assert_eq!(status, 200, "人工指定起点应该成功：{body}");
    let frame = next_msg_type(&mut ws, "task_dispatch").await;
    assert_eq!(frame["payload"]["startIndex"], json!(2));

    // ⑥ 最后一个点之后（返程失败）：没有可续跑的点，续跑请求要报错而不是偷偷从头跑
    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "执行中", "progress": 90,
            "pointIndex": 4, "waypointIndex": 3, "stage": "patrol.returning",
        }),
    )
    .await;
    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "异常", "progress": 90,
            "pointIndex": 4, "waypointIndex": 3, "stage": "patrol.returning",
            "message": "返航规划失败",
        }),
    )
    .await;
    assert_eq!(
        resume_index(&state).await,
        None,
        "所有航点都走过了，没有可续跑的点"
    );
    let (status, body) = dispatch_task(port, &token, id, Some(json!({ "resume": true }))).await;
    assert_eq!(status, 400, "没有断点时不能静默从头跑：{body}");
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("断点") || message.contains("起点"),
        "错误文案要说清为什么不能续跑，实际：{message}"
    );
}

/// 跑完之后断点必须清掉：下一轮（下一个执行日 / 重新下发）从起点开始。
#[tokio::test]
async fn finishing_the_task_clears_the_breakpoint() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;
    let token = login(port).await;
    let map_id = insert_real_map(&state.db).await;
    let route_id = insert_route(&state.db, &map_id).await;
    let id = insert_task(&state.db, route_id).await;

    let (status, _) = dispatch_task(port, &token, id, None).await;
    assert_eq!(status, 200);
    next_msg_type(&mut ws, "task_dispatch").await;

    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "执行中", "progress": 50,
            "pointIndex": 3, "waypointIndex": 2, "stage": "patrol.waiting",
        }),
    )
    .await;
    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "异常", "progress": 50,
            "pointIndex": 3, "waypointIndex": 2, "stage": "patrol.waiting",
            "message": "本体故障",
        }),
    )
    .await;
    assert_eq!(resume_index(&state).await, Some(2));

    // 再跑一轮并跑到「已完成」：断点必须清掉
    let (status, _) = dispatch_task(port, &token, id, Some(json!({ "resume": true }))).await;
    assert_eq!(status, 200);
    next_msg_type(&mut ws, "task_dispatch").await;
    report(
        &mut ws,
        json!({
            "taskCode": TASK_CODE, "status": "已完成", "progress": 100,
            "pointIndex": 4, "waypointIndex": 3, "stage": "patrol.completed",
            "message": "巡检完成",
        }),
    )
    .await;
    assert_eq!(task_status(&state).await, "已完成");
    assert_eq!(resume_index(&state).await, None);
}

/// 续跑下发失败（设备离线 / 急停 / 被接管）时断点必须留着。
///
/// 现场顺序常常是「设备还在急停，先点了从断点继续」→ 400 → 解除急停 → 再点。
/// 如果失败那一次把 `resume_index` 清掉了，第二次就只剩「从起点重跑」，
/// 前面巡检过的点白跑一遍。
#[tokio::test]
async fn failed_resume_dispatch_keeps_the_breakpoint() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let token = login(port).await;
    let map_id = insert_real_map(&state.db).await;
    let route_id = insert_route(&state.db, &map_id).await;
    let id = insert_task(&state.db, route_id).await;

    // 直接造一条「中断在 P3」的记录（设备没有连上网关）
    sqlx::query("UPDATE tasks SET task_status = '异常', resume_index = 2 WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await
        .expect("造断点失败");

    let (status, body) = dispatch_task(port, &token, id, Some(json!({ "resume": true }))).await;
    assert_eq!(status, 400, "设备不在线必须报错：{body}");
    assert_eq!(
        resume_index(&state).await,
        Some(2),
        "下发失败不能把断点弄丢"
    );
}

/// 下发时要把「上一轮到过的航点」清掉：否则新一轮在起步阶段就卡住时，
/// 看门狗会把上一轮的点当成断点，界面给出的「从 Pn 继续」会跳过中间一整段。
#[tokio::test]
async fn dispatch_resets_the_previous_run_position() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;
    let token = login(port).await;
    let map_id = insert_real_map(&state.db).await;
    let route_id = insert_route(&state.db, &map_id).await;
    let id = insert_task(&state.db, route_id).await;

    // 上一轮跑到 P4（下标 3）留下的痕迹
    sqlx::query("UPDATE tasks SET last_waypoint_index = 3 WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();

    // 从起点重跑：痕迹清空
    let (status, _) = dispatch_task(port, &token, id, None).await;
    assert_eq!(status, 200);
    next_msg_type(&mut ws, "task_dispatch").await;
    assert_eq!(last_waypoint(&state).await, None);

    // 从 P3（下标 2）续跑：本轮起点之前那个点（下标 1）才是「已到达」，
    // 这样续跑那一轮在同一个点再中断时，断点仍然是 P3 而不是往后跳。
    sqlx::query("UPDATE tasks SET resume_index = 2, task_status = '异常' WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await
        .unwrap();
    let (status, _) = dispatch_task(port, &token, id, Some(json!({ "resume": true }))).await;
    assert_eq!(status, 200);
    next_msg_type(&mut ws, "task_dispatch").await;
    assert_eq!(last_waypoint(&state).await, Some(1));
}
