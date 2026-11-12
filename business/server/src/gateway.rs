use crate::app::AppState;
use crate::db::{now, today};
use crate::error::AppError;
use crate::gateway_types::{
    CommandPayload, DeviceEventPayload, HeartbeatAckPayload, HeartbeatPayload, PendingCommand,
    RegisterAckPayload, RegisterPayload, TaskControlPayload, TaskDispatchPayload,
    TaskProgressPayload, WsMessage,
};
use crate::models::{Device, Task};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tracing::{error, info, warn};

/// 共享的设备连接池：device_id → sender
pub type DeviceConnections = Arc<RwLock<HashMap<String, broadcast::Sender<PendingCommand>>>>;

/// 创建共享的设备连接池
pub fn new_device_connections() -> DeviceConnections {
    Arc::new(RwLock::new(HashMap::new()))
}

/// WebSocket 升级端点：GET /ws/device
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // 对端地址即设备接入 IP，直接落到 `devices.device_ip` 同步给中台
    let peer_ip = peer.ip().to_string();
    ws.on_upgrade(move |socket| handle_socket(socket, state, Some(peer_ip)))
}

/// 处理单个设备 WebSocket 连接
async fn handle_socket(socket: WebSocket, state: AppState, peer_ip: Option<String>) {
    let (mut sender, mut receiver) = socket.split();
    let db = state.db.clone();
    let connections = state.device_connections.clone();

    // 当前连接的设备信息（注册后填充）
    let mut device_id: Option<String> = None;

    // 为该连接创建广播通道
    let (tx, mut rx) = broadcast::channel::<PendingCommand>(64);

    info!("new device WebSocket connection opened");

    // 后台任务：将广播通道的消息转发到 WebSocket sender
    let forward_task = tokio::spawn(async move {
        while let Ok(cmd) = rx.recv().await {
            // cmd_type 即对外的 msgType，只有 remote_command 归一到 command
            let msg_type = match cmd.cmd_type.as_str() {
                "remote_command" => "command".to_string(),
                other => other.to_string(),
            };
            let message = WsMessage {
                msg_type,
                ts: now(),
                payload: cmd.payload,
            };
            let text = serde_json::to_string(&message).unwrap_or_default();
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // 主动向设备发送 ping（每 30 秒），检测连接健康
    let ping_tx = tx.clone();
    let ping_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            // broadcast::Sender::send 失败仅意味着无接收者，忽略
            let _ = ping_tx.send(PendingCommand {
                cmd_type: "ping".to_string(),
                payload: json!({"msg": "ping"}),
            });
        }
    });

    // 处理接收到的消息
    loop {
        tokio::select! {
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let ws_msg: WsMessage = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(e) => {
                                warn!("invalid WS message: {e}");
                                send_msg(&tx, "error", json!({"message": format!("invalid message: {e}")})).await;
                                continue;
                            }
                        };

                        match ws_msg.msg_type.as_str() {
                            "register" => {
                                let payload: RegisterPayload = match serde_json::from_value(ws_msg.payload) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        send_msg(&tx, "error", json!({"message": format!("invalid register payload: {e}")})).await;
                                        continue;
                                    }
                                };
                                match handle_register(&db, &payload, peer_ip.as_deref()).await {
                                    Ok(device) => {
                                        // 设备接入后同步到中台台账，便于在中台分配用户
                                        sync_device_to_zentai(&state.config, &db, &device).await;
                                        let did = payload.device_id.clone();
                                        let old_device_id = device_id.replace(did.clone());

                                        // 如果该设备已有旧连接，将旧 sender 移除
                                        if let Some(ref old) = old_device_id {
                                            connections.write().await.remove(old);
                                        }
                                        connections.write().await.insert(did.clone(), tx.clone());

                                        info!("device registered: {} / {} (park={:?})", did, device.device_id, device.park_id);
                                        let park_code = park_code_of(&db, device.park_id).await;
                                        let unbound = device.park_id.is_none();
                                        send_msg(&tx, "register_ack", json!(RegisterAckPayload {
                                            success: true,
                                            message: if unbound {
                                                "注册成功，但尚未绑定园区".to_string()
                                            } else {
                                                "注册成功".to_string()
                                            },
                                            device_id: did.clone(),
                                            device_name: Some(device.device_id),
                                            park_id: device.park_id,
                                            park_code,
                                            park_name: device.park_name.clone(),
                                            server_ts: now(),
                                        })).await;

                                        // 检查是否有待执行任务并立即下发
                                        if let Err(e) = dispatch_pending_tasks(&db, &tx, &did).await {
                                            warn!("dispatch pending tasks for {}: {e}", did);
                                        }
                                    }
                                    Err(e) => {
                                        warn!("device register failed for {}: {e}", payload.device_id);
                                        send_msg(&tx, "register_ack", json!(RegisterAckPayload {
                                            success: false,
                                            message: e.to_string(),
                                            device_id: payload.device_id,
                                            device_name: None,
                                            park_id: None,
                                            park_code: None,
                                            park_name: None,
                                            server_ts: now(),
                                        })).await;
                                    }
                                }
                            }
                            "heartbeat" => {
                                if let Some(ref did) = device_id {
                                    let payload: HeartbeatPayload = serde_json::from_value(ws_msg.payload)
                                        .unwrap_or(HeartbeatPayload {
                                            battery: None,
                                            temperature: None,
                                            position: None,
                                            odometer: None,
                                            runtime_minutes: None,
                                            current_task: None,
                                            firmware: None,
                                            model: None,
                                            body_online: None,
                                            mode: None,
                                            charging: None,
                                            robot_mode: None,
                                            loc_status: None,
                                            map_id: None,
                                        });
                                    handle_heartbeat(&db, &state.config, did, &payload).await;
                                    send_msg(&tx, "heartbeat_ack", json!(HeartbeatAckPayload {
                                        server_ts: now(),
                                        pending_commands: vec![],
                                    })).await;
                                } else {
                                    send_msg(&tx, "error", json!({"message": "请先注册设备"})).await;
                                }
                            }
                            "task_progress" => {
                                if let Some(ref did) = device_id {
                                    let payload: TaskProgressPayload = serde_json::from_value(ws_msg.payload)
                                        .unwrap_or(TaskProgressPayload {
                                            task_id: None,
                                            task_code: String::new(),
                                            status: "unknown".to_string(),
                                            progress: 0,
                                            point_index: None,
                                            message: None,
                                            robot_task_id: None,
                                            stage: None,
                                            total_points: None,
                                        });
                                    handle_task_progress(&db, did, &payload).await;
                                    send_msg(&tx, "task_progress_ack", json!({"received": true})).await;
                                }
                            }
                            "event" => {
                                if let Some(ref did) = device_id {
                                    let payload: DeviceEventPayload = serde_json::from_value(ws_msg.payload)
                                        .unwrap_or(DeviceEventPayload {
                                            event_type: "unknown".to_string(),
                                            message: String::new(),
                                            data: None,
                                        });
                                    handle_device_event(&db, did, &payload).await;
                                    send_msg(&tx, "event_ack", json!({"received": true})).await;
                                }
                            }
                            "pong" => {
                                // 响应服务端 ping，不做处理
                            }
                            _ => {
                                warn!("unknown message type: {}", ws_msg.msg_type);
                                send_msg(&tx, "error", json!({"message": format!("unknown type: {}", ws_msg.msg_type)})).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("WebSocket closed by client");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = tx.send(PendingCommand {
                            cmd_type: "pong".to_string(),
                            payload: json!({"len": data.len()}),
                        });
                    }
                    Some(Ok(Message::Binary(data))) => {
                        info!("received binary message of {} bytes", data.len());
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Err(e)) => {
                        error!("WebSocket error: {e}");
                        break;
                    }
                    None => {
                        info!("WebSocket stream ended");
                        break;
                    }
                }
            }
        }
    }

    // 清理：移除设备连接
    if let Some(ref did) = device_id {
        connections.write().await.remove(did);
        // 标记设备离线，同时清掉接管态：断连即释放，避免留下「幽灵接管」
        sqlx::query(
            "UPDATE devices SET status = '离线', body_online = 0,
                teleop_holder = NULL, teleop_at = NULL, updated_at = ?1 WHERE device_id = ?2",
        )
        .bind(now())
        .bind(did)
        .execute(&db)
        .await
        .ok();
        info!("device {} disconnected, marked offline", did);
    }

    forward_task.abort();
    ping_task.abort();
}

// ─── 消息发送助手 ──────────────────────────────────────────────

async fn send_msg(tx: &broadcast::Sender<PendingCommand>, msg_type: &str, payload: Value) {
    // payload 直接下发：外层 WsMessage 由转发任务统一组装，避免二次 JSON 编码
    let _ = tx.send(PendingCommand {
        cmd_type: msg_type.to_string(),
        payload,
    });
}

// ─── 业务处理函数 ──────────────────────────────────────────────

/// 设备注册
///
/// 新设备首次注册时自动建档（无园区，状态「待绑定」）：园区由运营在设备管理里
/// 手动绑定，绑定后心跳状态与待执行任务才会正常推进。
///
/// 客户端能上报的台账字段全部在此落库：`model`（本体 1201 `type`）、
/// `firmware`（客户端配置）、`device_ip`（本次接入的对端 IP）。
async fn handle_register(
    db: &SqlitePool,
    payload: &RegisterPayload,
    peer_ip: Option<&str>,
) -> Result<Device, AppError> {
    let device_id = payload.device_id.trim();
    if device_id.is_empty() {
        return Err(AppError::BadRequest("设备编号不能为空".to_string()));
    }

    let timestamp = now();
    // 空白字符串视同未上报，避免把库里的旧值刷成 ""
    let model = payload
        .model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let firmware = payload
        .firmware
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());

    let existing = match fetch_device_by_code(db, device_id).await {
        Ok(device) => Some(device),
        Err(AppError::NotFound(_)) => None,
        Err(error) => return Err(error),
    };

    let Some(existing) = existing else {
        // 首次注册：自动建档为「待绑定」，等人工绑定园区后再接收任务
        let result = sqlx::query(
            "INSERT INTO devices (device_id, model, park_id, zone_id, status, battery, signal,
                temperature, runtime_minutes, odometer, position, current_task_name,
                last_heartbeat, firmware, device_ip, created_at, updated_at)
             VALUES (?1, ?2, NULL, NULL, '待绑定', 100, 4, 32.0, 0, 0, NULL, '—',
                ?3, ?4, ?5, ?3, ?3)",
        )
        .bind(device_id)
        .bind(model)
        .bind(&timestamp)
        .bind(firmware)
        .bind(peer_ip)
        .execute(db)
        .await?;
        info!(
            "new device auto-provisioned: {} (unbound)",
            result.last_insert_rowid()
        );
        return fetch_device_by_code(db, device_id).await;
    };

    // 已建档设备：未绑定园区时保持「待绑定」，其余状态转为在线待命；
    // 台账字段用 COALESCE，客户端没报的保持原值（园区 / 人工改的型号不被抹掉）
    sqlx::query(
        "UPDATE devices SET status = CASE
                WHEN park_id IS NULL THEN '待绑定'
                WHEN status = '充电中' THEN '充电中'
                ELSE '在线待命' END,
            model = COALESCE(?3, model),
            firmware = COALESCE(?4, firmware),
            device_ip = COALESCE(?5, device_ip),
            last_heartbeat = ?1, updated_at = ?1 WHERE id = ?2",
    )
    .bind(&timestamp)
    .bind(existing.id)
    .bind(model)
    .bind(firmware)
    .bind(peer_ip)
    .execute(db)
    .await?;

    fetch_device_by_code(db, device_id).await
}

/// 把业务平台的设备行同步到中台台账（失败只记日志）。
async fn sync_device_to_zentai(config: &crate::config::Config, db: &SqlitePool, device: &Device) {
    if !config.zentai_ready() {
        return;
    }
    let park = match device.park_id {
        Some(park_id) => sqlx::query_as::<_, (String, String)>(
            "SELECT park_code, park_name FROM parks WHERE id = ?1",
        )
        .bind(park_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten(),
        None => None,
    };
    let payload = crate::zentai::DeviceUpsert {
        device_id: device.device_id.clone(),
        device_name: None,
        model: device.model.clone(),
        status: Some(device.status.clone()),
        firmware: device.firmware.clone(),
        device_ip: device.device_ip.clone(),
        park_code: park.as_ref().map(|(code, _)| code.clone()),
        park_name: park.as_ref().map(|(_, name)| name.clone()),
        remarks: None,
        user_ids: Vec::new(),
    };
    crate::zentai::try_upsert_device(config, &payload).await;
}

/// 心跳推导设备状态。
///
/// - 未绑定园区：保持「待绑定」，设备必须先人工绑园区才会进入任务调度
/// - 「执行任务」：任务进度自己推进状态，心跳不改
/// - 充电：优先用本体 1315 上报的 `charging`（充电桩接入 / 电池电源状态），
///   老客户端没有该字段时才退回「电量 ≤ 10%」的兼容猜测
/// - 停止充电 / 重新上线：从「离线」「充电中」回到「在线待命」，
///   其余状态（故障 / 维护中）不动
fn derive_device_status(
    current: &str,
    park_bound: bool,
    charging: Option<bool>,
    battery: Option<i64>,
) -> Option<&'static str> {
    use crate::handlers::devices::DEVICE_STATUS_UNBOUND;

    if !park_bound {
        return (current != DEVICE_STATUS_UNBOUND).then_some(DEVICE_STATUS_UNBOUND);
    }
    if current == "执行任务" {
        return None;
    }
    match charging {
        Some(true) => (current != "充电中").then_some("充电中"),
        Some(false) => matches!(current, "离线" | "充电中").then_some("在线待命"),
        None => {
            if matches!(battery, Some(value) if value <= 10) {
                (current != "充电中").then_some("充电中")
            } else {
                (current == "离线").then_some("在线待命")
            }
        }
    }
}

/// 心跳看门狗：超过 `device_timeout_secs` 没心跳的「在线」设备标记为离线。
///
/// WebSocket 静默断开（拉闸、掉 Wi-Fi、拔网线）时服务端收不到 Close 帧，
/// 只靠断连回调会一直停在「在线待命 / 充电中」，任务下发与接管判断都会跟错；
/// 因此在线状态除了断连回调，还必须有心跳超时兵底。
///
/// 每 15s 扫一次，只在「在线 → 离线」那一刻写一次告警，不会刷屏。
pub fn spawn_heartbeat_watchdog(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(15));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match sweep_stale_devices(&state).await {
                Ok(0) => {}
                Ok(count) => info!("心跳看门狗：{count} 台设备心跳超时，已标记离线"),
                Err(error) => warn!("心跳看门狗执行失败: {error}"),
            }
        }
    });
}

/// 扫描并处理心跳超时设备，返回本次置为离线的台数。
///
/// 只判定「完整时间戳」的行（`YYYY-MM-DD HH:MM:SS`）：`seed.rs` 的演示设备写的是
/// 相对时间（如 `09:15:02`），它们不代表真机接入，不能被看门狗误判成掉线。
pub async fn sweep_stale_devices(state: &AppState) -> Result<u64, AppError> {
    let timeout = state.config.device_timeout_secs;
    let cutoff = (chrono::Local::now() - chrono::Duration::seconds(timeout))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    let stale: Vec<(i64, String, Option<i64>)> = sqlx::query_as(
        "SELECT id, device_id, park_id FROM devices
         WHERE status <> '离线'
           AND last_heartbeat LIKE '____-__-__ __:__:__'
           AND last_heartbeat < ?1",
    )
    .bind(&cutoff)
    .fetch_all(&state.db)
    .await?;

    if stale.is_empty() {
        return Ok(0);
    }

    let timestamp = now();
    let mut affected = 0_u64;
    for (id, code, park_id) in &stale {
        let result = sqlx::query(
            "UPDATE devices SET status = '离线', body_online = 0,
                teleop_holder = NULL, teleop_at = NULL, updated_at = ?1 WHERE id = ?2",
        )
        .bind(&timestamp)
        .bind(id)
        .execute(&state.db)
        .await?;
        affected += result.rows_affected();

        // 一条告警一次：alert_code 带上时间戳，重试时冲突就忽略
        let alert_code = format!("AL-HB-{}-{id}", timestamp.replace(['-', ' ', ':'], ""));
        let content = format!("设备 {code} 心跳超时（>{timeout}s 无上报），已标记离线");
        let _ = sqlx::query(
            "INSERT INTO alerts (alert_code, level, content, device_id, park_id, status,
                occurred_at, created_at)
             VALUES (?1, '警告', ?2, ?3, ?4, '未处理', ?5, ?5)",
        )
        .bind(&alert_code)
        .bind(&content)
        .bind(id)
        .bind(park_id)
        .bind(&timestamp)
        .execute(&state.db)
        .await;
    }

    Ok(affected)
}

/// 查询园区编码（用于 register_ack 下发与地图回传归属）
async fn park_code_of(db: &SqlitePool, park_id: Option<i64>) -> Option<String> {
    let park_id = park_id?;
    sqlx::query_scalar("SELECT park_code FROM parks WHERE id = ?1")
        .bind(park_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

/// 按设备编号读取设备（含园区 / 区域名称）
///
/// 列清单复用 `handlers::devices::DEVICE_SELECT`，避免与设备接口漂移
/// （历史上漏选 `video_url` 等列导致注册回执失败）。
async fn fetch_device_by_code(db: &SqlitePool, device_id: &str) -> Result<Device, AppError> {
    let sql = format!(
        "{} WHERE d.device_id = ?1",
        crate::handlers::devices::DEVICE_SELECT
    );
    sqlx::query_as::<_, Device>(&sql)
        .bind(device_id)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| AppError::NotFound("设备不存在".to_string()))
}

/// 设备绑定园区后，若设备在线则立即补发待执行任务。
pub async fn dispatch_pending_for_device(
    db: &SqlitePool,
    connections: &DeviceConnections,
    device_code: &str,
) -> bool {
    let tx = connections.read().await.get(device_code).cloned();
    match tx {
        Some(tx) => dispatch_pending_tasks(db, &tx, device_code).await.is_ok(),
        None => false,
    }
}

/// 把园区绑定 / 解绑结果推给在线设备。
///
/// 设备端用 `parkId` 确定建图回传归属：`parkId = null` 表示已解绑。
pub async fn notify_park_binding(
    connections: &DeviceConnections,
    device_code: &str,
    park_id: Option<i64>,
    park_code: Option<&str>,
    park_name: Option<&str>,
) -> bool {
    let tx = connections.read().await.get(device_code).cloned();
    let Some(tx) = tx else {
        return false;
    };
    let msg = WsMessage {
        msg_type: "park_binding".to_string(),
        ts: now(),
        payload: json!({
            "parkId": park_id,
            "parkCode": park_code,
            "parkName": park_name,
        }),
    };
    tx.send(PendingCommand {
        cmd_type: msg.msg_type,
        payload: msg.payload,
    })
    .is_ok()
}

/// 心跳处理
///
/// 运行态（电量/信号/温度/里程/任务）每次覆写；台账类字段（型号/固件/状态）
/// 变化时标记 `ledger_changed`，心跳末尾统一再同步一次中台，避免 10s 一次打接口。
async fn handle_heartbeat(
    db: &SqlitePool,
    config: &crate::config::Config,
    device_id: &str,
    payload: &HeartbeatPayload,
) {
    let timestamp = now();
    let mut ledger_changed = false;

    // 使用带有参数绑定的 SQL 更新
    let timestamp_clone = timestamp.clone();
    let device_id_clone = device_id.to_string();

    // 分别更新各个字段，避免复杂的动态 SQL
    let _ =
        sqlx::query("UPDATE devices SET last_heartbeat = ?1, updated_at = ?1 WHERE device_id = ?2")
            .bind(&timestamp)
            .bind(device_id)
            .execute(db)
            .await;

    if let Some(b) = payload.battery {
        let _ =
            sqlx::query("UPDATE devices SET battery = ?1, updated_at = ?2 WHERE device_id = ?3")
                .bind(b)
                .bind(&timestamp_clone)
                .bind(&device_id_clone)
                .execute(db)
                .await;
    }
    if let Some(t) = payload.temperature {
        let _ = sqlx::query(
            "UPDATE devices SET temperature = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(t)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;
    }
    if let Some(pos) = &payload.position {
        let _ =
            sqlx::query("UPDATE devices SET position = ?1, updated_at = ?2 WHERE device_id = ?3")
                .bind(pos)
                .bind(&timestamp_clone)
                .bind(&device_id_clone)
                .execute(db)
                .await;
    }
    if let Some(o) = payload.odometer {
        let _ =
            sqlx::query("UPDATE devices SET odometer = ?1, updated_at = ?2 WHERE device_id = ?3")
                .bind(o)
                .bind(&timestamp_clone)
                .bind(&device_id_clone)
                .execute(db)
                .await;
    }
    if let Some(r) = payload.runtime_minutes {
        let _ = sqlx::query(
            "UPDATE devices SET runtime_minutes = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(r)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;
    }
    if let Some(task) = &payload.current_task {
        let _ = sqlx::query(
            "UPDATE devices SET current_task_name = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(task)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;
    }
    // 定位状态与当前生效地图：设备详情页据此判断「狗在不在地图上」
    if let Some(loc) = payload.loc_status {
        let _ = sqlx::query(
            "UPDATE devices SET loc_status = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(loc)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;
    }
    if let Some(map) = payload
        .map_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let _ = sqlx::query(
            "UPDATE devices SET active_map_id = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(map)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;
    }
    if let Some(fw) = payload
        .firmware
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        // 只有真的变了才返回 rows_affected = 1，用于决定是否重同步中台
        ledger_changed |= sqlx::query(
            "UPDATE devices SET firmware = ?1, updated_at = ?2
             WHERE device_id = ?3 AND COALESCE(firmware, '') <> ?1",
        )
        .bind(fw)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await
        .map(|result| result.rows_affected() > 0)
        .unwrap_or(false);
    }
    if let Some(model) = payload
        .model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        ledger_changed |= sqlx::query(
            "UPDATE devices SET model = ?1, updated_at = ?2
             WHERE device_id = ?3 AND COALESCE(model, '') <> ?1",
        )
        .bind(model)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await
        .map(|result| result.rows_affected() > 0)
        .unwrap_or(false);
    }
    if let Some(charging) = payload.charging {
        // 充电状态只信本体 1315 上报（充电桩接入 / 电池电源状态）
        ledger_changed |= sqlx::query(
            "UPDATE devices SET charging = ?1, updated_at = ?2
             WHERE device_id = ?3 AND COALESCE(charging, 0) <> ?1",
        )
        .bind(i64::from(charging))
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await
        .map(|result| result.rows_affected() > 0)
        .unwrap_or(false);
    }

    // 状态推导：未绑定 → 待绑定；执行中保持；充电以本体上报为准
    let current = sqlx::query_as::<_, (String, Option<i64>)>(
        "SELECT status, park_id FROM devices WHERE device_id = ?1",
    )
    .bind(&device_id_clone)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    if let Some((current_status, park_id)) = current
        && let Some(target) = derive_device_status(
            &current_status,
            park_id.is_some(),
            payload.charging,
            payload.battery,
        )
    {
        ledger_changed |= sqlx::query(
            "UPDATE devices SET status = ?1, updated_at = ?2
             WHERE device_id = ?3 AND status <> ?1",
        )
        .bind(target)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await
        .map(|result| result.rows_affected() > 0)
        .unwrap_or(false);
    }

    if let Some(online) = payload.body_online {
        // 本体链路状态：模拟模式 / 本体掉线时置 0，前端据此不再展示电量等假数据
        let _ = sqlx::query(
            "UPDATE devices SET body_online = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(i64::from(online))
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;
    }
    if let Some(robot_mode) = payload
        .robot_mode
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        // 本体工作模式（1315 mode / MachineStatus）：每次心跳覆写即可，前端只做展示
        let _ =
            sqlx::query("UPDATE devices SET robot_mode = ?1, updated_at = ?2 WHERE device_id = ?3")
                .bind(robot_mode)
                .bind(&timestamp_clone)
                .bind(&device_id_clone)
                .execute(db)
                .await;
    }
    if let Some(mode) = payload
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        let _ =
            sqlx::query("UPDATE devices SET run_mode = ?1, updated_at = ?2 WHERE device_id = ?3")
                .bind(mode)
                .bind(&timestamp_clone)
                .bind(&device_id_clone)
                .execute(db)
                .await;
    }

    // 台账字段变了（首登拿到型号 / 固件升级 / 状态迁移）才推中台
    if ledger_changed
        && config.zentai_ready()
        && let Ok(device) = fetch_device_by_code(db, device_id).await
    {
        sync_device_to_zentai(config, db, &device).await;
    }
}

/// 任务进度上报
async fn handle_task_progress(db: &SqlitePool, device_id: &str, payload: &TaskProgressPayload) {
    let timestamp = now();

    // 更新任务状态
    if !payload.task_code.is_empty() {
        let valid_statuses = ["待执行", "执行中", "已完成", "异常", "已暂停"];
        let status = if valid_statuses.contains(&payload.status.as_str()) {
            &payload.status
        } else {
            "执行中"
        };

        let result = sqlx::query(
            "UPDATE tasks SET task_status = ?1, progress = ?2, updated_at = ?3,
                robot_task_id = COALESCE(?5, robot_task_id),
                robot_stage = COALESCE(?6, robot_stage),
                robot_updated_at = CASE WHEN ?5 IS NOT NULL OR ?6 IS NOT NULL THEN ?3
                                        ELSE robot_updated_at END
             WHERE task_code = ?4",
        )
        .bind(status)
        .bind(payload.progress)
        .bind(&timestamp)
        .bind(&payload.task_code)
        .bind(payload.robot_task_id.as_deref())
        .bind(payload.stage.as_deref())
        .execute(db)
        .await;

        if let Ok(r) = result
            && r.rows_affected() > 0
        {
            // 记录事件：优先用客户端给的中文描述，没有则用阶段名
            let message = payload
                .message
                .as_deref()
                .filter(|m| !m.is_empty())
                .or(payload.stage.as_deref())
                .unwrap_or("进度更新");
            let title = match payload.total_points {
                Some(total) if total > 0 => format!(
                    "任务 {} 进度 {}/{}（{}%）",
                    payload.task_code,
                    payload.point_index.unwrap_or(0),
                    total,
                    payload.progress
                ),
                _ => format!("任务 {} 进度更新: {}%", payload.task_code, payload.progress),
            };
            sqlx::query(
                "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
                 VALUES ('progress', ?1, ?2, (SELECT id FROM devices WHERE device_id = ?3),
                         (SELECT id FROM tasks WHERE task_code = ?4), ?5)",
            )
            .bind(title)
            .bind(message)
            .bind(device_id)
            .bind(&payload.task_code)
            .bind(&timestamp)
            .execute(db)
            .await
            .ok();
        }

        // 任务完成时，更新设备当前任务
        if status == "已完成" || status == "异常" || status == "已暂停" {
            sqlx::query(
                "UPDATE devices SET current_task_name = '—', status = '在线待命', updated_at = ?1
                 WHERE device_id = ?2 AND current_task_name = (SELECT task_name FROM tasks WHERE task_code = ?3)",
            )
            .bind(&timestamp)
            .bind(device_id)
            .bind(&payload.task_code)
            .execute(db)
            .await
            .ok();
        } else if status == "执行中" {
            sqlx::query(
                "UPDATE devices SET status = '执行任务', updated_at = ?1 WHERE device_id = ?2",
            )
            .bind(&timestamp)
            .bind(device_id)
            .execute(db)
            .await
            .ok();
        }
    }
}

/// 设备事件处理
async fn handle_device_event(db: &SqlitePool, device_id: &str, payload: &DeviceEventPayload) {
    let timestamp = now();

    // 根据事件类型处理
    match payload.event_type.as_str() {
        "error" | "fault" | "obstacle" => {
            // 创建告警
            let date_prefix = today();
            let alert_code = format!(
                "AL-WS-{}-{}",
                date_prefix.replace("-", ""),
                &timestamp[11..13]
            );

            sqlx::query(
                "INSERT INTO alerts (alert_code, level, content, device_id, park_id, status, occurred_at, created_at)
                 VALUES (?1, ?2, ?3,
                     (SELECT id FROM devices WHERE device_id = ?4),
                     (SELECT park_id FROM devices WHERE device_id = ?4),
                     '未处理', ?5, ?5)",
            )
            .bind(&alert_code)
            .bind(if payload.event_type == "error" { "严重" } else { "警告" })
            .bind(&payload.message)
            .bind(device_id)
            .bind(&timestamp)
            .execute(db)
            .await
            .ok();
        }
        _ => {}
    }

    // 写入事件流
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('device_event', ?1, ?2, (SELECT id FROM devices WHERE device_id = ?3), NULL, ?4)",
    )
    .bind(format!("设备事件: {}", payload.event_type))
    .bind(&payload.message)
    .bind(device_id)
    .bind(&timestamp)
    .execute(db)
    .await
    .ok();
}

/// 下发待执行任务
async fn dispatch_pending_tasks(
    db: &SqlitePool,
    tx: &broadcast::Sender<PendingCommand>,
    device_id: &str,
) -> Result<(), AppError> {
    // 未绑定园区的设备不接收任务：地图与路线都按园区归属，绑定后才会补发
    let bound_park: Option<i64> =
        sqlx::query_scalar("SELECT park_id FROM devices WHERE device_id = ?1")
            .bind(device_id)
            .fetch_optional(db)
            .await?
            .flatten();
    let Some(park_id) = bound_park else {
        return Ok(());
    };

    // 查找该设备所有"待执行"状态的任务（且任务园区与设备园区一致）
    let tasks = sqlx::query_as::<_, Task>(
        "SELECT t.id, t.task_code, t.task_name, t.task_type, t.execute_mode, t.level,
            t.device_id, d.device_id AS device_code, t.park_id, p.park_name, t.zone_id,
            z.zone_name, t.route_id, r.route_code, r.route_name, t.plan_start, t.plan_end,
            t.task_status, t.progress, t.dispatch_policy, t.description, t.robot_task_id,
            t.robot_stage, t.robot_updated_at, t.created_at, t.updated_at, t.created_by
         FROM tasks t
         LEFT JOIN devices d ON d.id = t.device_id
         LEFT JOIN parks p ON p.id = t.park_id
         LEFT JOIN zones z ON z.id = t.zone_id
         LEFT JOIN routes r ON r.id = t.route_id
         WHERE d.device_id = ?1 AND t.task_status = '待执行'
            AND (t.park_id IS NULL OR t.park_id = ?2)
         ORDER BY t.level DESC, t.created_at ASC",
    )
    .bind(device_id)
    .bind(park_id)
    .fetch_all(db)
    .await?;

    for task in tasks {
        let dispatch = match crate::dispatch::build_dispatch_payload(db, &task).await {
            Ok(payload) => payload,
            Err(e) => {
                warn!("build dispatch payload for task {}: {e}", task.task_code);
                continue;
            }
        };

        let payload = serde_json::to_value(&dispatch).unwrap_or_default();
        let _ = tx.send(PendingCommand {
            cmd_type: "task_dispatch".to_string(),
            payload,
        });

        // 将任务标记为"执行中"
        sqlx::query("UPDATE tasks SET task_status = '执行中', updated_at = ?1 WHERE id = ?2")
            .bind(now())
            .bind(task.id)
            .execute(db)
            .await
            .ok();

        // 更新设备状态
        sqlx::query(
            "UPDATE devices SET status = '执行任务', current_task_name = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(&task.task_name)
        .bind(now())
        .bind(device_id)
        .execute(db)
        .await
        .ok();

        info!("dispatched task {} to device {}", task.task_code, device_id);
    }

    Ok(())
}

// ─── 外部调用接口 ──────────────────────────────────────────────

/// 向指定设备下发任务（由 REST API 在创建/分配任务时调用）
pub async fn dispatch_task_to_device(
    connections: &DeviceConnections,
    device_id: &str,
    task: &TaskDispatchPayload,
) -> bool {
    let map = connections.read().await;
    if let Some(tx) = map.get(device_id) {
        let payload = serde_json::to_value(task).unwrap_or_default();
        match tx.send(PendingCommand {
            cmd_type: "task_dispatch".to_string(),
            payload,
        }) {
            Ok(_) => {
                info!("sent task dispatch to device {device_id}");
                true
            }
            Err(_) => {
                warn!("failed to send task to device {device_id} (no receivers)");
                false
            }
        }
    } else {
        warn!("device {device_id} not connected");
        false
    }
}

/// 向指定设备发送任务控制指令（暂停 / 恢复 / 取消）
pub async fn send_task_control_to_device(
    connections: &DeviceConnections,
    device_id: &str,
    control: &TaskControlPayload,
) -> bool {
    let map = connections.read().await;
    if let Some(tx) = map.get(device_id) {
        let payload = serde_json::to_value(control).unwrap_or_default();
        match tx.send(PendingCommand {
            cmd_type: "task_control".to_string(),
            payload,
        }) {
            Ok(_) => true,
            Err(_) => {
                warn!("failed to send task control to device {device_id} (no receivers)");
                false
            }
        }
    } else {
        warn!("device {device_id} not connected, cannot send task control");
        false
    }
}

/// 向指定设备发送远程指令
pub async fn send_command_to_device(
    connections: &DeviceConnections,
    device_id: &str,
    command: &str,
    params: Option<Value>,
) -> bool {
    let map = connections.read().await;
    if let Some(tx) = map.get(device_id) {
        let payload = CommandPayload {
            command: command.to_string(),
            params,
            issued_at: now(),
        };
        let payload = serde_json::to_value(payload).unwrap_or_default();
        match tx.send(PendingCommand {
            cmd_type: "remote_command".to_string(),
            payload,
        }) {
            Ok(_) => {
                info!("sent command '{command}' to device {device_id}");
                true
            }
            Err(_) => false,
        }
    } else {
        warn!("device {device_id} not connected, cannot send command");
        false
    }
}

/// 向指定设备发送实时视频开关指令
///
/// `video_start` 带拉流地址（就是设备档案里的 `video_url`），客户端据此起 ffmpeg；
/// `video_stop` 不带载荷，客户端收到就杀掉拉流进程。两者都是独立 msgType，
/// 不走 `command`，避免和业务指令（开始巡检 / 召回充电）混在一起。
pub async fn send_video_control(
    connections: &DeviceConnections,
    device_id: &str,
    url: Option<&str>,
) -> bool {
    let map = connections.read().await;
    let Some(tx) = map.get(device_id) else {
        warn!("device {device_id} not connected, cannot send video control");
        return false;
    };
    let (cmd_type, payload) = match url {
        Some(url) => (
            "video_start",
            json!({ "url": url, "format": "flv" }),
        ),
        None => ("video_stop", json!({})),
    };
    match tx.send(PendingCommand {
        cmd_type: cmd_type.to_string(),
        payload,
    }) {
        Ok(_) => true,
        Err(_) => {
            warn!("failed to send {cmd_type} to device {device_id} (no receivers)");
            false
        }
    }
}

/// 向指定设备发送遥控摇杆
///
/// 摇杆走高频通道，载荷按 `{msgType:"teleop"}` 下发，客户端转成智航 1304。
/// 与 `command` 分开是因为摇杆是连续流（按住期间 ~10Hz 下发），
/// 混在 `command` 里会让审计日志被刷爆。
pub async fn send_teleop_to_device(
    connections: &DeviceConnections,
    device_id: &str,
    axes: &crate::models::TeleopRequest,
) -> bool {
    let map = connections.read().await;
    let Some(tx) = map.get(device_id) else {
        warn!("device {device_id} not connected, cannot send teleop");
        return false;
    };
    let payload = json!({
        "forward": axes.forward.clamp(-1.0, 1.0),
        "strafe": axes.strafe.clamp(-1.0, 1.0),
        "turn": axes.turn.clamp(-1.0, 1.0),
        "speed": axes.speed_ratio(),
    });
    match tx.send(PendingCommand {
        cmd_type: "teleop".to_string(),
        payload,
    }) {
        Ok(_) => true,
        Err(_) => {
            warn!("failed to send teleop to device {device_id} (no receivers)");
            false
        }
    }
}

/// 获取在线设备列表
#[allow(dead_code)] // 预留：供在线状态查询接口复用
pub async fn get_online_devices(connections: &DeviceConnections) -> Vec<String> {
    let map = connections.read().await;
    map.keys().cloned().collect()
}

/// 检查设备是否在线
#[allow(dead_code)] // 预留：供在线状态查询接口复用
pub async fn is_device_online(connections: &DeviceConnections, device_id: &str) -> bool {
    let map = connections.read().await;
    map.contains_key(device_id)
}

#[cfg(test)]
mod tests {
    use super::derive_device_status;

    #[test]
    fn unbound_device_stays_pending() {
        // 未绑园区：不管充不充电，都必须保持「待绑定」，否则会被任务调度选中
        assert_eq!(
            derive_device_status("待绑定", false, Some(true), Some(80)),
            None
        );
        assert_eq!(
            derive_device_status("充电中", false, Some(true), Some(80)),
            Some("待绑定")
        );
    }

    #[test]
    fn charging_comes_from_body_report() {
        assert_eq!(
            derive_device_status("在线待命", true, Some(true), Some(88)),
            Some("充电中")
        );
        assert_eq!(
            derive_device_status("充电中", true, Some(false), Some(88)),
            Some("在线待命")
        );
        // 电量低但本体说没在充电：不再瞎猜成「充电中」
        assert_eq!(
            derive_device_status("在线待命", true, Some(false), Some(5)),
            None
        );
        // 电量低且老客户端没上报充电状态：兼容兜底
        assert_eq!(
            derive_device_status("在线待命", true, None, Some(5)),
            Some("充电中")
        );
    }

    #[test]
    fn running_task_and_manual_statuses_are_kept() {
        assert_eq!(
            derive_device_status("执行任务", true, Some(false), Some(60)),
            None
        );
        assert_eq!(
            derive_device_status("执行任务", true, Some(true), Some(60)),
            None
        );
        assert_eq!(
            derive_device_status("故障", true, Some(false), Some(60)),
            None
        );
        assert_eq!(
            derive_device_status("离线", true, Some(false), Some(60)),
            Some("在线待命")
        );
    }
}
