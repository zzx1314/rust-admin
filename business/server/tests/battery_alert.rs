//! 低电量告警 → 企业微信群 的回归测试。
//!
//! 现场诉求：狗电量掉到阈值（设置页「平台参数 → 低电量阈值」，默认 20%）以下，
//! 要在企业微信群里出现一条，值班的人安排回充。
//!
//! 这个用例守五条：
//! 1. **闲置也推**：设备没有 `执行中` 的任务时照样建单 + 推群（`battery:` 键不走
//!    `alert_only_in_task` 那道门）——等「执行计划期间」再报就来不及了；
//! 2. **一轮只一条**：心跳 10s 一次，低电量期间不能每帧一条告警 / 一条群消息；
//! 3. **充电 / 电量回升自动结案**：结完案下一次真低电量还能再报（边沿触发）；
//! 4. **恢复余量**：电量刚回到阈值上方不结案，避免阈值边缘来回刷；
//! 5. **假数据不报警**：模拟模式 / 本体掉线 / 还没绑园区都不落告警。

use axum::Router;
use futures_util::{SinkExt, StreamExt};
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::notify::{Notice, Notifier};
use robodog_ops_server::{db, gateway, seed};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const DEVICE_CODE: &str = "RD-BAT-1";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct TempDb {
    dir: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-{tag}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

fn test_config(dir: &Path) -> Config {
    Config {
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
    }
}

/// 有 webhook 但**不启动消费端**：只看「入没入队」，不发外网。
async fn test_state() -> (AppState, mpsc::Receiver<Notice>, TempDb) {
    let dir = temp_dir("battery-alert");
    let temp = TempDb { dir: dir.clone() };
    let config = Arc::new(test_config(&dir));
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
        phone_audio: Default::default(),
        ai_patrol: Default::default(),
        caches: Default::default(),
    };
    (state, rx.expect("部署开关开着时应有消费端"), temp)
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

async fn register_device(port: u16, code: &str) -> DeviceWs {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device"))
        .await
        .expect("设备连不上网关");
    ws.send(Message::Text(
        json!({
            "msgType": "register",
            "ts": "2026-01-01 00:00:00",
            "payload": {
                "deviceId": code,
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

/// 上报一帧心跳。电量 / 充电 / 本体在线 / 模式都由调用方决定。
async fn heartbeat(
    ws: &mut DeviceWs,
    battery: i64,
    charging: Option<bool>,
    body_online: Option<bool>,
    mode: Option<&str>,
) {
    let mut payload = json!({ "battery": battery });
    if let Some(charging) = charging {
        payload["charging"] = json!(charging);
    }
    if let Some(body_online) = body_online {
        payload["bodyOnline"] = json!(body_online);
    }
    if let Some(mode) = mode {
        payload["mode"] = json!(mode);
    }
    ws.send(Message::Text(
        json!({
            "msgType": "heartbeat",
            "ts": "2026-01-01 00:00:00",
            "payload": payload,
        })
        .to_string(),
    ))
    .await
    .expect("发送心跳失败");
    // 心跳回执在处理完之后才发，等到它就能保证库已写完（含告警与推送入队）
    next_msg(ws, "heartbeat_ack").await;
}

/// 注册后把设备绑到某个园区（未绑定园区的设备不报低电量）。
async fn bind_park(state: &AppState, code: &str) {
    sqlx::query(
        "UPDATE devices SET park_id = (SELECT id FROM parks ORDER BY id ASC LIMIT 1)
          WHERE device_id = ?1",
    )
    .bind(code)
    .execute(&state.db)
    .await
    .expect("绑定园区失败");
}

/// 该设备的低电量告警（`AL-BAT-*`）：`(status, content, dedup_key)`。
async fn battery_alerts(state: &AppState, code: &str) -> Vec<(String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT status, content, dedup_key FROM alerts
          WHERE alert_code LIKE 'AL-BAT-%' AND device_id = (SELECT id FROM devices WHERE device_id = ?1)
          ORDER BY id ASC",
    )
    .bind(code)
    .fetch_all(&state.db)
    .await
    .expect("查低电量告警失败")
}

async fn auto_order_count(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM work_orders WHERE source = '告警自动派单'")
        .fetch_one(&state.db)
        .await
        .expect("查工单失败")
}

/// 闲置设备低电量：照样建单 + 进群（`battery:` 键不看「执行计划期间」）。
#[tokio::test]
async fn idle_device_low_battery_creates_order_and_pushes() {
    let (state, mut rx, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;
    bind_park(&state, DEVICE_CODE).await;

    // 播种数据里这台设备没有任务，先确认它不在执行计划（否则测的就不是「闲置也推」）
    let running: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks
          WHERE task_status = '执行中' AND device_id = (SELECT id FROM devices WHERE device_id = ?1)",
    )
    .bind(DEVICE_CODE)
    .fetch_one(&state.db)
    .await
    .expect("查任务失败");
    assert_eq!(running, 0);

    heartbeat(&mut ws, 12, Some(false), Some(true), Some("robot")).await;

    let alerts = battery_alerts(&state, DEVICE_CODE).await;
    assert_eq!(alerts.len(), 1, "低电量应落一条告警: {alerts:?}");
    assert_eq!(alerts[0].0, "未处理");
    assert!(alerts[0].1.contains("12%"), "{}", alerts[0].1);
    assert_eq!(alerts[0].2.as_deref(), Some("battery:RD-BAT-1"));

    let notice = rx.try_recv().expect("低电量应推企业微信（闲置也要推）");
    assert!(
        notice.title.contains("低电量") && notice.title.contains("已自动派单"),
        "低电量消息应带专用标题与派单结论: {}",
        notice.title
    );
    assert_eq!(auto_order_count(&state).await, 1, "低电量应建一张工单");
}

/// 低电量期间每 10s 一帧心跳：只能有一条告警、一条群消息。
#[tokio::test]
async fn repeated_low_battery_heartbeats_only_alert_once() {
    let (state, mut rx, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;
    bind_park(&state, DEVICE_CODE).await;

    for battery in [19, 15, 11, 8] {
        heartbeat(&mut ws, battery, Some(false), Some(true), Some("robot")).await;
    }

    let alerts = battery_alerts(&state, DEVICE_CODE).await;
    assert_eq!(alerts.len(), 1, "低电量连刷不能每帧一条: {alerts:?}");
    assert_eq!(alerts[0].0, "未处理");
    assert!(rx.try_recv().is_ok(), "第一条应进群");
    assert!(rx.try_recv().is_err(), "后续心跳不该再进群");
    assert_eq!(auto_order_count(&state).await, 1, "工单只建一张");
}

/// 接入充电桩 → 自动结案；拔掉充电桩再低电量 → 重新报一条。
#[tokio::test]
async fn charging_resolves_and_low_battery_rearms() {
    let (state, mut rx, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;
    bind_park(&state, DEVICE_CODE).await;

    heartbeat(&mut ws, 12, Some(false), Some(true), Some("robot")).await;
    assert_eq!(battery_alerts(&state, DEVICE_CODE).await.len(), 1);
    assert!(rx.try_recv().is_ok());

    // 插上充电桩：低电量是正常状态，自动结案（不再推一条「恢复」消息）
    heartbeat(&mut ws, 12, Some(true), Some(true), Some("robot")).await;
    let alerts = battery_alerts(&state, DEVICE_CODE).await;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].0, "已处理", "充电后应自动结案");
    assert!(rx.try_recv().is_err(), "结案不该推群");

    // 拔掉充电桩、电量又掉到阈值以下：边沿触发，重新报一条
    heartbeat(&mut ws, 12, Some(false), Some(true), Some("robot")).await;
    let alerts = battery_alerts(&state, DEVICE_CODE).await;
    assert_eq!(alerts.len(), 2, "结案后重新低电量应再报一条: {alerts:?}");
    assert_eq!(alerts[1].0, "未处理");
}

/// 电量刚回到阈值上方（未超过恢复余量）不结案：避免阈值边缘来回刷。
#[tokio::test]
async fn recovery_needs_a_margin_above_the_threshold() {
    let (state, mut rx, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;
    bind_park(&state, DEVICE_CODE).await;

    heartbeat(&mut ws, 18, Some(false), Some(true), Some("robot")).await;
    assert!(rx.try_recv().is_ok());

    // 阈值 20 + 余量 5 = 25：22% 还在余量内，保持未处理
    heartbeat(&mut ws, 22, Some(false), Some(true), Some("robot")).await;
    assert_eq!(battery_alerts(&state, DEVICE_CODE).await[0].0, "未处理");
    assert!(rx.try_recv().is_err());

    // 27% 超过余量：结案
    heartbeat(&mut ws, 27, Some(false), Some(true), Some("robot")).await;
    assert_eq!(battery_alerts(&state, DEVICE_CODE).await[0].0, "已处理");
}

/// 阈值读设置页的 `battery_threshold`（改完下一帧心跳就生效，不需要重启）。
#[tokio::test]
async fn threshold_comes_from_settings() {
    let (state, mut rx, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;
    bind_park(&state, DEVICE_CODE).await;

    sqlx::query("UPDATE settings SET setting_value = '30' WHERE setting_key = 'battery_threshold'")
        .execute(&state.db)
        .await
        .expect("改阈值失败");

    // 28%：默认 20% 时不算低电量，改成 30% 后应报警
    heartbeat(&mut ws, 28, Some(false), Some(true), Some("robot")).await;
    let alerts = battery_alerts(&state, DEVICE_CODE).await;
    assert_eq!(alerts.len(), 1, "28% 在 30% 阈值下应报警: {alerts:?}");
    assert!(alerts[0].1.contains("30%"), "{}", alerts[0].1);
    assert!(rx.try_recv().is_ok());
}

/// 假数据 / 未绑定园区不报警：模拟模式、本体掉线、还没绑园区。
#[tokio::test]
async fn simulated_offline_and_unbound_devices_are_ignored() {
    let (state, mut rx, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;
    bind_park(&state, DEVICE_CODE).await;

    // 模拟模式：电量是假的
    heartbeat(&mut ws, 5, Some(false), Some(true), Some("sim")).await;
    // 本体掉线：电量是上一帧的残留值
    heartbeat(&mut ws, 5, None, Some(false), Some("robot")).await;
    assert!(
        battery_alerts(&state, DEVICE_CODE).await.is_empty(),
        "模拟模式 / 本体掉线不该报警"
    );
    assert!(rx.try_recv().is_err());

    // 未绑定园区：没有责任人可派，也不报警
    let mut other = register_device(port, "RD-BAT-2").await;
    heartbeat(&mut other, 5, Some(false), Some(true), Some("robot")).await;
    assert!(
        battery_alerts(&state, "RD-BAT-2").await.is_empty(),
        "未绑定园区的设备不该报警"
    );
    assert!(rx.try_recv().is_err());
}
