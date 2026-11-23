//! 定时巡检「按配置的时间段执行」的回归测试。
//!
//! 背景：`plan_start` / `plan_end` 以前只是两个手填字符串，建任务即推、设备上线补发
//! 都不看它们，「计划时间 09:30」的任务在凌晨建好就会立刻下发。这里钉住三件事：
//!
//! 1. 没到窗口的定时巡检不下发（设备就绪也一样等）；
//! 2. 到点由 `schedule::sweep` 自动下发，并记下 `last_dispatched_on`（同一天不重跑）；
//! 3. 窗口过了没跑成只告警一次，任务留在「待执行」等下一个执行日。

use futures_util::{SinkExt, StreamExt};
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::auth::Claims;
use robodog_ops_server::config::Config;
use robodog_ops_server::models::TaskRequest;
use robodog_ops_server::{db, gateway, handlers, schedule, seed};
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
        "robodog-ops-schedule-test-{}-{}",
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
        web_dist: dir.join("web-dist"),
        backend_db_path: dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: dir.join("map_ingest"),
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
    let register = json!({
        "msgType": "register",
        "ts": "2026-01-01 00:00:00",
        "payload": {
            "deviceId": DEVICE_CODE,
            "secret": null,
            "model": "X30-Pro",
            "firmware": "test-fw",
        },
    });
    ws.send(Message::Text(register.to_string()))
        .await
        .expect("发送注册帧失败");
    next_msg_type(&mut ws, "register_ack").await;
    ws
}

/// 等指定类型的帧；`None` 表示超时（用来断言「不该下发」）
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

async fn device_pk(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT id FROM devices WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .fetch_one(&state.db)
        .await
        .expect("演示设备应在台账里")
}

async fn create_scheduled(state: &AppState, name: &str, plan_start: &str, plan_end: &str) -> i64 {
    create_scheduled_with(state, name, plan_start, plan_end, None).await
}

/// `publish_status` 为 `None` 时走默认值（= 已发布，老调用方语义不变）。
async fn create_scheduled_with(
    state: &AppState,
    name: &str,
    plan_start: &str,
    plan_end: &str,
    publish_status: Option<&str>,
) -> i64 {
    let pk = device_pk(state).await;
    let park: i64 = sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
        .bind(pk)
        .fetch_one(&state.db)
        .await
        .unwrap();
    let task = handlers::tasks::create_task_core(
        state,
        TaskRequest {
            task_code: None,
            task_name: name.to_string(),
            task_type: Some("定时巡检".to_string()),
            execute_mode: Some("周期任务".to_string()),
            end_task: None,
            light_night_only: None,
            level: Some("常规".to_string()),
            device_id: Some(pk),
            park_id: Some(park),
            zone_id: None,
            route_id: Some(None),
            plan_start: Some(plan_start.to_string()),
            plan_end: Some(plan_end.to_string()),
            schedule_days: None,
            publish_status: publish_status.map(str::to_string),
            task_status: None,
            progress: None,
            dispatch_policy: None,
            description: None,
        },
        None,
    )
    .await
    .expect("建任务失败");
    task.id
}

async fn task_row(state: &AppState, id: i64) -> (String, Option<String>) {
    sqlx::query_as("SELECT task_status, last_dispatched_on FROM tasks WHERE id = ?1")
        .bind(id)
        .fetch_one(&state.db)
        .await
        .expect("任务应存在")
}

fn clock(offset_minutes: i64) -> String {
    let at = chrono::Local::now() + chrono::Duration::minutes(offset_minutes);
    at.format("%H:%M").to_string()
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 直接插一条周期任务（绕过建任务时的坐标预检，用来造「到点必发不出去」的坏数据）。
async fn insert_periodic(
    state: &AppState,
    code: &str,
    name: &str,
    route_id: Option<i64>,
    start: &str,
    end: &str,
) -> i64 {
    let pk = device_pk(state).await;
    let park: i64 = sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
        .bind(pk)
        .fetch_one(&state.db)
        .await
        .unwrap();
    let stamp = robodog_ops_server::db::now();
    sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, level, device_id,
            park_id, route_id, plan_start, plan_end, task_status, progress, created_at, updated_at)
         VALUES (?1, ?2, '定时巡检', '周期任务', '常规', ?3, ?4, ?5, ?6, ?7, '待执行', 0, ?8, ?8)
         RETURNING id",
    )
    .bind(code)
    .bind(name)
    .bind(pk)
    .bind(park)
    .bind(route_id)
    .bind(start)
    .bind(end)
    .bind(&stamp)
    .fetch_one(&state.db)
    .await
    .expect("插入周期任务失败")
}

/// 一条「到点就下发不出去」的计划不能拖垮整轮扫描：排在它后面的计划照常下发。
///
/// 背景：`sweep` 以前对 `dispatch_due` 用 `?`，一条坏数据（路线坐标不是米制 / 地图
/// 是示意坐标系 / 地图包缺失）就会让整轮扫描中止，后面所有待执行的计划一起停摆，
/// 而日志里只有一句「计划调度扫描失败」。现场表现：某些时段整个平台的定时巡检都不动。
#[tokio::test]
async fn one_undispatchable_plan_does_not_stop_the_whole_sweep() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    // 把种子路线改成「画布像素坐标」（缺 coordFrame=map）：引用它的计划到点必报错
    let route_id: i64 = sqlx::query_scalar("SELECT id FROM routes ORDER BY id ASC LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("演示数据应包含路线");
    sqlx::query("UPDATE routes SET route_data = ?1 WHERE id = ?2")
        .bind(
            r#"{"schemaVersion":2,"points":[{"name":"P1","x":335,"y":120},
                {"name":"P2","x":555,"y":145}],"legs":[],"returnLeg":null}"#,
        )
        .bind(route_id)
        .execute(&state.db)
        .await
        .unwrap();

    // 坏任务先建（id 小 → 先被扫到），好任务在后
    let bad = insert_periodic(
        &state,
        "TK-BAD-1",
        "坏路线计划",
        Some(route_id),
        &clock(-5),
        &clock(30),
    )
    .await;
    let good = insert_periodic(&state, "TK-GOOD-1", "好计划", None, &clock(-5), &clock(30)).await;

    let report = schedule::sweep(&state)
        .await
        .expect("一条坏计划不能把扫描器打断");
    assert!(report.dispatched >= 1, "排在坏计划后面的任务应该照常下发");

    let (bad_status, bad_dispatched) = task_row(&state, bad).await;
    assert_eq!(bad_status, "待执行", "发不出去就保持待执行，窗口内继续重试");
    assert_eq!(bad_dispatched, None);
    let (good_status, good_dispatched) = task_row(&state, good).await;
    assert_eq!(good_status, "执行中", "好计划不能被坏数据挡住");
    assert_eq!(good_dispatched.as_deref(), Some(today().as_str()));

    // 事件流里要写清「下发失败 + 原因」，控制台不用上服务器翻日志
    let reason: Option<String> = sqlx::query_scalar(
        "SELECT description FROM task_events
          WHERE task_id = ?1 AND event_type = 'schedule_wait' ORDER BY id DESC LIMIT 1",
    )
    .bind(bad)
    .fetch_optional(&state.db)
    .await
    .unwrap();
    let reason = reason.expect("下发失败要留一条等待事件");
    assert!(
        reason.contains("下发失败"),
        "事件要写明下发失败及原因，实际：{reason}"
    );

    // 设备侧真的收到了好计划
    let frame = next_msg_type(&mut ws, "task_dispatch").await;
    assert_eq!(frame["msgType"], "task_dispatch");
}

/// 窗口还早：建任务不下发，扫描器也不下发，事件流里写明「到点自动下发」。
#[tokio::test]
async fn future_window_is_not_dispatched() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    let id = create_scheduled(&state, "A区上午例行巡检", &clock(90), &clock(120)).await;

    // sweep 走一遍：窗口没到，什么都不该发生
    let report = schedule::sweep(&state).await.expect("扫描失败");
    assert_eq!(report.dispatched, 0, "没到窗口不能下发");

    assert!(
        next_msg(&mut ws, "task_dispatch", Duration::from_millis(600))
            .await
            .is_none(),
        "定时巡检在窗口之外不能推给设备"
    );
    let (status, dispatched_on) = task_row(&state, id).await;
    assert_eq!(status, "待执行");
    assert_eq!(dispatched_on, None);

    let hint: Option<String> = sqlx::query_scalar(
        "SELECT description FROM task_events
          WHERE task_id = ?1 AND event_type = 'schedule' ORDER BY id ASC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .unwrap();
    let hint = hint.expect("建任务时应写一条排期事件");
    assert!(
        hint.contains("计划窗口"),
        "排期事件要写明计划窗口，实际：{hint}"
    );
}

/// 窗口覆盖当前时刻：扫描器到点下发，并记下今天已经跑过。
#[tokio::test]
async fn due_window_is_dispatched_once_per_day() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    let id = create_scheduled(&state, "B区临时加密巡检", &clock(-5), &clock(30)).await;

    // 建任务时窗口已覆盖当前时刻 → 直接下发
    let frame = next_msg_type(&mut ws, "task_dispatch").await;
    assert_eq!(frame["msgType"], "task_dispatch");
    let (status, dispatched_on) = task_row(&state, id).await;
    assert_eq!(status, "执行中", "窗口内的定时巡检应立即下发");
    assert_eq!(dispatched_on.as_deref(), Some(today().as_str()));

    // 同一天再扫一轮：不会重复下发（last_dispatched_on 挡住）
    let report = schedule::sweep(&state).await.expect("扫描失败");
    assert_eq!(report.dispatched, 0, "同一个窗口内不能重复下发");
}

/// 草稿计划：窗口内也不下发（建计划 / 扫描器 / 补发三条自动路径都认发布状态），
/// 调用发布接口之后才进入自动调度。
#[tokio::test]
async fn draft_plan_is_not_dispatched_until_published() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    // 窗口覆盖当前时刻的草稿：以前这种情况建好就推送了
    let id =
        create_scheduled_with(&state, "D区草稿计划", &clock(-5), &clock(30), Some("草稿")).await;
    assert!(
        next_msg(&mut ws, "task_dispatch", Duration::from_millis(600))
            .await
            .is_none(),
        "草稿计划不能建好就下发"
    );

    let report = schedule::sweep(&state).await.expect("扫描失败");
    assert_eq!(report.dispatched, 0, "草稿不参与自动调度");
    assert_eq!(report.missed, 0, "草稿不能报「窗口未执行」");
    let (status, dispatched_on) = task_row(&state, id).await;
    assert_eq!(status, "待执行");
    assert_eq!(dispatched_on, None);

    // 事件流里要写清为什么没跑（否则控制台表现就是「到点了什么都不发生」）
    let hint: Option<String> = sqlx::query_scalar(
        "SELECT description FROM task_events
          WHERE task_id = ?1 AND event_type = 'schedule' ORDER BY id ASC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .unwrap();
    let hint = hint.expect("建草稿时应写一条说明事件");
    assert!(hint.contains("草稿"), "事件要说清是草稿，实际：{hint}");

    // 发布（走真实接口：时间窗 / 路线坐标校验一并回归）
    let published = handlers::tasks::publish_task(
        axum::extract::State(state.clone()),
        axum::extract::Path(id),
        axum::Extension(Claims {
            sub: 1,
            username: "admin".to_string(),
            role: "admin".to_string(),
            exp: 0,
            zentai_user_id: None,
            zentai_username: None,
            is_admin: true,
        }),
    )
    .await
    .expect("发布应成功")
    .0
    .data;
    assert_eq!(published.publish_status, "已发布");

    // 发布后由扫描器接管（真实部署里 30s 一轮）
    let report = schedule::sweep(&state).await.expect("扫描失败");
    assert_eq!(report.dispatched, 1, "已发布的计划到点就该下发");
    let frame = next_msg_type(&mut ws, "task_dispatch").await;
    assert_eq!(frame["msgType"], "task_dispatch");
    let (status, dispatched_on) = task_row(&state, id).await;
    assert_eq!(status, "执行中");
    assert_eq!(dispatched_on.as_deref(), Some(today().as_str()));
}

/// 窗口已过：不是立刻下发，而是记一次「未执行」告警，任务留在待执行等下一个执行日。
#[tokio::test]
async fn missed_window_is_reported_once_and_keeps_waiting() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    // 直接插一条「早就排好、今天 00:00-00:01 窗口已经过去」的周期任务：
    // 用固定窗口而不是 `now - 2h`，避免跨零点时窗口语义变化导致用例随时钟漂移。
    let pk = device_pk(&state).await;
    let park: i64 = sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
        .bind(pk)
        .fetch_one(&state.db)
        .await
        .unwrap();
    let created = (chrono::Local::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, level, device_id,
            park_id, plan_start, plan_end, task_status, progress, created_at, updated_at)
         VALUES ('TK-MISS-1', 'C区夜巡', '定时巡检', '周期任务', '常规', ?1, ?2, '00:00', '00:01',
            '待执行', 0, ?3, ?3) RETURNING id",
    )
    .bind(pk)
    .bind(park)
    .bind(&created)
    .fetch_one(&state.db)
    .await
    .expect("插入任务失败");

    let first = schedule::sweep(&state).await.expect("扫描失败");
    assert_eq!(first.missed, 1, "窗口过了没跑成要告警一次");
    let second = schedule::sweep(&state).await.expect("扫描失败");
    assert_eq!(second.missed, 0, "同一天不能重复告警");

    assert!(
        next_msg(&mut ws, "task_dispatch", Duration::from_millis(400))
            .await
            .is_none(),
        "窗口已过的任务不能被扫描器补发"
    );
    let (status, dispatched_on) = task_row(&state, id).await;
    assert_eq!(status, "待执行", "周期计划不能因为一天没跑成就报废");
    assert_eq!(dispatched_on, None);

    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_events WHERE task_id = ?1 AND event_type = 'schedule_miss'",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(events, 1);
}

/// 周期任务必须带合法的计划时间段：手填 / 中台接口也挡在后端。
#[tokio::test]
async fn periodic_task_requires_window() {
    let (state, _db) = test_state().await;
    let pk = device_pk(&state).await;
    let park: i64 = sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
        .bind(pk)
        .fetch_one(&state.db)
        .await
        .unwrap();

    for (start, end) in [
        (None, None),
        (Some("09:30"), None),
        (Some("09:30"), Some("09:30")),
        (Some("9点"), Some("10:10")),
    ] {
        let error = handlers::tasks::create_task_core(
            &state,
            TaskRequest {
                task_code: None,
                task_name: "缺时间".to_string(),
                task_type: Some("定时巡检".to_string()),
                execute_mode: Some("周期任务".to_string()),
                end_task: None,
                light_night_only: None,
                level: None,
                device_id: Some(pk),
                park_id: Some(park),
                zone_id: None,
                route_id: Some(None),
                plan_start: start.map(str::to_string),
                plan_end: end.map(str::to_string),
                schedule_days: None,
                publish_status: None,
                task_status: None,
                progress: None,
                dispatch_policy: None,
                description: None,
            },
            None,
        )
        .await
        .expect_err("周期任务缺时间段应被拒绝");
        // 文案要指导用户填时间（「必须选择时间段」或「开始/结束不能相同」）
        let text = format!("{error}");
        assert!(
            text.contains("时间段"),
            "错误文案要指导用户填时间，实际：{text}"
        );
    }
}

/// 到点但设备没就绪：控制台能看到「到点未下发」+ 原因，且不会每 30s 刷一条。
#[tokio::test]
async fn waiting_for_device_is_visible_in_events() {
    let (state, _db) = test_state().await;
    // 设备本体没连上（真实设备掉线后的状态）：门禁会挡住下发
    sqlx::query(
        "UPDATE devices SET run_mode = 'robot', body_online = 0, last_heartbeat = ?1
         WHERE device_id = ?2",
    )
    .bind(
        (chrono::Local::now() - chrono::Duration::seconds(600))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
    )
    .bind(DEVICE_CODE)
    .execute(&state.db)
    .await
    .unwrap();

    // 用固定任务号：`WAIT_STATES` 是进程级状态，别和别的用例抢同一个 key
    let pk = device_pk(&state).await;
    let park: i64 = sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
        .bind(pk)
        .fetch_one(&state.db)
        .await
        .unwrap();
    let stamp = chrono::Local::now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, level, device_id,
            park_id, plan_start, plan_end, task_status, progress, created_at, updated_at)
         VALUES ('TK-WAIT-1', 'E区等待设备巡检', '定时巡检', '周期任务', '常规', ?1, ?2, ?3, ?4,
            '待执行', 0, ?5, ?5) RETURNING id",
    )
    .bind(pk)
    .bind(park)
    .bind(clock(-5))
    .bind(clock(30))
    .bind(stamp.format("%Y-%m-%d %H:%M:%S").to_string())
    .fetch_one(&state.db)
    .await
    .expect("插入任务失败");

    let report = schedule::sweep(&state).await.expect("扫描失败");
    assert_eq!(report.dispatched, 0, "设备没就绪不能下发");

    let (title, description): (String, String) = sqlx::query_as(
        "SELECT title, description FROM task_events
          WHERE task_id = ?1 AND event_type = 'schedule_wait'",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .expect("到点没发出去必须留一条可见事件");
    assert!(title.contains("到点未下发"), "标题要说明没下发：{title}");
    assert!(
        description.contains("已重试 1 次"),
        "首次等待事件要带重试次数：{description}"
    );
    assert!(
        description.contains("本体未连接"),
        "原因要写清是本体没连上，运维才知道去查客户端：{description}"
    );

    // 再扫两轮：30s 一轮不节流的话事件流会被刷爆，这里必须还是 1 条
    schedule::sweep(&state).await.expect("扫描失败");
    schedule::sweep(&state).await.expect("扫描失败");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_events WHERE task_id = ?1 AND event_type = 'schedule_wait'",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(count, 1, "同一个窗口内只能有一条等待事件（描述可刷新）");
}

/// 跑完的周期任务在下一个执行日自动回到待执行，等下一个窗口。
#[tokio::test]
async fn finished_periodic_task_rearms_next_day() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let _ws = register_device(port).await;

    let id = create_scheduled(&state, "D区白天巡检", &clock(60), &clock(90)).await;
    // 昨天的窗口跑完了
    sqlx::query(
        "UPDATE tasks SET task_status = '已完成', progress = 100, last_dispatched_on = ?1
         WHERE id = ?2",
    )
    .bind(
        (chrono::Local::now() - chrono::Duration::days(1))
            .format("%Y-%m-%d")
            .to_string(),
    )
    .bind(id)
    .execute(&state.db)
    .await
    .unwrap();

    let report = schedule::sweep(&state).await.expect("扫描失败");
    assert_eq!(report.rearmed, 1);
    let (status, dispatched_on) = task_row(&state, id).await;
    assert_eq!(status, "待执行", "周期任务要回到待执行等下一次");
    // 昨天那次的时间戳保留：它既是历史，也是「今天还没跑」的依据
    assert!(dispatched_on.is_some());
}
