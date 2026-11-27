use crate::app::AppState;
use crate::db::now;
use crate::error::AppError;
use crate::gateway_types::{
    AiDetectPayload, AiEndPayload, AiPointPayload, CommandPayload, DeviceEventPayload,
    HeartbeatAckPayload, HeartbeatPayload, PendingCommand, RegisterAckPayload, RegisterPayload,
    TaskControlPayload, TaskDispatchPayload, TaskProgressPayload, TeleopStatePayload, WsMessage,
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
use tokio::sync::{Mutex, RwLock, broadcast, oneshot};
use tracing::{debug, error, info, warn};
mod commands;
mod device;
mod events;
mod tasks;
#[cfg(test)]
mod tests;

// 对外 API（`crate::gateway::*`）：别的模块真正用到的那几个。
pub use commands::{
    CommandResultWaiters, cancel_command_result, complete_command_result, dispatch_task_to_device,
    expect_command_result, get_online_devices, is_device_connected, is_device_online,
    mark_offline_if_online, new_command_result_waiters, send_command_to_device,
    send_message_to_device, send_task_control_to_device, send_teleop_to_device,
    wait_command_result,
};
pub use device::{
    dispatch_pending_for_device, notify_park_binding, spawn_heartbeat_watchdog, sweep_stale_devices,
};
pub use tasks::{spawn_task_watchdog, sweep_stuck_tasks};

// crate 内共享：协议处理函数与子模块之间互相调用用。
#[allow(clippy::wildcard_imports)]
pub(crate) use commands::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use device::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use events::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use tasks::*;

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
                        let ctx = ConnectionCtx {
                            state: &state,
                            tx: &tx,
                            peer_ip: peer_ip.as_deref(),
                            connections: &connections,
                        };
                        match serde_json::from_str::<WsMessage>(&text) {
                            Ok(message) => route_message(&ctx, &mut device_id, message).await,
                            Err(error) => {
                                warn!("invalid WS message: {error}");
                                send_msg(
                                    &tx,
                                    "error",
                                    json!({"message": format!("invalid message: {error}")}),
                                )
                                .await;
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

// ─── 消息路由与逐类处理 ────────────────────────────────────────

/// 一条设备连接上的上下文。
///
/// 拆出 [`route_message`] 之后，每种消息都要「知道自己在哪条连接上」：
/// 往哪条通道回消息、对端 IP 是什么、连接池在哪。打包成一个结构传，
/// 比六个位置参数清楚，也避免注册前就把 `device_id` 当成已注册设备用。
struct ConnectionCtx<'a> {
    state: &'a AppState,
    /// 本连接的下发通道（转发任务把它变成 WebSocket 帧）
    tx: &'a broadcast::Sender<PendingCommand>,
    /// 接入对端 IP（只在 `register` 时落库）
    peer_ip: Option<&'a str>,
    /// 设备连接池（`device_id → sender`）
    connections: &'a DeviceConnections,
}

/// 取「已注册的设备号」；未注册时记一条日志并忽略这条消息。
///
/// 以前每个分支都写一遍 `if let Some(ref did) = device_id { … }`，
/// 未注册的消息被静默丢掉；现在至少留一行日志，排查「客户端发了但平台没反应」时有用。
fn registered<'a>(device_id: Option<&'a str>, msg_type: &str) -> Option<&'a str> {
    if let Some(device) = device_id {
        return Some(device);
    }
    warn!("{msg_type} 来自尚未注册的连接，忽略");
    None
}

/// 一条文本帧的路由：**只做分发**，业务在各自的 `on_*` 里。
///
/// 拆之前这里是 470 行的一个 `match`（心跳状态推导、任务状态机、AI 开窗、
/// 隧道登记、指令回执全挤在里面），改哪种消息都要在同一个函数里翻半天。
async fn route_message(
    ctx: &ConnectionCtx<'_>,
    device_id: &mut Option<String>,
    message: WsMessage,
) {
    match message.msg_type.as_str() {
        "register" => on_register(ctx, device_id, message.payload).await,
        "heartbeat" => on_heartbeat(ctx, device_id.as_deref(), message.payload).await,
        "task_progress" => {
            if let Some(device) = registered(device_id.as_deref(), "task_progress") {
                on_task_progress(ctx, device, message.payload).await;
            }
        }
        // ── 航点 AI 识别（客户端代调板端 Agent）──────────────
        // 正式环境业务平台在公网、背包 Agent 只在内网，服务端连不上；
        // 客户端与 Agent 同机，由它探测/启停并把检出回传。
        "ai_point" => {
            if let Some(device) = registered(device_id.as_deref(), "ai_point") {
                on_ai_point(ctx, device, message.payload).await;
            }
        }
        "ai_detect" => {
            if let Some(device) = registered(device_id.as_deref(), "ai_detect") {
                on_ai_detect(ctx, device, message.payload).await;
            }
        }
        "ai_end" => {
            if let Some(device) = registered(device_id.as_deref(), "ai_end") {
                on_ai_end(ctx, device, message.payload).await;
            }
        }
        // ── 板端 Agent 隧道（`agent_tunnel.rs`）──────────────
        // 设备客户端跑在背包那块板子上，`127.0.0.1:8000` 一定通。
        // 它上线时打个招呼，服务端据此知道「这台设备能代调 Agent」。
        "agent_hello" => on_agent_hello(ctx, device_id.as_deref(), message.payload).await,
        // 隧道回执：唤醒 `/api/backpack/*` 那个正在等的 HTTP 请求
        "agent_result" => on_agent_result(ctx, message.payload).await,
        // Agent SSE 事件（页面画框 / 刷新状态）：带上来源设备转发给背包页
        "agent_event" => ctx
            .state
            .agent_tunnel
            .publish(device_id.as_deref(), &message.payload),
        // 数量核对不一致（板端已判定）：告警落库 + 企业微信
        "ai_alert" => {
            if let Some(device) = registered(device_id.as_deref(), "ai_alert") {
                on_ai_alert(ctx, device, message.payload).await;
            }
        }
        // 数量核对一致（勾了「数量一致也通知」才有）：只推企业微信群，
        // 不落告警、不建工单
        "ai_count" => {
            if let Some(device) = registered(device_id.as_deref(), "ai_count") {
                on_ai_count(ctx, device, message.payload).await;
            }
        }
        // 命中帧晚落盘：卡片已发过，只补一张证据图
        "ai_evidence" => {
            if let Some(device) = registered(device_id.as_deref(), "ai_evidence") {
                on_ai_evidence(ctx, device, message.payload).await;
            }
        }
        "command_result" => {
            if let Some(device) = registered(device_id.as_deref(), "command_result") {
                on_command_result(ctx, device, message.payload).await;
            }
        }
        // 长任务进度（目前只有「同步媒体」）：只回写状态，不动等待表
        "command_progress" => {
            if let Some(device) = registered(device_id.as_deref(), "command_progress") {
                on_command_progress(ctx, device, message.payload);
            }
        }
        "event" => {
            if let Some(device) = registered(device_id.as_deref(), "event") {
                on_device_event(ctx, device, message.payload).await;
            }
        }
        // ── 网页对讲（`phone_audio.rs`）──────────────────────
        // 客户端把板端 UDP 的 PCM 转成 base64 传上来，平台只做转发（不落库、
        // 不混音）；状态（started / error / stopped）一并转给正在通话的浏览器。
        "phone_audio" => {
            if let Some(device) = registered(device_id.as_deref(), "phone_audio") {
                ctx.state
                    .phone_audio
                    .publish_audio(device, &message.payload);
            }
        }
        "phone_stream" => {
            if let Some(device) = registered(device_id.as_deref(), "phone_stream") {
                ctx.state
                    .phone_audio
                    .publish_state(device, &message.payload);
            }
        }
        // 响应服务端 ping，不做处理
        "pong" => {}
        "teleop_state" => {
            if let Some(device) = registered(device_id.as_deref(), "teleop_state") {
                on_teleop_state(ctx, device, message.payload).await;
            }
        }
        other => {
            warn!("unknown message type: {other}");
            send_msg(
                ctx.tx,
                "error",
                json!({"message": format!("unknown type: {other}")}),
            )
            .await;
        }
    }
}

/// `register`：建档 / 上线、登记连接池、回 `register_ack`、补发待执行任务。
async fn on_register(ctx: &ConnectionCtx<'_>, device_id: &mut Option<String>, payload: Value) {
    let state = ctx.state;
    let db = &state.db;
    let tx = ctx.tx;

    let payload: RegisterPayload = match serde_json::from_value(payload) {
        Ok(payload) => payload,
        Err(error) => {
            warn!("invalid register payload: {error}");
            send_msg(
                tx,
                "error",
                json!({"message": format!("invalid register payload: {error}")}),
            )
            .await;
            return;
        }
    };
    match handle_register(db, &payload, ctx.peer_ip).await {
        Ok(device) => {
            // 设备接入后同步到中台台账，便于在中台分配用户
            sync_device_to_zentai(&state.config, db, &device).await;
            let did = payload.device_id.clone();
            let old_device_id = device_id.replace(did.clone());

            {
                let mut map = ctx.connections.write().await;
                // 换设备号时只摘掉自己上一次的登记，别碰别的连接；
                // 同一设备换连接直接覆盖（旧连接退出时靠通道身份判断，
                // 不会误删新登记）
                if let Some(ref old) = old_device_id {
                    release_connection(&mut map, old, tx);
                }
                map.insert(did.clone(), tx.clone());
            }

            info!(
                "device registered: {} / {} (park={:?})",
                did, device.device_id, device.park_id
            );
            let park_code = park_code_of(db, device.park_id).await;
            let unbound = device.park_id.is_none();
            send_msg(
                tx,
                "register_ack",
                json!(RegisterAckPayload {
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
                }),
            )
            .await;

            // 检查是否有待执行任务并立即下发
            if let Err(error) = dispatch_pending_tasks(db, tx, &did).await {
                warn!("dispatch pending tasks for {did}: {error}");
            }
        }
        Err(error) => {
            warn!("device register failed for {}: {error}", payload.device_id);
            send_msg(
                tx,
                "register_ack",
                json!(RegisterAckPayload {
                    success: false,
                    message: error.to_string(),
                    device_id: payload.device_id,
                    device_name: None,
                    park_id: None,
                    park_code: None,
                    park_name: None,
                    server_ts: now(),
                }),
            )
            .await;
        }
    }
}

/// `heartbeat`：连接池自愈、状态推导、补发任务、回 `heartbeat_ack`。
///
/// 未注册的连接也回一条 error（老客户端靠它知道要重发 `register`）。
async fn on_heartbeat(ctx: &ConnectionCtx<'_>, device_id: Option<&str>, payload: Value) {
    let state = ctx.state;
    let db = &state.db;
    let tx = ctx.tx;
    let Some(device) = device_id else {
        send_msg(tx, "error", json!({"message": "请先注册设备"})).await;
        return;
    };

    // 心跳自愈：连接池里登记的不是本连接（历史版本的清理竞态、进程重启）
    // 就补登记，否则台账显示在线、中台下发指令却永远报「设备未连接」。
    {
        let mut map = ctx.connections.write().await;
        if !is_current_connection(&map, device, tx) {
            warn!("device {device} heartbeat without pool entry, re-registering connection");
            map.insert(device.to_string(), tx.clone());
        }
    }
    let payload: HeartbeatPayload = serde_json::from_value(payload).unwrap_or(HeartbeatPayload {
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
        signal: None,
        signal_source: None,
    });
    handle_heartbeat(state, device, &payload).await;
    // 本体就绪后才补发待执行任务：注册时通常还没连上本体，
    // 早发只会得到一条「未连接机器人本体」并留下假执行中。
    if payload.body_online == Some(true)
        && let Err(error) = dispatch_pending_tasks(db, tx, device).await
    {
        warn!("dispatch pending tasks for {device}: {error}");
    }
    // 强制接管标记随心跳下发：客户端重启后本地的 `teleop_force` 会丢，
    // 不补的话摇杆又会被 `ctrl_source = 3` 门禁拦下。
    let teleop_force: bool =
        sqlx::query_scalar("SELECT COALESCE(teleop_force, 0) FROM devices WHERE device_id = ?1")
            .bind(device)
            .fetch_optional(db)
            .await
            .ok()
            .flatten()
            .unwrap_or(0)
            != 0;
    send_msg(
        tx,
        "heartbeat_ack",
        json!(HeartbeatAckPayload {
            server_ts: now(),
            pending_commands: vec![],
            teleop_force,
        }),
    )
    .await;
}

/// `task_progress`：任务状态机 + 航点 AI 窗口推进，回 `task_progress_ack`。
async fn on_task_progress(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: TaskProgressPayload =
        serde_json::from_value(payload).unwrap_or(TaskProgressPayload {
            task_id: None,
            task_code: String::new(),
            status: "unknown".to_string(),
            progress: 0,
            point_index: None,
            waypoint_index: None,
            message: None,
            robot_task_id: None,
            stage: None,
            total_points: None,
        });
    handle_task_progress(ctx.state, device, &payload).await;
    send_msg(ctx.tx, "task_progress_ack", json!({"received": true})).await;
}

/// `ai_point`：走到带「AI 识别」动作的航点 → 开识别窗口（客户端代调模式）。
async fn on_ai_point(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: AiPointPayload = serde_json::from_value(payload).unwrap_or_default();
    ctx.state
        .ai_patrol
        .on_client_point(ctx.state, device, &payload)
        .await;
}

/// `ai_detect`：实时检出（板端帧级） → 告警 + 证据图。
async fn on_ai_detect(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: AiDetectPayload = serde_json::from_value(payload).unwrap_or_default();
    ctx.state
        .ai_patrol
        .on_client_detect(ctx.state, device, &payload)
        .await;
}

/// `ai_end`：客户端宣布本轮识别结束（正常路径）。
async fn on_ai_end(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: AiEndPayload = serde_json::from_value(payload).unwrap_or_default();
    ctx.state
        .ai_patrol
        .on_client_end(ctx.state, device, &payload)
        .await;
}

/// `ai_alert`：数量核对不一致（板端已判定）→ 告警 + 企业微信。
async fn on_ai_alert(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: crate::gateway_types::AiAlertPayload =
        serde_json::from_value(payload).unwrap_or_default();
    ctx.state
        .ai_patrol
        .on_client_alert(ctx.state, device, &payload)
        .await;
}

/// `ai_count`：数量核对一致（勾了「数量一致也通知」才有）。
async fn on_ai_count(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: crate::gateway_types::AiCountPayload =
        serde_json::from_value(payload).unwrap_or_default();
    ctx.state
        .ai_patrol
        .on_client_count(ctx.state, device, &payload)
        .await;
}

/// `ai_evidence`：命中帧晚落盘 → 只补一张证据图（卡片已发过）。
async fn on_ai_evidence(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: crate::gateway_types::AiEvidencePayload =
        serde_json::from_value(payload).unwrap_or_default();
    ctx.state
        .ai_patrol
        .on_client_evidence(ctx.state, device, &payload)
        .await;
}

/// `agent_hello`：设备客户端登记「这台设备能代调板端 Agent」。
async fn on_agent_hello(ctx: &ConnectionCtx<'_>, device_id: Option<&str>, payload: Value) {
    let Some(device) = registered(device_id, "agent_hello") else {
        return;
    };
    let base = payload
        .get("base")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if base.is_empty() {
        warn!("设备 {device} 的 agent_hello 缺少 base，忽略");
    } else {
        ctx.state.agent_tunnel.hello(device, &base).await;
    }
}

/// `agent_result`：隧道回执，唤醒正在等的 HTTP 请求。
async fn on_agent_result(ctx: &ConnectionCtx<'_>, payload: Value) {
    let request_id = payload
        .get("requestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if request_id.is_empty() {
        warn!("agent_result 缺少 requestId，忽略: {payload}");
    } else {
        ctx.state.agent_tunnel.complete(&request_id, payload).await;
    }
}

/// `command_result`：唤醒指令等待表 + 「同步媒体」的结论回写中台。
async fn on_command_result(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let state = ctx.state;
    let payload: crate::gateway_types::CommandResultPayload =
        serde_json::from_value(payload).unwrap_or_default();
    if payload.request_id.trim().is_empty() {
        warn!("设备 {device} 的 command_result 缺少 requestId");
    } else {
        complete_command_result(
            &state.command_results,
            payload.request_id.trim(),
            CommandResult {
                ok: payload.ok,
                message: payload.message.clone(),
                data: payload.data.clone(),
            },
        )
        .await;
    }
    // 长任务（「同步媒体」/「回传录像」）：中台下发时只拿到「已送达」，真实进度/结论
    // 由这里回写中台审计（否则设备页只能猜）。
    if matches!(payload.command.as_str(), "同步媒体" | "回传录像") {
        report_long_task_to_zentai(state, device, &payload);
    }
    send_msg(ctx.tx, "command_result_ack", json!({"received": true})).await;
}

/// `command_progress`：长任务进度（「同步媒体」/「回传录像」）。
///
/// 只回写中台审计，不动 `command_results` 等待表 —— 等待表只关心「最终结论」。
fn on_command_progress(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: crate::gateway_types::CommandProgressPayload =
        serde_json::from_value(payload).unwrap_or_default();
    if payload.request_id.trim().is_empty() {
        warn!("设备 {device} 的 command_progress 缺少 requestId");
        return;
    }
    if !matches!(payload.command.as_str(), "同步媒体" | "回传录像") {
        return;
    }
    let body = long_task_progress_body(device, &payload);
    let config = ctx.state.config.clone();
    tokio::spawn(async move {
        crate::zentai::try_report_media_sync(&config, body).await;
    });
}

/// `event`：本体事件（导航失败、故障码…）→ 事件流 / 告警 / 任务状态兜底。
async fn on_device_event(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: DeviceEventPayload =
        serde_json::from_value(payload).unwrap_or(DeviceEventPayload {
            event_type: "unknown".to_string(),
            message: String::new(),
            data: None,
        });
    handle_device_event(ctx.state, device, &payload).await;
    send_msg(ctx.tx, "event_ack", json!({"received": true})).await;
}

/// `teleop_state`：客户端进程内重连本体后，业务 WS 没断、服务端收不到断连回调；
/// 必须靠这条消息把「平台以为还握着 RoamerX」的幽灵接管清掉。
async fn on_teleop_state(ctx: &ConnectionCtx<'_>, device: &str, payload: Value) {
    let payload: TeleopStatePayload =
        serde_json::from_value(payload).unwrap_or(TeleopStatePayload {
            holder: false,
            reason: None,
        });
    handle_teleop_state(&ctx.state.db, device, &payload).await;
    send_msg(ctx.tx, "teleop_state_ack", json!({"received": true})).await;
}

// ─── 消息发送助手 ──────────────────────────────────────────────

async fn send_msg(tx: &broadcast::Sender<PendingCommand>, msg_type: &str, payload: Value) {
    // payload 直接下发：外层 WsMessage 由转发任务统一组装，避免二次 JSON 编码
    let _ = tx.send(PendingCommand {
        cmd_type: msg_type.to_string(),
        payload,
    });
}
