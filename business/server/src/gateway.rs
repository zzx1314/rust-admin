use crate::app::AppState;
use crate::db::{now, today};
use crate::error::AppError;
use crate::gateway_types::{
    CommandPayload, DeviceEventPayload, HeartbeatAckPayload, HeartbeatPayload,
    PendingCommand, RegisterAckPayload, RegisterPayload, TaskDispatchPayload,
    TaskProgressPayload, WsMessage,
};
use crate::models::{Device, Task};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::HashMap;
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
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// 处理单个设备 WebSocket 连接
async fn handle_socket(socket: WebSocket, state: AppState) {
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
            let message = WsMessage {
                msg_type: match cmd.cmd_type.as_str() {
                    "task_dispatch" => "task_dispatch".to_string(),
                    "task_control" => "task_control".to_string(),
                    "remote_command" => "command".to_string(),
                    _ => "command".to_string(),
                },
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
                                match handle_register(&db, &payload).await {
                                    Ok(device) => {
                                        let did = payload.device_id.clone();
                                        let old_device_id = device_id.replace(did.clone());

                                        // 如果该设备已有旧连接，将旧 sender 移除
                                        if let Some(ref old) = old_device_id {
                                            connections.write().await.remove(old);
                                        }
                                        connections.write().await.insert(did.clone(), tx.clone());

                                        info!("device registered: {} / {}", did, device.device_id);
                                        send_msg(&tx, "register_ack", json!(RegisterAckPayload {
                                            success: true,
                                            message: "注册成功".to_string(),
                                            device_id: did.clone(),
                                            device_name: Some(device.device_id),
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
                                            signal: None,
                                            temperature: None,
                                            position: None,
                                            odometer: None,
                                            runtime_minutes: None,
                                            current_task: None,
                                            firmware: None,
                                        });
                                    handle_heartbeat(&db, did, &payload).await;
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
        // 标记设备离线
        sqlx::query("UPDATE devices SET status = '离线', updated_at = ?1 WHERE device_id = ?2")
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
    let msg = WsMessage {
        msg_type: msg_type.to_string(),
        ts: now(),
        payload,
    };
    let text = serde_json::to_string(&msg).unwrap_or_default();
    let _ = tx.send(PendingCommand {
        cmd_type: msg_type.to_string(),
        payload: json!(text),
    });
}

// ─── 业务处理函数 ──────────────────────────────────────────────

/// 设备注册
async fn handle_register(
    db: &SqlitePool,
    payload: &RegisterPayload,
) -> Result<Device, AppError> {
    let device_id = payload.device_id.trim();
    if device_id.is_empty() {
        return Err(AppError::BadRequest("设备编号不能为空".to_string()));
    }

    let device = sqlx::query_as::<_, Device>(
        "SELECT d.id, d.device_id, d.model, d.park_id, p.park_name,
            d.zone_id, z.zone_name, d.status, d.battery, d.signal, d.temperature,
            d.runtime_minutes, d.odometer, d.position, d.current_task_name,
            d.last_heartbeat, d.firmware, d.device_ip, d.created_at, d.updated_at
         FROM devices d
         LEFT JOIN parks p ON p.id = d.park_id
         LEFT JOIN zones z ON z.id = d.zone_id
         WHERE d.device_id = ?1",
    )
    .bind(device_id)
    .fetch_optional(db)
    .await?;

    let device = device.ok_or_else(|| AppError::NotFound("设备不存在，请检查设备编号".to_string()))?;

    // 更新设备状态为在线
    let timestamp = now();
    sqlx::query(
        "UPDATE devices SET status = CASE WHEN status = '充电中' THEN '充电中' ELSE '在线待命' END,
            last_heartbeat = ?1, updated_at = ?1 WHERE id = ?2",
    )
    .bind(&timestamp)
    .bind(device.id)
    .execute(db)
    .await?;

    Ok(device)
}

/// 心跳处理
async fn handle_heartbeat(db: &SqlitePool, device_id: &str, payload: &HeartbeatPayload) {
    let timestamp = now();

    let status = match payload.battery {
        Some(b) if b <= 10 => Some("充电中"),
        _ => None,
    };

    // 使用带有参数绑定的 SQL 更新
    let timestamp_clone = timestamp.clone();
    let device_id_clone = device_id.to_string();

    // 分别更新各个字段，避免复杂的动态 SQL
    let _ = sqlx::query(
        "UPDATE devices SET last_heartbeat = ?1, updated_at = ?1 WHERE device_id = ?2"
    )
    .bind(&timestamp)
    .bind(device_id)
    .execute(db).await;

    if let Some(b) = payload.battery {
        let _ = sqlx::query("UPDATE devices SET battery = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(b)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    }
    if let Some(s) = payload.signal {
        let _ = sqlx::query("UPDATE devices SET signal = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(s)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    }
    if let Some(t) = payload.temperature {
        let _ = sqlx::query("UPDATE devices SET temperature = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(t)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    }
    if let Some(pos) = &payload.position {
        let _ = sqlx::query("UPDATE devices SET position = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(pos)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    }
    if let Some(o) = payload.odometer {
        let _ = sqlx::query("UPDATE devices SET odometer = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(o)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    }
    if let Some(r) = payload.runtime_minutes {
        let _ = sqlx::query("UPDATE devices SET runtime_minutes = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(r)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    }
    if let Some(task) = &payload.current_task {
        let _ = sqlx::query("UPDATE devices SET current_task_name = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(task)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    }
    if let Some(fw) = &payload.firmware {
        let _ = sqlx::query("UPDATE devices SET firmware = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(fw)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    }
    if let Some(s) = status {
        let _ = sqlx::query("UPDATE devices SET status = ?1, updated_at = ?2 WHERE device_id = ?3")
            .bind(s)
            .bind(&timestamp_clone)
            .bind(&device_id_clone)
            .execute(db).await;
    } else {
        let _ = sqlx::query(
            "UPDATE devices SET status = CASE WHEN status = '离线' THEN '在线待命' ELSE status END,
             updated_at = ?1 WHERE device_id = ?2"
        )
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db).await;
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
            "UPDATE tasks SET task_status = ?1, progress = ?2, updated_at = ?3 WHERE task_code = ?4",
        )
        .bind(status)
        .bind(payload.progress)
        .bind(&timestamp)
        .bind(&payload.task_code)
        .execute(db)
        .await;

        if let Ok(r) = result {
            if r.rows_affected() > 0 {
                // 记录事件
                let message = payload.message.as_deref().unwrap_or("进度更新");
                sqlx::query(
                    "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
                     VALUES ('progress', ?1, ?2, (SELECT id FROM devices WHERE device_id = ?3),
                             (SELECT id FROM tasks WHERE task_code = ?4), ?5)",
                )
                .bind(format!("任务 {} 进度更新: {}%", payload.task_code, payload.progress))
                .bind(message)
                .bind(device_id)
                .bind(&payload.task_code)
                .bind(&timestamp)
                .execute(db)
                .await
                .ok();
            }
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
            let alert_code = format!("AL-WS-{}-{}", date_prefix.replace("-", ""), &timestamp[11..13]);

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
    // 查找该设备所有"待执行"状态的任务
    let tasks = sqlx::query_as::<_, Task>(
        "SELECT t.id, t.task_code, t.task_name, t.task_type, t.execute_mode, t.level,
            t.device_id, d.device_id AS device_code, t.park_id, p.park_name, t.zone_id,
            z.zone_name, t.route_id, r.route_code, r.route_name, t.plan_start, t.plan_end,
            t.task_status, t.progress, t.dispatch_policy, t.description, t.created_at, t.updated_at
         FROM tasks t
         LEFT JOIN devices d ON d.id = t.device_id
         LEFT JOIN parks p ON p.id = t.park_id
         LEFT JOIN zones z ON z.id = t.zone_id
         LEFT JOIN routes r ON r.id = t.route_id
         WHERE d.device_id = ?1 AND t.task_status = '待执行'
         ORDER BY t.level DESC, t.created_at ASC",
    )
    .bind(device_id)
    .fetch_all(db)
    .await?;

    for task in tasks {
        // 获取路线数据
        let route_data: Option<Value> = if let Some(route_id) = task.route_id {
            let row: Option<String> =
                sqlx::query_scalar("SELECT route_data FROM routes WHERE id = ?1")
                    .bind(route_id)
                    .fetch_optional(db)
                    .await
                    .ok()
                    .flatten();
            row.and_then(|s| serde_json::from_str(&s).ok())
        } else {
            None
        };

        let dispatch = TaskDispatchPayload {
            task_id: task.id,
            task_code: task.task_code.clone(),
            task_name: task.task_name.clone(),
            task_type: task.task_type.clone(),
            execute_mode: task.execute_mode.clone(),
            route: route_data.clone().map(|_| json!({
                "routeId": task.route_id,
                "routeCode": task.route_code,
                "routeName": task.route_name,
            })),
            route_data,
            plan_start: task.plan_start.clone(),
            plan_end: task.plan_end.clone(),
            dispatch_policy: task.dispatch_policy.clone(),
            description: task.description.clone(),
        };

        let payload = serde_json::to_value(&dispatch).unwrap_or_default();
        let msg = WsMessage {
            msg_type: "task_dispatch".to_string(),
            ts: now(),
            payload,
        };
        let text = serde_json::to_string(&msg).unwrap_or_default();
        let _ = tx.send(PendingCommand {
            cmd_type: "task_dispatch".to_string(),
            payload: json!(text),
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
        let msg = WsMessage {
            msg_type: "task_dispatch".to_string(),
            ts: now(),
            payload,
        };
        let text = serde_json::to_string(&msg).unwrap_or_default();
        match tx.send(PendingCommand {
            cmd_type: "task_dispatch".to_string(),
            payload: json!(text),
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
        let msg = WsMessage {
            msg_type: "command".to_string(),
            ts: now(),
            payload: serde_json::to_value(payload).unwrap_or_default(),
        };
        let text = serde_json::to_string(&msg).unwrap_or_default();
        match tx.send(PendingCommand {
            cmd_type: "remote_command".to_string(),
            payload: json!(text),
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

/// 获取在线设备列表
pub async fn get_online_devices(connections: &DeviceConnections) -> Vec<String> {
    let map = connections.read().await;
    map.keys().cloned().collect()
}

/// 检查设备是否在线
pub async fn is_device_online(connections: &DeviceConnections, device_id: &str) -> bool {
    let map = connections.read().await;
    map.contains_key(device_id)
}
