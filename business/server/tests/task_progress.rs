//! 任务进度上报的回归测试。
//!
//! 现场事故（TK-1112-12）：客户端定位失败后先回了一条「异常」，但它没有清掉本地
//! 任务上下文，进度定时器继续每 5s 报一次「执行中 / 0%」。服务端无条件按上报内容
//! 覆盖状态，把异常刷回执行中；同时 `updated_at` 一直是新鲜的，任务看门狗永远不触发，
//! 任务在平台上一挂就是半小时。
//!
//! 两个约束在这里钉住：
//! 1. 终态（已完成 / 异常）不能被后续「执行中」的周期上报拉回去；
//! 2. 还在上报但进度长时间不推进（`progress_updated_at` 很旧）也要判异常。

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
        "robodog-ops-progress-test-{}-{}",
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
    });

    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");

    let state = AppState {
        db: pool,
        config,
        device_connections: gateway::new_device_connections(),
        video_hub: Default::default(),
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

async fn register_device(port: u16, device_id: &str) -> DeviceWs {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device"))
        .await
        .expect("设备连不上网关");
    let register = json!({
        "msgType": "register",
        "ts": "2026-01-01 00:00:00",
        "payload": {
            "deviceId": device_id,
            "secret": null,
            "model": "X30-Pro",
            "firmware": "test-fw",
        },
    });
    ws.send(Message::Text(register.to_string()))
        .await
        .expect("发送注册帧失败");
    next_msg(&mut ws, "register_ack").await;
    ws
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

async fn device_pk(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT id FROM devices WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .fetch_one(&state.db)
        .await
        .expect("演示设备应在台账里")
}

async fn insert_task(state: &AppState, code: &str, status: &str, progress: i64) -> i64 {
    let pk = device_pk(state).await;
    let stamp = "2026-01-01 00:00:00";
    sqlx::query_scalar(
        "INSERT INTO tasks (task_code, task_name, device_id, task_status, progress,
            created_at, updated_at, progress_updated_at)
         VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?5, ?5) RETURNING id",
    )
    .bind(code)
    .bind(pk)
    .bind(status)
    .bind(progress)
    .bind(stamp)
    .fetch_one(&state.db)
    .await
    .expect("插入任务失败")
}

async fn send_progress(ws: &mut DeviceWs, task_code: &str, status: &str, progress: i64) {
    let frame = json!({
        "msgType": "task_progress",
        "ts": "2026-01-01 00:00:00",
        "payload": {
            "taskCode": task_code,
            "status": status,
            "progress": progress,
            "message": "测试上报",
        },
    });
    ws.send(Message::Text(frame.to_string()))
        .await
        .expect("发送 task_progress 失败");
    // ack 在写库之后才发，收到它就说明这次上报已经处理完
    next_msg(ws, "task_progress_ack").await;
}

async fn task_row(state: &AppState, id: i64) -> (String, i64, Option<String>) {
    sqlx::query_as("SELECT task_status, progress, progress_updated_at FROM tasks WHERE id = ?1")
        .bind(id)
        .fetch_one(&state.db)
        .await
        .expect("任务应存在")
}

/// 已完成的任务不会被迟到的「执行中」上报拉回去。
#[tokio::test]
async fn terminal_task_is_not_revived_by_late_progress() {
    let (state, _db) = test_state().await;
    let task_id = insert_task(&state, "TK-DONE-1", "已完成", 100).await;

    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;
    send_progress(&mut ws, "TK-DONE-1", "执行中", 10).await;

    let (status, progress, _) = task_row(&state, task_id).await;
    assert_eq!(status, "已完成", "终态不能被周期上报刷回执行中");
    assert_eq!(progress, 100, "终态进度不能被覆盖");
}

/// 执行中的任务正常接受进度，并刷新进度时间戳（看门狗据此判断是否停滞）。
#[tokio::test]
async fn running_task_accepts_progress_and_stamps_it() {
    let (state, _db) = test_state().await;
    let task_id = insert_task(&state, "TK-RUN-1", "执行中", 0).await;

    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;
    send_progress(&mut ws, "TK-RUN-1", "执行中", 40).await;

    let (status, progress, stamped) = task_row(&state, task_id).await;
    assert_eq!(status, "执行中");
    assert_eq!(progress, 40);
    assert!(
        stamped.as_deref() != Some("2026-01-01 00:00:00"),
        "进度变化必须刷新 progress_updated_at，实际 {stamped:?}"
    );
}

/// 还在上报但进度不推进：看门狗按「进度停滞」判异常。
#[tokio::test]
async fn stalled_progress_is_marked_abnormal() {
    let (state, _db) = test_state().await;
    let task_id = insert_task(&state, "TK-STALL-1", "执行中", 5).await;

    // 最后一次上报是刚才（updated_at 新鲜），但进度从很早以前就没动过
    let fresh = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let stale = (chrono::Local::now() - chrono::Duration::seconds(3600))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    sqlx::query("UPDATE tasks SET updated_at = ?1, progress_updated_at = ?2 WHERE id = ?3")
        .bind(&fresh)
        .bind(&stale)
        .bind(task_id)
        .execute(&state.db)
        .await
        .unwrap();
    sqlx::query("UPDATE devices SET last_heartbeat = ?1, status = '执行任务' WHERE device_id = ?2")
        .bind(&fresh)
        .bind(DEVICE_CODE)
        .execute(&state.db)
        .await
        .unwrap();

    let affected = gateway::sweep_stuck_tasks(&state).await.unwrap();
    assert_eq!(affected, 1, "只有进度停滞的那条任务会被判定");

    let (status, _, _) = task_row(&state, task_id).await;
    assert_eq!(status, "异常");

    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_events WHERE task_id = ?1 AND description LIKE '%没有任何推进%'",
    )
    .bind(task_id)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(events, 1, "判定异常时应说明是进度停滞");
}
