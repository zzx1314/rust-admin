//! 告警白名单（`settings.alert_ignore_fault_codes`）回归。
//!
//! 现场口径：本体有些高频故障码只是噪音，报出来既不值得派人处理，也不该进企业微信群。
//! 典型两条（2026-11 现场反馈）：
//!
//! - `12608` RS 与 lidar 时间不同步（级别 1）：机器人每 ~90s 报一次；
//! - `13440` 导航指令非法（级别 2）：一定跟着 `13897 capability busy` 一起出现，
//!   真正要处置的是根因那条。
//!
//! 命中白名单的故障**不落 `alerts`、不建工单、不推企业微信**，
//! 只在 `task_events` 事件流里留一行（排查时还能证明「本体确实报过」）。
//!
//! 这里走真实设备 WS：造一台**正在执行计划**的设备（没被挡住就必然建单 + 推群），
//! 再上行 `event` 帧，白名单码与非白名单码两侧都钉住。

use futures_util::SinkExt;
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::notify::Notifier;
use robodog_ops_server::{alert_flow, db, gateway, seed};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
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

/// 有 webhook 但**不启动消费端**：只看「入没入队」，不发外网。
async fn build_state() -> (
    AppState,
    mpsc::Receiver<robodog_ops_server::notify::Notice>,
    TempDb,
) {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-whitelist-{}-{}",
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
        service_token: None,
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
        wecom_enabled: true,
        wecom_webhook: Some(
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=test-key".to_string(),
        ),
        wecom_min_level: "警告".to_string(),
        wecom_mention_all: false,
    });
    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");
    let (notifier, rx) = Notifier::channel(&config, None);
    let state = AppState {
        db: pool,
        notifier,
        config,
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
        ai_patrol: Default::default(),
    };
    (state, rx.expect("部署开关开着时应有消费端"), temp)
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

async fn next_msg(ws: &mut DeviceWs, msg_type: &str) -> Value {
    let deadline = Duration::from_secs(5);
    tokio::time::timeout(deadline, async {
        use futures_util::StreamExt;
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
    let ack = next_msg(&mut ws, "register_ack").await;
    assert_eq!(ack["payload"]["success"], true, "注册应成功: {ack}");
    ws
}

/// 把设备挂到一条 `执行中` 的任务上：此时任何没被挡住的告警都必须建单 + 推群。
async fn put_device_in_task(state: &AppState) {
    let device_pk: i64 = sqlx::query_scalar("SELECT id FROM devices WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .fetch_one(&state.db)
        .await
        .expect("设备应在台账里");
    sqlx::query(
        "UPDATE tasks SET device_id = ?1, task_status = '执行中'
          WHERE id = (SELECT MIN(id) FROM tasks)",
    )
    .bind(device_pk)
    .execute(&state.db)
    .await
    .expect("造执行中任务失败");
}

/// 上行一帧设备事件，等 `event_ack` 确认服务端处理完。
async fn send_event(ws: &mut DeviceWs, event_type: &str, message: &str, severity: i64) {
    let frame = json!({
        "msgType": "event",
        "ts": "2026-01-01 00:00:00",
        "payload": {
            "eventType": event_type,
            "message": message,
            "data": { "severity": severity },
        },
    });
    ws.send(Message::Text(frame.to_string()))
        .await
        .expect("发送 event 失败");
    next_msg(ws, "event_ack").await;
}

async fn alert_count(state: &AppState, like: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE content LIKE ?1")
        .bind(like)
        .fetch_one(&state.db)
        .await
        .expect("查告警失败")
}

async fn auto_order_count(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM work_orders WHERE source = '告警自动派单'")
        .fetch_one(&state.db)
        .await
        .expect("查工单失败")
}

async fn device_event_count(state: &AppState, like: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_events WHERE event_type = 'device_event' AND description LIKE ?1",
    )
    .bind(like)
    .fetch_one(&state.db)
    .await
    .expect("查事件失败")
}

async fn set_whitelist(state: &AppState, value: &str) {
    sqlx::query("UPDATE settings SET setting_value = ?1 WHERE setting_key = ?2")
        .bind(value)
        .bind(alert_flow::IGNORE_CODES_SETTING)
        .execute(&state.db)
        .await
        .expect("改白名单失败");
}

/// 默认配置下（白名单 = `12608,13440`）：这两条高频故障既不落告警、也不建单、也不推群。
#[tokio::test]
async fn whitelisted_fault_codes_are_not_recorded_or_pushed() {
    let (state, mut rx, _temp) = build_state().await;
    put_device_in_task(&state).await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    send_event(
        &mut ws,
        "fault",
        "RS 与 lidar 时间不同步（code=12608, 级别=1）",
        1,
    )
    .await;
    send_event(&mut ws, "fault", "导航指令非法（code=13440, 级别=2）", 2).await;

    assert_eq!(
        alert_count(&state, "%code=12608%").await,
        0,
        "12608 不该落告警"
    );
    assert_eq!(
        alert_count(&state, "%code=13440%").await,
        0,
        "13440 不该落告警"
    );
    assert_eq!(auto_order_count(&state).await, 0, "白名单故障不该建单");
    assert!(rx.try_recv().is_err(), "白名单故障不该推企业微信");
    // 事件流照常留痕：排查时要能证明「本体确实报过」
    assert_eq!(device_event_count(&state, "%code=12608%").await, 1);
    assert_eq!(device_event_count(&state, "%code=13440%").await, 1);
}

/// 不在白名单里的故障码照旧：落告警 + 建单 + 推群（这是白名单的反向钉子）。
#[tokio::test]
async fn other_fault_codes_still_alert_and_push() {
    let (state, mut rx, _temp) = build_state().await;
    put_device_in_task(&state).await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    send_event(
        &mut ws,
        "fault",
        "感知障碍物检测延迟（code=13382, 级别=1）",
        1,
    )
    .await;

    assert_eq!(
        alert_count(&state, "%code=13382%").await,
        1,
        "13382 应照常落告警"
    );
    assert_eq!(auto_order_count(&state).await, 1, "执行计划期间应自动建单");
    assert!(rx.try_recv().is_ok(), "执行计划期间应推企业微信");
}

/// 白名单改成别的码，12608 就恢复告警：配置是真源，不是写死在代码里的黑名单。
#[tokio::test]
async fn whitelist_is_configurable_at_runtime() {
    let (state, mut rx, _temp) = build_state().await;
    put_device_in_task(&state).await;
    set_whitelist(&state, "99999").await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port).await;

    send_event(
        &mut ws,
        "fault",
        "RS 与 lidar 时间不同步（code=12608, 级别=1）",
        1,
    )
    .await;

    assert_eq!(
        alert_count(&state, "%code=12608%").await,
        1,
        "移出白名单后应落告警"
    );
    assert!(rx.try_recv().is_ok(), "移出白名单后应推企业微信");

    // 清空白名单 = 全部告警照旧（不能因为「没配」就静默掉）
    set_whitelist(&state, "").await;
    assert!(
        !alert_flow::fault_code_ignored(&state.db, "12608").await,
        "空白名单不该命中任何码"
    );
}

/// 没有故障码的事件（人工事件 / 翻译文案）不受白名单影响。
#[tokio::test]
async fn event_without_code_is_never_whitelisted() {
    let (state, _rx, _temp) = build_state().await;
    assert!(
        !alert_flow::message_ignored(&state.db, "The localization system has poor accuracy").await,
        "取不到码的文案不能因为白名单被静默"
    );
    assert!(alert_flow::message_ignored(&state.db, "任意文案（code=12608, 级别=1）").await);
    assert!(!alert_flow::message_ignored(&state.db, "任意文案（code=13382, 级别=1）").await);
}
