//! 控制权被遥控器抢占（`1315 ctrl_source = 3`）的留痕回归测试。
//!
//! 现场问题：运维在业务控制台看到设备「在线待命、电量正常」，摇杆推不动、任务也下发不了，
//! 但因为本体控制权在硬件遥控器手里，平台只能干等——没有任何提示说明「谁把控制权拿走了」。
//!
//! 现在心跳里 `ctrlSource` 由 1/2 变成 3 时：
//! 1. 落一条「未处理 / 警告」告警（业务控制台轮询后弹窗提示）；
//! 2. 事件流记一条 `device_event`；
//! 3. 持续为 3 不重复写告警（遥控器时断时续不刷屏）；
//! 4. 控制权回到 APP / SDK 时自动把告警结掉。

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
        "robodog-ops-ctrl-source-test-{}-{}",
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

async fn register_device(port: u16, device_id: &str) -> DeviceWs {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device"))
        .await
        .expect("设备连不上网关");
    ws.send(Message::Text(
        json!({
            "msgType": "register",
            "ts": "2026-01-01 00:00:00",
            "payload": {
                "deviceId": device_id,
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

/// 上报一帧心跳（`ctrlSource` 为 1315 `ctrl_source`）
async fn heartbeat(ws: &mut DeviceWs, ctrl_source: i64) {
    ws.send(Message::Text(
        json!({
            "msgType": "heartbeat",
            "ts": "2026-01-01 00:00:00",
            "payload": {
                "battery": 80,
                "bodyOnline": true,
                "mode": "robot",
                "ctrlSource": ctrl_source,
                "robotMode": "remote",
                "motionStatus": "stand",
            },
        })
        .to_string(),
    ))
    .await
    .expect("发送心跳失败");
    // 心跳回执在处理完之后才发，等到它就能保证库已写完
    next_msg(ws, "heartbeat_ack").await;
}

async fn ctrl_source_of(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT ctrl_source FROM devices WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .fetch_one(&state.db)
        .await
        .expect("设备应在台账里")
}

/// 该设备的控制权抢占告警（`AL-CTRL-*`）：返回 (总数, 未处理数)
async fn preempt_alerts(state: &AppState) -> (i64, i64) {
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM alerts WHERE alert_code LIKE 'AL-CTRL-%'
           AND device_id = (SELECT id FROM devices WHERE device_id = ?1)",
    )
    .bind(DEVICE_CODE)
    .fetch_one(&state.db)
    .await
    .unwrap();
    let pending: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM alerts WHERE alert_code LIKE 'AL-CTRL-%' AND status = '未处理'
           AND device_id = (SELECT id FROM devices WHERE device_id = ?1)",
    )
    .bind(DEVICE_CODE)
    .fetch_one(&state.db)
    .await
    .unwrap();
    (total, pending)
}

async fn control_events(state: &AppState) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_events WHERE title IN ('控制权被遥控器抢占', '控制权已收回')
           AND device_id = (SELECT id FROM devices WHERE device_id = ?1)",
    )
    .bind(DEVICE_CODE)
    .fetch_one(&state.db)
    .await
    .unwrap()
}

#[tokio::test]
async fn remote_takeover_raises_alert_once_and_resolves_when_released() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;

    // 基线：控制权在 APP，不该有任何抢占告警
    heartbeat(&mut ws, 1).await;
    assert_eq!(ctrl_source_of(&state).await, 1);
    assert_eq!(preempt_alerts(&state).await, (0, 0), "常规控制不应告警");

    // 遥控器抢走控制权 → 一条未处理告警
    heartbeat(&mut ws, 3).await;
    assert_eq!(ctrl_source_of(&state).await, 3);
    assert_eq!(preempt_alerts(&state).await, (1, 1), "抢占应落一条未处理告警");

    let (level, content): (String, String) = sqlx::query_as(
        "SELECT level, content FROM alerts WHERE alert_code LIKE 'AL-CTRL-%'
           AND device_id = (SELECT id FROM devices WHERE device_id = ?1)",
    )
    .bind(DEVICE_CODE)
    .fetch_one(&state.db)
    .await
    .unwrap();
    assert_eq!(level, "警告");
    assert!(
        content.contains("ctrl_source=3") && content.contains(DEVICE_CODE),
        "告警文案要写清设备与被抢占原因: {content}"
    );

    // 持续为 3 不再重复告警（心跳 1Hz，不能刷屏）
    heartbeat(&mut ws, 3).await;
    heartbeat(&mut ws, 3).await;
    assert_eq!(
        preempt_alerts(&state).await,
        (1, 1),
        "同一个抢占过程只保留一条未处理告警"
    );

    // 控制权回到 SDK → 告警自动结掉
    heartbeat(&mut ws, 2).await;
    assert_eq!(ctrl_source_of(&state).await, 2);
    assert_eq!(
        preempt_alerts(&state).await,
        (1, 0),
        "控制权收回后告警应自动置为已处理"
    );
    let handled: Option<String> =
        sqlx::query_scalar("SELECT handled_at FROM alerts WHERE alert_code LIKE 'AL-CTRL-%'")
            .fetch_one(&state.db)
            .await
            .unwrap();
    assert!(handled.is_some(), "自动结案要写 handled_at");

    // 事件流留痕：1→3、3→2 两次变化
    assert_eq!(control_events(&state).await, 2);
}

#[tokio::test]
async fn flapping_remote_takeover_records_each_transition() {
    let (state, _db) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;

    // 3 → 1 → 3：第一次抢占在 3→1 时已结案，第二次抢占是新的一条
    heartbeat(&mut ws, 3).await;
    heartbeat(&mut ws, 1).await;
    heartbeat(&mut ws, 3).await;
    assert_eq!(preempt_alerts(&state).await, (2, 1), "每次抢占各自一条，只有最新未处理");
    assert_eq!(control_events(&state).await, 3, "三次变化都进事件流");
}
