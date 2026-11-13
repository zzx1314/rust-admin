use crate::app::AppState;
use crate::db::{now, today};
use crate::error::AppError;
use crate::gateway_types::{
    CommandPayload, DeviceEventPayload, HeartbeatAckPayload, HeartbeatPayload, PendingCommand,
    RegisterAckPayload, RegisterPayload, TaskControlPayload, TaskDispatchPayload,
    TaskProgressPayload, TeleopStatePayload, WsMessage,
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
use tracing::{debug, error, info, warn};

/// 共享的设备连接池：device_id → sender
pub type DeviceConnections = Arc<RwLock<HashMap<String, broadcast::Sender<PendingCommand>>>>;

/// 创建共享的设备连接池
pub fn new_device_connections() -> DeviceConnections {
    Arc::new(RwLock::new(HashMap::new()))
}

// ─── 连接池登记 ──────────────────────────────────────────

/// 池里该设备登记的是不是本条连接。
fn is_current_connection(
    map: &HashMap<String, broadcast::Sender<PendingCommand>>,
    device_id: &str,
    tx: &broadcast::Sender<PendingCommand>,
) -> bool {
    map.get(device_id)
        .is_some_and(|current| current.same_channel(tx))
}

/// 摘除本条连接的登记，返回「是否真的摘掉」（即本连接仍是该设备的当前连接）。
///
/// 客户端重连时旧 TCP 可能还没死透，新连接已经把池里的 sender 换掉；
/// 旧连接退出的清理必须比对身份，否则会把仍活着的新连接摘走。
fn release_connection(
    map: &mut HashMap<String, broadcast::Sender<PendingCommand>>,
    device_id: &str,
    tx: &broadcast::Sender<PendingCommand>,
) -> bool {
    if is_current_connection(map, device_id, tx) {
        map.remove(device_id);
        true
    } else {
        false
    }
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

                                        {
                                            let mut map = connections.write().await;
                                            // 换设备号时只摘掉自己上一次的登记，别碰别的连接；
                                            // 同一设备换连接直接覆盖（旧连接退出时靠通道身份判断，
                                            // 不会误删新登记）
                                            if let Some(ref old) = old_device_id {
                                                release_connection(&mut map, old, &tx);
                                            }
                                            map.insert(did.clone(), tx.clone());
                                        }

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
                                    // 心跳自愈：连接池里登记的不是本连接（历史版本的
                                    // 清理竞态、进程重启）就补登记，否则台账显示在线、
                                    // 中台下发指令却永远报「设备未连接」。
                                    {
                                        let mut map = connections.write().await;
                                        if !is_current_connection(&map, did, &tx) {
                                            warn!(
                                                "device {did} heartbeat without pool entry, re-registering connection"
                                            );
                                            map.insert(did.clone(), tx.clone());
                                        }
                                    }
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
                                            ctrl_source: None,
                                            motion_status: None,
                                            estop_software: None,
                                            sdk_node_available: None,
                                        });
                                    handle_heartbeat(&db, &state.config, did, &payload).await;
                                    // 本体就绪后才补发待执行任务：注册时通常还没连上本体，
                                    // 早发只会得到一条「未连接机器人本体」并留下假执行中。
                                    if payload.body_online == Some(true)
                                        && let Err(e) = dispatch_pending_tasks(&db, &tx, did).await
                                    {
                                        warn!("dispatch pending tasks for {did}: {e}");
                                    }
                                    // 强制接管标记随心跳下发：客户端重启后本地的
                                    // `teleop_force` 会丢，不补的话摇杆又会被
                                    // `ctrl_source = 3` 门禁拦下。
                                    let teleop_force: bool = sqlx::query_scalar(
                                        "SELECT COALESCE(teleop_force, 0) FROM devices
                                         WHERE device_id = ?1",
                                    )
                                    .bind(did)
                                    .fetch_optional(&db)
                                    .await
                                    .ok()
                                    .flatten()
                                    .unwrap_or(0)
                                        != 0;
                                    send_msg(&tx, "heartbeat_ack", json!(HeartbeatAckPayload {
                                        server_ts: now(),
                                        pending_commands: vec![],
                                        teleop_force,
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
                            // 客户端进程内重连本体后，业务 WS 没断，服务端收不到断连回调；
                            // 必须靠这条消息把「平台以为还握着 RoamerX」的幽灵接管清掉。
                            "teleop_state" => {
                                if let Some(ref did) = device_id {
                                    let payload: TeleopStatePayload = serde_json::from_value(ws_msg.payload)
                                        .unwrap_or(TeleopStatePayload {
                                            holder: false,
                                            reason: None,
                                        });
                                    handle_teleop_state(&db, did, &payload).await;
                                    send_msg(&tx, "teleop_state_ack", json!({"received": true})).await;
                                }
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

    // 清理：只摘掉本连接自己的登记项
    //
    // 客户端重连时旧 TCP 往往还没死透（拔网线 / 4G 掉线 / 进程被 kill 都收不到
    // Close 帧），新连接注册后会把连接池里的 sender 换成新的；等旧连接终于超时
    // 退出，如果无条件 `remove(did)` 摘掉的就是新连接 —— 台账还在「在线待命」，
    // 中台下发指令（同步地图等）却报「设备未连接」，而且要等设备重连才恢复。
    // 所以这里必须比对通道身份。
    if let Some(ref did) = device_id {
        let still_owner = {
            let mut map = connections.write().await;
            release_connection(&mut map, did, &tx)
        };
        if still_owner {
            // 标记设备离线，同时清掉接管态：断连即释放，避免留下「幽灵接管」
            sqlx::query(
                "UPDATE devices SET status = '离线', body_online = 0,
                    teleop_holder = NULL, teleop_at = NULL, teleop_force = 0,
                    updated_at = ?1 WHERE device_id = ?2",
            )
            .bind(now())
            .bind(did)
            .execute(&db)
            .await
            .ok();
            info!("device {} disconnected, marked offline", did);
        } else {
            info!(
                "device {} stale connection closed, current registration kept",
                did
            );
        }
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
    body_online: Option<bool>,
) -> Option<&'static str> {
    use crate::handlers::devices::DEVICE_STATUS_UNBOUND;

    if !park_bound {
        return (current != DEVICE_STATUS_UNBOUND).then_some(DEVICE_STATUS_UNBOUND);
    }
    if current == "执行任务" {
        return None;
    }
    // 本体链路断开但客户端 WS 还在：以前仍显示「在线待命」，运维会以为能遥控。
    // 对齐官方 App 的“设备初始化中/未连接”，这里落到「故障」，本体恢复后回待命。
    if body_online == Some(false) {
        return (current != "故障").then_some("故障");
    }
    if body_online == Some(true) && current == "故障" {
        return Some("在线待命");
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
                teleop_holder = NULL, teleop_at = NULL, teleop_force = 0,
                updated_at = ?1 WHERE id = ?2",
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

/// 任务看门狗：`执行中` 但长时间没有进度上报、或者进度长时间不推进的任务判定为中断，
/// 置「异常」。
///
/// 客户端只在本地持有任务上下文时每 5s 上报一次进度；遇到下面几种情况平台侧就
/// 再也收不到任何上报，任务会永久停在「执行中」：
/// - 客户端重启 / 掉线（WebSocket 断了，本地 runner 也没了）；
/// - 平台只发了 `task_control`（resume）没发 `task_dispatch`，客户端压根没有任务可跑；
/// - 客户端回了终态但上报丢失。
///
/// 还有一种更容易看错的情况：客户端还在周期性上报「执行中」，但进度一直是 0%——
/// 上报没断，`updated_at` 一直新鲜，只靠「有没有上报」永远判不出来。所以同时看
/// `progress_updated_at`（只在 progress 数值变化时刷新）。
///
/// 与心跳看门狗一样每 15s 扫一次。心跳看门狗先把设备置为离线，这里再用
/// `task_timeout_secs` 给任务上一道时间兜底。
pub fn spawn_task_watchdog(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(tokio::time::Duration::from_secs(15));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match sweep_stuck_tasks(&state).await {
                Ok(0) => {}
                Ok(count) => info!("任务看门狗：{count} 条任务超时未上报进度，已置为异常"),
                Err(error) => warn!("任务看门狗执行失败: {error}"),
            }
        }
    });
}

/// 看门狗扫描结果：
/// (task_pk, task_code, task_name, progress, device_pk, device_code, device_status, kind)，
/// `kind` 为 `silent`（没有上报）或 `stalled`（有上报但进度不动）。
type StuckTask = (i64, String, String, i64, i64, String, String, String);

/// 扫描并处理「执行中但不再上报」的任务，返回本次置为异常的条数。
///
/// 只判定真机设备（`last_heartbeat` 是完整时间戳）：`seed.rs` 的演示任务写的是
/// 相对时间，不会被看门狗翻成异常。
pub async fn sweep_stuck_tasks(state: &AppState) -> Result<u64, AppError> {
    let timeout = state.config.task_timeout_secs;
    let stuck = state.config.task_stuck_secs;
    let silent_cutoff = (chrono::Local::now() - chrono::Duration::seconds(timeout))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let stalled_cutoff = (chrono::Local::now() - chrono::Duration::seconds(stuck))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    // 两种「卡住」都要判：
    // - silent：超时没有任何上报（掉线 / 客户端重启）；
    // - stalled：还在报，但进度数值长时间不动（客户端没有执行上下文还在周期性
    //   上报「执行中」）。只看 `updated_at` 时，后一种会让任务永远停在执行中。
    let stale: Vec<StuckTask> = sqlx::query_as(
        "SELECT t.id, t.task_code, t.task_name, t.progress, d.id, d.device_id, d.status,
                CASE WHEN t.updated_at < ?1 THEN 'silent' ELSE 'stalled' END
         FROM tasks t
         JOIN devices d ON d.id = t.device_id
         WHERE t.task_status = '执行中'
           AND d.last_heartbeat LIKE '____-__-__ __:__:__'
           AND (t.updated_at < ?1 OR COALESCE(t.progress_updated_at, t.updated_at) < ?2)",
    )
    .bind(&silent_cutoff)
    .bind(&stalled_cutoff)
    .fetch_all(&state.db)
    .await?;

    if stale.is_empty() {
        return Ok(0);
    }

    let timestamp = now();
    let mut affected = 0_u64;
    for (task_pk, task_code, task_name, progress, device_pk, device_code, device_status, kind) in
        &stale
    {
        // 带上 task_status 条件：扫描期间客户端刚好补报终态时不要覆盖真实结果
        let result = sqlx::query(
            "UPDATE tasks SET task_status = '异常', updated_at = ?1
             WHERE id = ?2 AND task_status = '执行中'",
        )
        .bind(&timestamp)
        .bind(task_pk)
        .execute(&state.db)
        .await?;
        if result.rows_affected() == 0 {
            continue;
        }
        affected += result.rows_affected();

        sqlx::query(
            "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
             VALUES ('status', ?1, ?2, ?3, ?4, ?5)",
        )
        .bind(format!("任务 {task_code} 执行中断"))
        .bind(if kind == "silent" {
            format!(
                "超过 {timeout}s 未收到进度上报（设备 {device_code} {device_status}），已判定为异常；\
                 设备恢复后请重新下发任务"
            )
        } else {
            format!(
                "进度停在 {progress}% 超过 {stuck}s 没有任何推进（设备 {device_code} {device_status}），\
                 已判定为异常；请检查本体定位/导航状态后重新下发任务"
            )
        })
        .bind(device_pk)
        .bind(task_pk)
        .bind(&timestamp)
        .execute(&state.db)
        .await
        .ok();

        // 设备端任务占用一并释放，避免设备页一直显示「执行任务 / 旧任务名」
        sqlx::query(
            "UPDATE devices SET current_task_name = '—',
                status = CASE WHEN status = '执行任务' THEN '在线待命' ELSE status END,
                updated_at = ?1
             WHERE id = ?2 AND current_task_name = ?3",
        )
        .bind(&timestamp)
        .bind(device_pk)
        .bind(task_name)
        .execute(&state.db)
        .await
        .ok();
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
    // 控制源 / 运动状态：判断「是否已完全接管」的两个硬指标
    if let Some(source) = payload.ctrl_source {
        record_control_source_change(db, device_id, source, &timestamp).await;
    }
    if let Some(motion) = payload
        .motion_status
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let _ = sqlx::query(
            "UPDATE devices SET motion_status = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(motion)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;
    }
    // 软件急停 / SDK 节点：任务下发与遥控的硬门禁，NULL 表示老客户端未上报
    if let Some(estop) = payload.estop_software {
        let _ = sqlx::query(
            "UPDATE devices SET estop_software = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(i64::from(estop))
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;

        // 「解除急停下发后没回读生效」落的 `AL-ESTOP-*` 告警，在急停真的解除后
        // 自动结案，避免告警中心长期挂着已经恢复的条目。
        if !estop {
            let _ = sqlx::query(
                "UPDATE alerts SET status = '已处理', handled_at = ?1
                  WHERE status = '未处理' AND alert_code LIKE 'AL-ESTOP-%'
                    AND device_id = (SELECT id FROM devices WHERE device_id = ?2)",
            )
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db)
            .await;
        }
    }
    if let Some(available) = payload.sdk_node_available {
        let _ = sqlx::query(
            "UPDATE devices SET sdk_node_available = ?1, updated_at = ?2 WHERE device_id = ?3",
        )
        .bind(i64::from(available))
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
            payload
                .body_online
                .filter(|_| payload.mode.as_deref() != Some("sim")),
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

        // 本体状态不可信（掉线 / 客户端刚重建到本体的链路）时，1315 派生的健康字段
        // 一律置空表示「未知」：以前保留旧值，客户端重连后要等下一帧 1315 / 1207 才
        // 刷新，页面就拿着上一次的 `sdk_node=0` / `estop=1` 继续拦遥控与任务，
        // 现场表现是「本体明明恢复了，页面还说本体 SDK 节点未就绪」。
        // 模拟设备没有本体，不做这层清理（否则联调页面全是未知）。
        if !online && payload.mode.as_deref() != Some("sim") {
            let _ = sqlx::query(
                "UPDATE devices SET sdk_node_available = NULL, estop_software = NULL,
                        motion_status = NULL, ctrl_source = 0, updated_at = ?1
                  WHERE device_id = ?2",
            )
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db)
            .await;
        }
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

/// 「导航能力被占用」的判据窗口（秒）
///
/// `ctrl_source = 3` 之后本体要开始报 13897 / 13440 才会留下事件，留 5 分钟足够覆盖
/// 一次下发尝试的完整过程，又不至于把半小时前的旧故障算进来。
const CAPABILITY_BUSY_WINDOW_SECS: i64 = 300;

/// 本体近 5 分钟是否回报过「导航能力被占用」的迹象（`13897 capability busy` / `13440`）。
///
/// `ctrl_source = 3` 在协议里是「其他控制源」，现场其实有两种完全不同的原因，
/// 平台文案必须能区分，否则每次下发任务都会误报成「遥控器被接管」：
///
/// 1. 真有物理遥控器 / 第三方 App 压着控制权；
/// 2. 本体里残留了没结束的任务（或算法节点卡住），导航能力被判 busy——
///    固件这时同样把控制源算成「其他」。
///
/// 第 2 种一定伴随 13897 / 13440 的航点失败事件，所以拿它们当判据。
async fn recent_capability_busy(db: &SqlitePool, device_id: &str) -> bool {
    let since = (chrono::Local::now() - chrono::Duration::seconds(CAPABILITY_BUSY_WINDOW_SECS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let busy: Option<i64> = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
             SELECT 1 FROM task_events
              WHERE device_id = (SELECT id FROM devices WHERE device_id = ?1)
                AND created_at >= ?2
                AND (description LIKE '%13897%'
                     OR description LIKE '%capability busy%'
                     OR description LIKE '%13440%')
              ORDER BY id DESC
              LIMIT 20
         )",
    )
    .bind(device_id)
    .bind(&since)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    busy.unwrap_or(0) > 0
}

/// `ctrl_source = 3` 的解释句（不含各入口自己的下一步建议）。
///
/// 告警、任务下发门禁、遥控门禁共用这一句，保证三处口径一致：
/// 既不武断地喊「遥控器被接管」，也不把真遥控器说成本体自己的问题。
pub(crate) async fn ctrl_preempt_reason(db: &SqlitePool, device_id: &str) -> String {
    if recent_capability_busy(db, device_id).await {
        format!(
            "本体近 {} 分钟回报过 13897 capability busy / 13440 导航指令非法，\
             更可能是本体内残留任务或算法节点占用了导航能力（而不是物理遥控器）",
            CAPABILITY_BUSY_WINDOW_SECS / 60
        )
    } else {
        "可能是物理遥控器、第三方 App 或本体内部任务占着控制权".to_string()
    }
}

/// 控制源（1315 `ctrl_source`）：写库 + **只在变化时**留痕。
///
/// 心跳是 1Hz，每帧都写告警会把告警中心刷爆，所以先比旧值。
/// `ctrl_source = 3`（其他控制源：遥控器 / 第三方 App / 本体内部任务）是现场最需要
/// 立刻知道的一条：它一出现，平台摇杆、接管、任务下发全部失效，而设备看上去还是
/// 「在线待命」。落库的这条告警会被业务控制台轮询到并弹窗提示
/// （`web/src/layout/OpsLayout.vue`）。
async fn record_control_source_change(
    db: &SqlitePool,
    device_id: &str,
    source: i64,
    timestamp: &str,
) {
    let previous: Option<i64> =
        sqlx::query_scalar("SELECT ctrl_source FROM devices WHERE device_id = ?1")
            .bind(device_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();

    let _ = sqlx::query("UPDATE devices SET ctrl_source = ?1, updated_at = ?2 WHERE device_id = ?3")
        .bind(source)
        .bind(timestamp)
        .bind(device_id)
        .execute(db)
        .await;

    if previous == Some(source) {
        return;
    }

    let from = previous.map_or_else(|| "未上报".to_string(), |value| value.to_string());
    info!("设备 {device_id} 控制源变化: ctrl_source {from} → {source}");

    // 只留痕「与遥控器有关」的跳变（进 3 或出 3）：
    // 设备刚上线时 0 → 1 也是变化，但那一帧没有任何可行动信息。
    if source != 3 && previous != Some(3) {
        return;
    }

    // 事件流：运维在日志/设备事件里能看到「什么时候被谁拿走」
    let _ = sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('device_event', ?1, ?2, (SELECT id FROM devices WHERE device_id = ?3), NULL, ?4)",
    )
    .bind(if source == 3 {
        "控制权被其他控制源接管"
    } else {
        "控制权已收回"
    })
    .bind(format!(
        "1315 ctrl_source: {from} → {source}\
         （0 未知 / 1 APP / 2 SDK / 3 其他：遥控器、第三方 App 或本体内部任务）"
    ))
    .bind(device_id)
    .bind(timestamp)
    .execute(db)
    .await;

    if source == 3 {
        open_control_preempt_alert(db, device_id, timestamp).await;
    } else {
        resolve_control_preempt_alert(db, device_id, timestamp).await;
    }
}

/// 控制权被其他控制源接管（`ctrl_source = 3`）→ 一条「未处理」告警。
///
/// 同一台设备已经有一条未处理的抢占告警时不重复插入：遥控器时断时续（`3 → 1 → 3`）
/// 在现场很常见，刷屏反而会盖住真正的故障。
async fn open_control_preempt_alert(db: &SqlitePool, device_id: &str, timestamp: &str) {
    let pending: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM alerts
          WHERE status = '未处理' AND alert_code LIKE 'AL-CTRL-%'
            AND device_id = (SELECT id FROM devices WHERE device_id = ?1)
          LIMIT 1",
    )
    .bind(device_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    if pending.is_some() {
        return;
    }

    // alert_code 唯一：带上设备号 + 毫秒时间戳，避免同一秒内二次抢占互相覆盖
    let alert_code = format!(
        "AL-CTRL-{}-{}",
        device_id,
        chrono::Local::now().format("%Y%m%d%H%M%S%3f")
    );
    let reason = ctrl_preempt_reason(db, device_id).await;
    let _ = sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, device_id, park_id, status,
            occurred_at, created_at)
         VALUES (?1, '警告', ?2,
            (SELECT id FROM devices WHERE device_id = ?3),
            (SELECT park_id FROM devices WHERE device_id = ?3),
            '未处理', ?4, ?4)",
    )
    .bind(&alert_code)
    .bind(format!(
        "设备 {device_id} 上报 1315 ctrl_source=3（控制权不在平台）：{reason}。\
         平台摇杆与任务下发已暂停：请关闭遥控器电源 / 退出第三方程序，\
         并在本体上取消残留任务或重启本体算法服务；确认遥控器已断电后可「强制接管」"
    ))
    .bind(device_id)
    .bind(timestamp)
    .execute(db)
    .await;
}

/// 控制权回到 APP / SDK：自动把抢占告警结掉，避免告警中心长期挂着已恢复的条目。
async fn resolve_control_preempt_alert(db: &SqlitePool, device_id: &str, timestamp: &str) {
    let _ = sqlx::query(
        "UPDATE alerts SET status = '已处理', handled_at = ?1
          WHERE status = '未处理' AND alert_code LIKE 'AL-CTRL-%'
            AND device_id = (SELECT id FROM devices WHERE device_id = ?2)",
    )
    .bind(timestamp)
    .bind(device_id)
    .execute(db)
    .await;
}

/// 任务进度上报
///
/// 三条约束（都是现场卡单后补的）：
/// 1. **终态不可回退**：已完成 / 异常的任务不再被「执行中」的周期上报拉回去。
///    客户端没有执行上下文时仍会周期性上报，旧逻辑会把刚判定的异常刷回执行中，
///    看门狗又因为 `updated_at` 新鲜不触发，任务就永远停在「执行中」。
/// 2. **进度时间戳只随数值变化**：`progress_updated_at` 供看门狗判「进度停滞」；
///    `updated_at` 保持「最后一次收到上报」的含义。
/// 3. **状态没更新就不动设备台账**：避免一条过期上报把设备从「在线待命」改回
///    「执行任务」。
async fn handle_task_progress(db: &SqlitePool, device_id: &str, payload: &TaskProgressPayload) {
    let timestamp = now();
    if payload.task_code.is_empty() {
        return;
    }

    let valid_statuses = ["待执行", "执行中", "已完成", "异常", "已暂停"];
    let status = if valid_statuses.contains(&payload.status.as_str()) {
        &payload.status
    } else {
        "执行中"
    };

    let result = sqlx::query(
        "UPDATE tasks SET task_status = ?1, progress = ?2, updated_at = ?3,
            progress_updated_at = CASE WHEN progress <> ?2 OR progress_updated_at IS NULL
                                       THEN ?3 ELSE progress_updated_at END,
            robot_task_id = COALESCE(?5, robot_task_id),
            robot_stage = COALESCE(?6, robot_stage),
            robot_updated_at = CASE WHEN ?5 IS NOT NULL OR ?6 IS NOT NULL THEN ?3
                                    ELSE robot_updated_at END
         WHERE task_code = ?4
           AND (task_status NOT IN ('已完成', '异常') OR ?1 IN ('已完成', '异常'))",
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
        // 进度事件去重：客户端 5s 一报，内容没变就不重复落事件。
        // 以前暂停/卡住时一小时能刷出上千条同样的「任务已暂停」，把
        // 「导航指令非法」「无有效绕障轨迹」这些真正要看的故障全冲掉了。
        let last_progress: Option<(String, String)> = sqlx::query_as(
            "SELECT title, description FROM task_events
              WHERE event_type = 'progress'
                AND task_id = (SELECT id FROM tasks WHERE task_code = ?1)
              ORDER BY id DESC LIMIT 1",
        )
        .bind(&payload.task_code)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
        let unchanged = last_progress
            .as_ref()
            .is_some_and(|(prev_title, prev_desc)| {
                prev_title == &title && prev_desc.as_str() == message
            });
        if !unchanged {
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

/// 设备事件 → 告警级别。
///
/// 以前所有 `fault` 都写成「警告」，4021 的 `severity=3`（如 13504 内部节点异常）
/// 也被淹没；这里优先读事件 data 里的 `severity` / `level`，>=2 视为严重。
fn event_alert_level(payload: &DeviceEventPayload) -> &'static str {
    if payload.event_type == "error" {
        return "严重";
    }
    let severity = payload
        .data
        .as_ref()
        .and_then(|data| {
            data.get("severity")
                .or_else(|| data.get("level"))
                .and_then(Value::as_i64)
        })
        .unwrap_or(0);
    if payload.event_type == "fault" && severity >= 2 {
        "严重"
    } else {
        "警告"
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
            .bind(event_alert_level(payload))
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

/// 客户端上报「本地已不持有控制权」
///
/// 进程内重连本体会重建 control 通道，`1204` 控制权随之丢失，但设备与业务平台的
/// WebSocket 一直活着，服务端「断连即清 `teleop_holder`」那条清理永远不触发。
/// 不清就会留下幽灵接管：设备页显示「接管中」、任务下发被「正在远程接管中」拦下、
/// 摇杆又被客户端门禁拒绝，运维只能重启平台才能恢复。
///
/// `holder: true` 一律忽略：接管态由平台指令（「远程接管」）写入，不能让设备客户端
/// 自己宣布持有，否则两个来源会互相覆盖。
async fn handle_teleop_state(db: &SqlitePool, device_id: &str, payload: &TeleopStatePayload) {
    if payload.holder {
        debug!("device {device_id} reported holder=true, ignored (holder is platform-owned)");
        return;
    }

    let timestamp = now();
    match sqlx::query(
        "UPDATE devices SET teleop_holder = NULL, teleop_at = NULL, teleop_force = 0,
            updated_at = ?1 WHERE device_id = ?2 AND teleop_holder IS NOT NULL",
    )
    .bind(&timestamp)
    .bind(device_id)
    .execute(db)
    .await
    {
        Ok(result) if result.rows_affected() > 0 => {
            let reason = payload.reason.as_deref().unwrap_or("unreported");
            info!("device {device_id} released teleop holder (reason={reason})");
            // 留痕：接管态被谁清掉、为什么清，是现场争议最多的一件事
            sqlx::query(
                "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
                 VALUES ('device_event', ?1, ?2,
                     (SELECT id FROM devices WHERE device_id = ?3), NULL, ?4)",
            )
            .bind(format!("设备 {device_id} 接管态已释放"))
            .bind(format!(
                "客户端上报本地已不持有控制权（{reason}）：本体链路重建后 RoamerX 控制权失效"
            ))
            .bind(device_id)
            .bind(&timestamp)
            .execute(db)
            .await
            .ok();
        }
        Ok(_) => {}
        Err(error) => warn!("device {device_id} teleop_state update failed: {error}"),
    }
}

/// 下发待执行任务（设备注册 / 绑定园区 / 心跳确认本体就绪时调用）。
///
/// 三条规则：
/// - 设备未就绪（未绑园区 / 本体未连接 / 急停 / 控制权被抢 / 远程接管）时一条都不发：
///   注册时本体往往还没连上，早发只会得到一条「未连接机器人本体」，任务在平台上
///   看起来就是「已下发但不动」；
/// - 设备已有执行中 / 已暂停的任务时不补发：客户端只有一个执行上下文，后发的一条
///   会顶掉前一条，但平台侧两条都显示执行中；
/// - 一次只补发一条（优先级高、创建早的先跑），其余等这条上报终态后再补。
async fn dispatch_pending_tasks(
    db: &SqlitePool,
    tx: &broadcast::Sender<PendingCommand>,
    device_id: &str,
) -> Result<(), AppError> {
    let device: Option<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, park_id FROM devices WHERE device_id = ?1")
            .bind(device_id)
            .fetch_optional(db)
            .await?;
    let Some((device_pk, park_id)) = device else {
        return Ok(());
    };
    // 未绑定园区的设备不接收任务：地图与路线都按园区归属，绑定后才会补发
    let Some(park_id) = park_id else {
        return Ok(());
    };
    if let Some(reason) = crate::dispatch::device_block_reason(db, device_pk).await? {
        debug!("device {device_id} 暂不补发任务：{reason}");
        return Ok(());
    }

    let occupied: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM tasks WHERE device_id = ?1
           AND task_status IN ('执行中', '已暂停') LIMIT 1",
    )
    .bind(device_pk)
    .fetch_optional(db)
    .await?;
    if occupied.is_some() {
        return Ok(());
    }

    // 只取一条：优先级高、创建早的先跑
    let task = sqlx::query_as::<_, Task>(
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
         ORDER BY t.level DESC, t.created_at ASC, t.id ASC LIMIT 1",
    )
    .bind(device_id)
    .bind(park_id)
    .fetch_optional(db)
    .await?;
    let Some(task) = task else {
        return Ok(());
    };

    let dispatch = match crate::dispatch::build_dispatch_payload(db, &task).await {
        Ok(payload) => payload,
        Err(e) => {
            warn!("build dispatch payload for task {}: {e}", task.task_code);
            return Ok(());
        }
    };
    let payload = serde_json::to_value(&dispatch).unwrap_or_default();

    // 先置态再发：与 `push_task_dispatch` 保持一致，客户端拒单的上报不会被覆盖。
    // 重新下发等于换一条本体任务，旧的 robot_task_id / stage 必须清掉。
    let timestamp = now();
    sqlx::query(
        "UPDATE tasks SET task_status = '执行中', progress = 0, progress_updated_at = ?1,
            robot_task_id = NULL, robot_stage = NULL, robot_updated_at = NULL, updated_at = ?1
         WHERE id = ?2",
    )
    .bind(&timestamp)
    .bind(task.id)
    .execute(db)
    .await?;

    if tx
        .send(PendingCommand {
            cmd_type: "task_dispatch".to_string(),
            payload,
        })
        .is_err()
    {
        // 连接已经没了：回滚成待执行，别留下一条假的「执行中」
        sqlx::query(
            "UPDATE tasks SET task_status = '待执行', progress = 0, updated_at = ?1
             WHERE id = ?2 AND task_status = '执行中' AND robot_task_id IS NULL",
        )
        .bind(now())
        .bind(task.id)
        .execute(db)
        .await
        .ok();
        warn!(
            "device {device_id} 补发任务 {} 失败：连接已断开",
            task.task_code
        );
        return Ok(());
    }

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

/// 台账状态自愈：连接池里没有该设备时，把「在线」状态纠正为「离线」。
///
/// `devices.status` 是给业务控制台和中台看的展示态，连接池才是设备在线的唯一真相。
/// 进程重启、旧连接残留或历史版本的清理竞态都会让台账停在「在线待命 / 充电中」，
/// 运维在中台看到在线却收到「设备未连接」，所以下发失败时顺手把状态纠正过来。
///
/// 返回是否真的改了状态（没改说明本来就是离线 / 待绑定 / 故障）。
pub async fn mark_offline_if_online(db: &SqlitePool, device_code: &str) -> bool {
    match sqlx::query(
        "UPDATE devices SET status = '离线', body_online = 0,
            teleop_holder = NULL, teleop_at = NULL, teleop_force = 0, updated_at = ?1
         WHERE device_id = ?2 AND status IN ('在线待命', '充电中', '执行任务')",
    )
    .bind(now())
    .bind(device_code)
    .execute(db)
    .await
    {
        Ok(result) => result.rows_affected() > 0,
        Err(error) => {
            warn!("mark device {device_code} offline failed: {error}");
            false
        }
    }
}

/// 向指定设备发送远程指令
/// 设备是否还有活跃的业务 WebSocket 连接。
///
/// 用于区分「设备离线，指令未送达」和「设备在线但本体不健康」两种错误：
/// 前者保留原有 `wsDelivered=false` 语义，后者直接返回可读的健康门禁原因。
pub async fn is_device_connected(connections: &DeviceConnections, device_id: &str) -> bool {
    connections.read().await.contains_key(device_id)
}

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
    use super::{
        DeviceEventPayload, derive_device_status, event_alert_level, is_current_connection,
        release_connection,
    };
    use crate::gateway_types::PendingCommand;
    use std::collections::HashMap;
    use tokio::sync::broadcast;

    type Pool = HashMap<String, broadcast::Sender<PendingCommand>>;

    fn channel() -> broadcast::Sender<PendingCommand> {
        // 保留 receiver 否则通道无接收者（本用例只关心 sender 身份）
        let (tx, _rx) = broadcast::channel(8);
        tx
    }

    #[test]
    fn stale_connection_cannot_unregister_the_new_one() {
        // 现场故障复现：客户端重连，旧 TCP 还没死透。
        // 旧连接退出时如果无条件 remove，就会把新连接摘走，台账仍是「在线待命」，
        // 中台「同步地图」却报「设备未连接」。
        let device = "0202222672400230";
        let stale = channel();
        let fresh = channel();
        let mut pool: Pool = HashMap::new();

        // 旧连接注册 → 新连接注册（顶替）
        pool.insert(device.to_string(), stale.clone());
        pool.insert(device.to_string(), fresh.clone());
        assert!(is_current_connection(&pool, device, &fresh));

        // 旧连接退出：不得摘掉新登记
        assert!(!release_connection(&mut pool, device, &stale));
        assert!(
            is_current_connection(&pool, device, &fresh),
            "新连接必须还在连接池里"
        );

        // 新连接自己退出才真的摘掉
        assert!(release_connection(&mut pool, device, &fresh));
        assert!(!pool.contains_key(device));
    }

    #[test]
    fn heartbeat_can_re_register_connection() {
        // 心跳自愈：池里没登记本连接时就补登记（进程重启 / 历史残留状态）
        let device = "RD-001";
        let tx = channel();
        let mut pool: Pool = HashMap::new();
        assert!(!is_current_connection(&pool, device, &tx));
        pool.insert(device.to_string(), tx.clone());
        assert!(is_current_connection(&pool, device, &tx));
    }

    #[test]
    fn unbound_device_stays_pending() {
        // 未绑园区：不管充不充电，都必须保持「待绑定」，否则会被任务调度选中
        assert_eq!(
            derive_device_status("待绑定", false, Some(true), Some(80), None),
            None
        );
        assert_eq!(
            derive_device_status("充电中", false, Some(true), Some(80), None),
            Some("待绑定")
        );
    }

    #[test]
    fn charging_comes_from_body_report() {
        assert_eq!(
            derive_device_status("在线待命", true, Some(true), Some(88), None),
            Some("充电中")
        );
        assert_eq!(
            derive_device_status("充电中", true, Some(false), Some(88), None),
            Some("在线待命")
        );
        // 电量低但本体说没在充电：不再瞎猜成「充电中」
        assert_eq!(
            derive_device_status("在线待命", true, Some(false), Some(5), None),
            None
        );
        // 电量低且老客户端没上报充电状态：兼容兜底
        assert_eq!(
            derive_device_status("在线待命", true, None, Some(5), None),
            Some("充电中")
        );
    }

    #[test]
    fn running_task_and_manual_statuses_are_kept() {
        assert_eq!(
            derive_device_status("执行任务", true, Some(false), Some(60), None),
            None
        );
        assert_eq!(
            derive_device_status("执行任务", true, Some(true), Some(60), None),
            None
        );
        assert_eq!(
            derive_device_status("故障", true, Some(false), Some(60), None),
            None
        );
        assert_eq!(
            derive_device_status("离线", true, Some(false), Some(60), None),
            Some("在线待命")
        );
    }

    #[test]
    fn fault_severity_maps_to_alert_level() {
        let event = |event_type: &str, data: Option<serde_json::Value>| DeviceEventPayload {
            event_type: event_type.to_string(),
            message: "内部节点异常".to_string(),
            data,
        };
        // 13504 level=3 必须落「严重」，不能被普通 fault 淹没
        assert_eq!(
            event_alert_level(&event(
                "fault",
                Some(serde_json::json!({ "severity": 3 }))
            )),
            "严重"
        );
        assert_eq!(
            event_alert_level(&event(
                "fault",
                Some(serde_json::json!({ "severity": 1 }))
            )),
            "警告"
        );
        assert_eq!(event_alert_level(&event("fault", None)), "警告");
        assert_eq!(event_alert_level(&event("error", None)), "严重");
    }

    #[test]
    fn body_offline_is_fault_and_recovers() {
        // WS 还连着但本体 1315 超时：不能继续显示“在线待命”
        assert_eq!(
            derive_device_status("在线待命", true, Some(false), Some(80), Some(false)),
            Some("故障")
        );
        assert_eq!(
            derive_device_status("充电中", true, Some(true), Some(80), Some(false)),
            Some("故障")
        );
        // 本体恢复后自动回到待命
        assert_eq!(
            derive_device_status("故障", true, Some(false), Some(80), Some(true)),
            Some("在线待命")
        );
    }
}
