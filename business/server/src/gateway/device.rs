//! 设备接入、心跳与设备级状态：注册建档、心跳推导、看门狗、接管/控制权。

// 同一模块拆出来的几块共用一个词汇表（`DeviceConnections` / `ConnectionCtx` / `send_msg` …），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

// ─── 业务处理函数 ──────────────────────────────────────────────

/// 设备注册
///
/// 新设备首次注册时自动建档（无园区，状态「待绑定」）：园区由运营在设备管理里
/// 手动绑定，绑定后心跳状态与待执行任务才会正常推进。
///
/// 客户端能上报的台账字段全部在此落库：`model`（本体 1201 `type`）、
/// `firmware`（客户端配置）、`device_ip`（本次接入的对端 IP）。
pub(crate) async fn handle_register(
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
             VALUES (?1, ?2, NULL, NULL, '待绑定', 100, 0, 32.0, 0, 0, NULL, '—',
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

/// 归一化信号来源：只认 `wifi` / `cellular`，其余（老客户端 / 写错）存 NULL
///
/// 老客户端只带 `signal` 不带来源，写入时 `signal_source` 会跟着置空 ——
/// 宁可页面不标「WiFi / 4G」，也不要让旧标签配新数值。
fn signal_source_of(raw: Option<&str>) -> Option<&'static str> {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("wifi") => Some("wifi"),
        Some("cellular") => Some("cellular"),
        _ => None,
    }
}

/// 把业务平台的设备行同步到中台台账（失败只记日志）。
pub(crate) async fn sync_device_to_zentai(
    config: &crate::config::Config,
    db: &SqlitePool,
    device: &Device,
) {
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
pub(crate) fn derive_device_status(
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
                occurred_at, created_at, dedup_key)
             VALUES (?1, '警告', ?2, ?3, ?4, '未处理', ?5, ?5, ?6)",
        )
        .bind(&alert_code)
        .bind(&content)
        .bind(id)
        .bind(park_id)
        .bind(&timestamp)
        // 同一台设备反复掉线 = 同一个问题，别再刷工单
        .bind(format!("hb:{id}"))
        .execute(&state.db)
        .await;
        // 落库后走统一出口；要不要建单 + 推群由 `alert_flow::should_dispatch` 判：
        // 掉线告警**只在设备当时有执行中任务**时才进群（巡检中掉线是最典型的失败场景），
        // 闲置掉线只留在告警中心。
        crate::notify::alert_created(&state.notify(), &alert_code).await;
    }

    Ok(affected)
}
/// 查询园区编码（用于 register_ack 下发与地图回传归属）
pub(crate) async fn park_code_of(db: &SqlitePool, park_id: Option<i64>) -> Option<String> {
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
/// （历史上漏选列导致注册回执失败）。
pub(crate) async fn fetch_device_by_code(
    db: &SqlitePool,
    device_id: &str,
) -> Result<Device, AppError> {
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
pub(crate) async fn handle_heartbeat(
    state: &crate::app::AppState,
    device_id: &str,
    payload: &HeartbeatPayload,
) {
    let db = &state.db;
    let config = &state.config;
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
    // 联网信号（板端 WiFi SIGNAL / 4G CSQ 折算，0-100）：页面「设备管理」的「信号」列
    //
    // 只认真实上报的值：没连 WiFi 也没拿到 4G IP / 老客户端不带该字段时保持库里原值，
    // 不要退回 0（那会显示成「信号极差」把排查方向带偏）。
    if let Some(signal) = payload.signal {
        let source = signal_source_of(payload.signal_source.as_deref());
        let _ = sqlx::query(
            "UPDATE devices SET signal = ?1, signal_source = ?2, updated_at = ?3
               WHERE device_id = ?4",
        )
        .bind(signal.clamp(0, 100))
        .bind(source)
        .bind(&timestamp_clone)
        .bind(&device_id_clone)
        .execute(db)
        .await;
    }
    // 控制源 / 运动状态：判断「是否已完全接管」的两个硬指标
    if let Some(source) = payload.ctrl_source {
        record_control_source_change(state, device_id, source, &timestamp).await;
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
        let _ =
            sqlx::query("UPDATE devices SET loc_status = ?1, updated_at = ?2 WHERE device_id = ?3")
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
pub(crate) const CAPABILITY_BUSY_WINDOW_SECS: i64 = 300;

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
pub(crate) async fn recent_capability_busy(db: &SqlitePool, device_id: &str) -> bool {
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
pub(crate) async fn record_control_source_change(
    state: &crate::app::AppState,
    device_id: &str,
    source: i64,
    timestamp: &str,
) {
    let db = &state.db;
    let previous: Option<i64> =
        sqlx::query_scalar("SELECT ctrl_source FROM devices WHERE device_id = ?1")
            .bind(device_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();

    let _ =
        sqlx::query("UPDATE devices SET ctrl_source = ?1, updated_at = ?2 WHERE device_id = ?3")
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
        "控制权被本体内部任务占用"
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
        // 我们自己下发的任务在跑：本体报 3 是"导航能力在本体手里"，属正常，
        // 不落抢占告警（否则每次下发任务都弹一次「控制权不在平台」）。
        if has_running_task(db, device_id).await {
            debug!("设备 {device_id} ctrl_source=3 出现在任务执行期间，按正常情况处理");
        } else {
            open_control_preempt_alert(state, device_id, timestamp).await;
        }
    } else {
        resolve_control_preempt_alert(db, device_id, timestamp).await;
    }
}

/// 控制权被其他控制源接管（`ctrl_source = 3`）→ 一条「未处理」告警。
///
/// 同一台设备已经有一条未处理的抢占告警时不重复插入：遥控器时断时续（`3 → 1 → 3`）
/// 在现场很常见，刷屏反而会盖住真正的故障。
pub(crate) async fn open_control_preempt_alert(
    state: &crate::app::AppState,
    device_id: &str,
    timestamp: &str,
) {
    let db = &state.db;
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
            occurred_at, created_at, dedup_key)
         VALUES (?1, '警告', ?2,
            (SELECT id FROM devices WHERE device_id = ?3),
            (SELECT park_id FROM devices WHERE device_id = ?3),
            '未处理', ?4, ?4, ?5)",
    )
    .bind(&alert_code)
    .bind(format!(
        "设备 {device_id} 上报 1315 ctrl_source=3（控制权不在平台）：{reason}。\
         平台摇杆与任务下发已暂停：请关闭遥控器电源 / 退出第三方程序，\
         并在本体上取消残留任务或重启本体算法服务；确认遥控器已断电后可「强制接管」"
    ))
    .bind(device_id)
    .bind(timestamp)
    // 同一台设备反复被抢占 = 同一个问题（源码里已按「未处理就跳过」限流）
    .bind(Some(format!("ctrl:{device_id}")))
    .execute(db)
    .await;
    // 落库后走统一出口；`alert_flow::should_dispatch` 只在「执行计划期间」才建单 + 推群。
    // 闲置时被抢占只留在告警中心（平台摇杆与任务同样会被门禁拦下，页面上看得见）。
    crate::notify::alert_created(&state.notify(), &alert_code).await;
}

/// 控制权回到 APP / SDK：自动把抢占告警结掉，避免告警中心长期挂着已恢复的条目。
pub(crate) async fn resolve_control_preempt_alert(
    db: &SqlitePool,
    device_id: &str,
    timestamp: &str,
) {
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
