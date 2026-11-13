//! 设备重连后「台账在线、连接池空」的回归测试。
//!
//! 现场现象：机器狗（`0202222672400230`）在业务控制台/中台显示「在线待命」，
//! 但中台点「同步地图」报 `设备 xxx 当前未连接（状态：在线待命）`。
//!
//! 成因是连接池按 `device_id` 单键登记：客户端重连时旧 TCP 可能还没死透
//! （拔网线 / 4G 掉线 / 进程被 kill 都收不到 Close 帧），新连接注册后把池里的
//! sender 换成新的；等旧连接终于超时退出，它的清理逻辑无条件 `remove(device_id)`，
//! 摘掉的就是**仍活着的新连接**。之后设备心跳照常把台账状态刷回「在线待命」，
//! 而池里没有登记，任何下行指令都报「未连接」。
//!
//! 这里用真实 TCP + 两个 WebSocket 连接复现：
//! 1. 旧连接注册 → 新连接注册（顶替）；
//! 2. 旧连接退出后，`synced maps` 依然能下达到新连接；
//! 3. 台账状态不能被旧连接的退出改成「离线」；新连接退出后才置离线。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
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

const SERVICE_TOKEN: &str = "test-service-token";
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
        "robodog-ops-reconnect-test-{}-{}",
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
        service_token: Some(SERVICE_TOKEN.to_string()),
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

/// 扮演设备客户端：连上网关并注册，等 `register_ack`
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
    let ack = next_msg(&mut ws, "register_ack").await;
    assert_eq!(ack["payload"]["success"], true, "注册应成功: {ack}");
    ws
}

/// 等下一条指定类型的文本消息（设备通道上还有 `ping` 保活帧）
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

async fn device_status(state: &AppState, device_code: &str) -> String {
    sqlx::query_scalar("SELECT status FROM devices WHERE device_id = ?1")
        .bind(device_code)
        .fetch_one(&state.db)
        .await
        .expect("设备应在台账里")
}

/// 中台视角：带服务令牌调「同步地图」
async fn sync_maps(state: &AppState, device_code: &str) -> (StatusCode, Value) {
    let request = Request::post(format!("/api/zentai/devices/{device_code}/sync-maps"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-service-token", SERVICE_TOKEN)
        .body(Body::from(r#"{"mode":"manual"}"#))
        .unwrap();
    // 直接驱动 Router：这里只关心业务平台的响应，不需要再起一个端口
    let router = build_router(state.clone());
    let response = tower::ServiceExt::oneshot(router, request)
        .await
        .expect("请求失败");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读响应体失败");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn close(ws: DeviceWs) {
    // 关掉连接让服务端走清理分支，并给它一点时间
    drop(ws);
    tokio::time::sleep(Duration::from_millis(400)).await;
}

/// 客户端上行一帧「本地已不持有控制权」
async fn report_teleop_released(ws: &mut DeviceWs, reason: &str) {
    let frame = json!({
        "msgType": "teleop_state",
        "ts": "2026-01-01 00:00:00",
        "payload": { "holder": false, "reason": reason },
    });
    ws.send(Message::Text(frame.to_string()))
        .await
        .expect("发送 teleop_state 失败");
    next_msg(ws, "teleop_state_ack").await;
}

async fn teleop_holder(state: &AppState, device_code: &str) -> Option<String> {
    sqlx::query_scalar("SELECT teleop_holder FROM devices WHERE device_id = ?1")
        .bind(device_code)
        .fetch_one(&state.db)
        .await
        .expect("设备应在台账里")
}

/// 进程内重连本体后，业务 WS 不断 ⇒ 服务端收不到断连回调，
/// `teleop_holder` 必须靠客户端上行 `teleop_state` 清掉。
#[tokio::test]
async fn client_reported_teleop_release_clears_holder() {
    let (state, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;

    // 模拟平台已接管（正常由「远程接管」指令写入）
    sqlx::query(
        "UPDATE devices SET teleop_holder = 'platform', teleop_force = 1,
            teleop_at = '2026-01-01 00:00:00' WHERE device_id = ?1",
    )
    .bind(DEVICE_CODE)
    .execute(&state.db)
    .await
    .expect("造接管态失败");

    report_teleop_released(&mut ws, "body-link-lost").await;

    assert_eq!(
        teleop_holder(&state, DEVICE_CODE).await,
        None,
        "客户端上报释放后不得留下幽灵接管"
    );
    let force: i64 = sqlx::query_scalar("SELECT teleop_force FROM devices WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .fetch_one(&state.db)
        .await
        .expect("取 teleop_force 失败");
    assert_eq!(force, 0, "强制接管标记必须一并清掉");

    // 事件流留痕，便于运维追溯接管态为什么没了
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_events WHERE event_type = 'device_event'",
    )
    .fetch_one(&state.db)
    .await
    .expect("统计事件失败");
    assert_eq!(events, 1, "释放接管应写入一条事件");

    close(ws).await;
}

/// 设备客户端不能自己宣布「持有控制权」：holder 由平台指令写入
#[tokio::test]
async fn client_cannot_claim_teleop_holder() {
    let (state, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;

    let frame = json!({
        "msgType": "teleop_state",
        "ts": "2026-01-01 00:00:00",
        "payload": { "holder": true, "reason": "self-claimed" },
    });
    ws.send(Message::Text(frame.to_string()))
        .await
        .expect("发送 teleop_state 失败");
    next_msg(&mut ws, "teleop_state_ack").await;

    assert_eq!(
        teleop_holder(&state, DEVICE_CODE).await,
        None,
        "客户端的 holder=true 必须被忽略"
    );

    close(ws).await;
}

#[tokio::test]
async fn stale_connection_does_not_kick_the_new_registration() {
    let (state, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;

    // 1. 旧连接注册（模拟网络抖动前的那个 TCP）
    let stale = register_device(port, DEVICE_CODE).await;
    assert_eq!(device_status(&state, DEVICE_CODE).await, "在线待命");

    // 2. 客户端重连：新连接注册，顶替池里的登记
    let mut fresh = register_device(port, DEVICE_CODE).await;

    // 3. 旧连接超时退出：不得把新登记摘走，也不得把台账改成离线
    close(stale).await;
    assert_eq!(
        device_status(&state, DEVICE_CODE).await,
        "在线待命",
        "旧连接的退出不能把仍在线设备的台账状态改成离线"
    );
    assert!(
        state
            .device_connections
            .read()
            .await
            .contains_key(DEVICE_CODE),
        "旧连接退出后连接池里必须还留着新连接"
    );

    // 4. 中台「同步地图」必须能下达到新连接
    let (status, body) = sync_maps(&state, DEVICE_CODE).await;
    assert_eq!(status, StatusCode::OK, "同步地图应成功: {body}");
    assert_eq!(body["data"]["dispatched"], true);
    let command = next_msg(&mut fresh, "command").await;
    assert_eq!(command["payload"]["command"], "同步地图");

    // 5. 新连接自己退出后，才真正摘掉登记并置离线
    close(fresh).await;
    assert_eq!(device_status(&state, DEVICE_CODE).await, "离线");
    assert!(
        !state
            .device_connections
            .read()
            .await
            .contains_key(DEVICE_CODE),
        "最后一个连接退出后连接池应为空"
    );
}

#[tokio::test]
async fn offline_device_reports_actionable_error() {
    let (state, _temp) = test_state().await;
    let port = spawn_server(state.clone()).await;

    // 设备连上再断开：连接池空，台账被置为离线
    let ws = register_device(port, DEVICE_CODE).await;
    close(ws).await;

    let (status, body) = sync_maps(&state, DEVICE_CODE).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("未接入业务平台"),
        "错误提示要说清「连接池里没有设备」而不是只甩状态: {message}"
    );
    assert!(
        message.contains(DEVICE_CODE),
        "错误提示要带上设备编号: {message}"
    );
}

#[tokio::test]
async fn unbound_device_can_still_sync_maps() {
    // 地图只跟设备有关系：设备没绑园区，中台「同步地图」也必须能下发。
    let (state, _temp) = test_state().await;
    sqlx::query("UPDATE devices SET park_id = NULL, zone_id = NULL WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .execute(&state.db)
        .await
        .expect("解绑设备失败");
    let port = spawn_server(state.clone()).await;
    let mut ws = register_device(port, DEVICE_CODE).await;

    let (status, body) = sync_maps(&state, DEVICE_CODE).await;
    assert_eq!(status, StatusCode::OK, "未绑定园区的设备也能同步地图: {body}");
    assert_eq!(body["data"]["dispatched"], true);
    assert_eq!(body["data"]["parkId"], Value::Null);

    let command = next_msg(&mut ws, "command").await;
    assert_eq!(command["payload"]["command"], "同步地图");
    assert_eq!(command["payload"]["params"]["parkId"], Value::Null);
    assert_eq!(command["payload"]["params"]["deviceId"], DEVICE_CODE);
    close(ws).await;
}

#[tokio::test]
async fn stale_online_ledger_state_is_healed() {
    // 历史脏数据：进程重启 / 旧清理竞态留下的「在线待命」但连接池为空
    let (state, _temp) = test_state().await;
    sqlx::query("UPDATE devices SET status = '在线待命', last_heartbeat = '2026-01-01 00:00:00' WHERE device_id = ?1")
        .bind(DEVICE_CODE)
        .execute(&state.db)
        .await
        .expect("造脏数据失败");

    let (status, body) = sync_maps(&state, DEVICE_CODE).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let message = body["msg"].as_str().unwrap_or_default();
    assert!(
        message.contains("已将其标记为离线"),
        "下发失败时应把残留的在线状态纠正为离线: {message}"
    );
    assert_eq!(device_status(&state, DEVICE_CODE).await, "离线");
}
