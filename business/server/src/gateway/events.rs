//! 本体事件与遥控接管：故障码过滤、事件转告警、`teleop_state`。

// 同一模块拆出来的几块共用一个词汇表（`DeviceConnections` / `ConnectionCtx` / `send_msg` …），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/// 从本体故障文案里取故障码（`感知障碍物检测延迟（code=13382, 级别=1）` → `13382`）。
///
/// 拿它当自动工单的去重键：同一台设备的**同一个故障码**就是同一个问题，
/// 而文案里的坐标/耗时/英文描述会变，按标题去重会拆成好几张单。
/// 取不到码（翻译后的文案没有 `code=`）时返回 `None`，工单退回按标题去重。
///
/// 告警白名单也用它取码（[`crate::alert_flow::message_ignored`]），所以是
/// `pub(crate)`：两处必须用同一套解析，否则「去重按 A 码、白名单按 B 码」。
pub(crate) fn fault_code(message: &str) -> Option<&str> {
    let rest = message.split("code=").nth(1)?.trim_start();
    let end = rest
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// 设备事件 → 告警级别。
///
/// 以前所有 `fault` 都写成「警告」，4021 的 `severity=3`（如 13504 内部节点异常）
/// 也被淹没；这里优先读事件 data 里的 `severity` / `level`，>=2 视为严重。
pub(crate) fn event_alert_level(payload: &DeviceEventPayload) -> &'static str {
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
pub(crate) async fn handle_device_event(
    state: &crate::app::AppState,
    device_id: &str,
    payload: &DeviceEventPayload,
) {
    let db = &state.db;
    let timestamp = now();

    // 根据事件类型处理
    match payload.event_type.as_str() {
        "error" | "fault" | "obstacle" => {
            // 告警白名单（`settings.alert_ignore_fault_codes`）：现场明确不关心的
            // 高频故障码（如 12608 RS/lidar 时间不同步、13440 导航指令非法）
            // **不落告警、不建工单、不推企业微信**，只在下面的 `task_events`
            // 事件流里留一行。事件流不能一起省：`recent_capability_busy` 正是靠
            // task_events 里的 13440 / 13897 区分「本体能力被占」和「遥控器接管」。
            if crate::alert_flow::message_ignored(&state.db, &payload.message).await {
                info!(
                    device = %device_id,
                    content = %payload.message,
                    "故障码在告警白名单内：跳过告警/自动建单/企业微信推送（事件流照常留痕）"
                );
            } else {
                // alert_code 带设备号 + 秒级时间戳。
                // 以前是 `AL-WS-{日期}-{小时}`：`alert_code` 有 UNIQUE 约束，
                // 同一小时内的第二条故障事件插入直接失败（静默 `.ok()` 吞掉），
                // 告警中心一小时内只留得下**全局第一条**故障，其余全丢。
                let alert_code = format!(
                    "AL-WS-{}-{}",
                    device_id,
                    timestamp.replace(['-', ' ', ':'], "")
                );
                // 同一台设备的同一个故障码 = 同一个问题：拿它当自动工单的去重键。
                // 以前按「设备号 + 内容前 60 字」比标题，内容里的坐标/时间一变就拆成两张单。
                let dedup_key = fault_code(&payload.message)
                    .map(|code| format!("fault:{{device}}:{code}"))
                    .unwrap_or_default();
                sqlx::query(
                    "INSERT INTO alerts (alert_code, level, content, device_id, park_id, status, occurred_at, created_at, dedup_key)
                     VALUES (?1, ?2, ?3,
                         (SELECT id FROM devices WHERE device_id = ?4),
                         (SELECT park_id FROM devices WHERE device_id = ?4),
                         '未处理', ?5, ?5,
                         NULLIF(REPLACE(?6, '{device}', COALESCE(CAST((SELECT id FROM devices WHERE device_id = ?4) AS TEXT), '')), ''))",
                )
                .bind(&alert_code)
                .bind(event_alert_level(payload))
                .bind(&payload.message)
                .bind(device_id)
                .bind(&timestamp)
                .bind(&dedup_key)
                .execute(db)
                .await
                .ok();
                crate::notify::alert_created(&state.notify(), &alert_code).await;
            }
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
pub(crate) async fn handle_teleop_state(
    db: &SqlitePool,
    device_id: &str,
    payload: &TeleopStatePayload,
) {
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
