//! 指令下发与回执：`requestId → oneshot` 等待表、各类 `send_*`、在线状态查询。

// 同一模块拆出来的几块共用一个词汇表（`DeviceConnections` / `ConnectionCtx` / `send_msg` …），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/// 设备执行一条指令后的真实结果（由客户端 `command_result` 消息回报）。
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub ok: bool,
    pub message: String,
    pub data: Option<Value>,
}

/// 等待设备回执的请求表：平台 `requestId` → 结果通道。
///
/// 指令下发成功（`wsDelivered=true`）只代表帧进了设备连接，不代表设备执行成功。
/// 需要「真正结果」的调用方（目前是「下发地图」）先登记一个通道，客户端执行完
/// 通过 `command_result` 回报，再由这里唤醒等待方。
pub type CommandResultWaiters = Arc<Mutex<HashMap<String, oneshot::Sender<CommandResult>>>>;
pub type CommandResultReceiver = oneshot::Receiver<CommandResult>;

/// 「同步媒体」进度文案：中台设备页直接展示 `message`，所以这里把
/// 「已传 / 总数 / 第几个包」拼成一句人看得懂的话。
pub(crate) fn media_sync_progress_text(
    payload: &crate::gateway_types::CommandProgressPayload,
) -> String {
    let total = payload.total;
    let uploaded = payload.uploaded;
    let mut text = if total > 0 {
        format!("同步中：已上传 {uploaded}/{total} 张")
    } else {
        format!("同步中：已上传 {uploaded} 张")
    };
    if payload.batches_total > 0 {
        text.push_str(&format!(
            "（第 {}/{} 个压缩包）",
            payload.batches_done, payload.batches_total
        ));
    }
    if payload.failed > 0 {
        text.push_str(&format!("，失败 {} 张", payload.failed));
    }
    text
}

/// 把「同步媒体」的最终结果回报给中台审计（异步后台跑，不堵 WS 循环）。
pub(crate) fn report_media_sync_to_zentai(
    state: &AppState,
    device_id: &str,
    payload: &crate::gateway_types::CommandResultPayload,
) {
    let body = json!({
        "deviceCode": device_id,
        "requestId": payload.request_id.trim(),
        "status": if payload.ok { "succeeded" } else { "failed" },
        "message": payload.message.as_str(),
        "detail": payload.extra,
    });
    let config = state.config.clone();
    tokio::spawn(async move {
        crate::zentai::try_report_media_sync(&config, body).await;
    });
}

pub fn new_command_result_waiters() -> CommandResultWaiters {
    Arc::new(Mutex::new(HashMap::new()))
}

/// 在发送指令前登记等待；必须早于 `send_command_to_device`，否则客户端回执
/// 可能比登记更早到达而被丢弃。
pub async fn expect_command_result(
    waiters: &CommandResultWaiters,
    request_id: &str,
) -> CommandResultReceiver {
    let (tx, rx) = oneshot::channel();
    waiters.lock().await.insert(request_id.to_string(), tx);
    rx
}

/// 指令没有发出去（设备离线 / 广播通道关闭）时撤销等待，避免残留。
pub async fn cancel_command_result(waiters: &CommandResultWaiters, request_id: &str) {
    waiters.lock().await.remove(request_id);
}

/// 客户端回执到达：唤醒等待方；迟到（已超时）的回执只记日志。
pub async fn complete_command_result(
    waiters: &CommandResultWaiters,
    request_id: &str,
    result: CommandResult,
) {
    let sender = waiters.lock().await.remove(request_id);
    match sender {
        Some(sender) => {
            let _ = sender.send(result);
        }
        None => debug!(
            request_id,
            "收到没有等待方的指令回执（可能已超时或调用方未要求等待）"
        ),
    }
}

/// 等待客户端执行结果；超时会清掉等待表，避免后续回执命中旧请求。
pub async fn wait_command_result(
    waiters: &CommandResultWaiters,
    request_id: &str,
    receiver: CommandResultReceiver,
    timeout: std::time::Duration,
) -> Result<CommandResult, String> {
    match tokio::time::timeout(timeout, receiver).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err("设备回执通道已关闭".to_string()),
        Err(_) => {
            waiters.lock().await.remove(request_id);
            Err(format!(
                "设备已收到指令，但 {} 秒内未回报执行结果",
                timeout.as_secs()
            ))
        }
    }
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

/// 向指定设备发送任意业务消息（航点 AI 的 `ai_start` / `ai_stop` 用它）。
///
/// 与 `dispatch_task_to_device` 同一条广播通道，返回「是否真的送进了设备连接」——
/// 设备不在线时调用方必须能感知（否则会误以为窗口已经开了）。
pub async fn send_message_to_device(
    connections: &DeviceConnections,
    device_id: &str,
    msg_type: &str,
    payload: Value,
) -> bool {
    let map = connections.read().await;
    let Some(tx) = map.get(device_id) else {
        return false;
    };
    match tx.send(PendingCommand {
        cmd_type: msg_type.to_string(),
        payload,
    }) {
        Ok(_) => {
            debug!("sent {msg_type} to device {device_id}");
            true
        }
        Err(_) => {
            warn!("failed to send {msg_type} to device {device_id} (no receivers)");
            false
        }
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
