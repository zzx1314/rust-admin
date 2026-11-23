//! 任务状态机：进度上报、待执行补发、停滞/超时看门狗。

// 同一模块拆出来的几块共用一个词汇表（`DeviceConnections` / `ConnectionCtx` / `send_msg` …），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

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
pub(crate) type StuckTask = (i64, String, String, i64, i64, String, String, String);

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

        // 设备离线导致的卡单按「严重」推：不是任务本身的问题，重启任务也没用
        let reason = if kind == "silent" {
            format!(
                "超过 {timeout}s 未收到进度上报（设备 {device_code} {device_status}），已判定为异常"
            )
        } else {
            format!(
                "进度停在 {progress}% 超过 {stuck}s 没有任何推进（设备 {device_code} {device_status}），已判定为异常"
            )
        };
        crate::notify::task_abnormal(
            &state.notify(),
            task_code,
            if device_status == "离线" {
                crate::notify::Level::Critical
            } else {
                crate::notify::Level::Warning
            },
            &reason,
        )
        .await;

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
/// 该设备是否有**我们自己下发**的、正在执行的任务。
///
/// 本体在跑导航任务时会把自己报成 `ctrl_source = 3`（控制权在本体自己手里）。
/// 这是正常现象，不是遥控器抢占——拿它去告警/弹窗只会让运维每次跑任务都被吓一次。
pub(crate) async fn has_running_task(db: &SqlitePool, device_id: &str) -> bool {
    let running: Option<i64> = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks t
           JOIN devices d ON d.id = t.device_id
          WHERE d.device_id = ?1 AND t.task_status = '执行中'",
    )
    .bind(device_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();
    running.unwrap_or(0) > 0
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
pub(crate) async fn handle_task_progress(
    state: &crate::app::AppState,
    device_id: &str,
    payload: &TaskProgressPayload,
) {
    let db = &state.db;
    let timestamp = now();
    if payload.task_code.is_empty() {
        return;
    }

    // 执行状态的合法集合只在 `status` 里配一份（以前这里、网关、ai_patrol 各一份，还抄歪了）
    let status = if crate::status::is_valid(&payload.status) {
        payload.status.as_str()
    } else {
        crate::status::RUNNING
    };

    // 航点到点 AI 识别：走到带「AI 识别」动作的航点时开识别窗口，终态时关窗口。
    // 与任务状态更新解耦——识别失败只打日志，不影响任务本身（见 ai_patrol 模块头）。
    state
        .ai_patrol
        .as_ref()
        .on_progress(state, device_id, payload)
        .await;

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

        // 客户端自己报「异常」：把本体的失败原因原样带进群。
        // 看门狗也会判异常并推送，两处靠 `task:{task_code}` 去重合并，
        // 先到的是客户端原因（更有信息量），看门狗那条会被静默窗口吃掉。
        if status == "异常" {
            crate::notify::task_abnormal(
                &state.notify(),
                &payload.task_code,
                crate::notify::Level::Warning,
                &format!("客户端上报任务异常：{message}"),
            )
            .await;
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
/// 下发待执行任务（设备注册 / 绑定园区 / 心跳确认本体就绪时调用）。
///
/// 三条规则：
/// - 设备未就绪（未绑园区 / 本体未连接 / 急停 / 控制权被抢 / 远程接管）时一条都不发：
///   注册时本体往往还没连上，早发只会得到一条「未连接机器人本体」，任务在平台上
///   看起来就是「已下发但不动」；
/// - 设备已有执行中 / 已暂停的任务时不补发：客户端只有一个执行上下文，后发的一条
///   会顶掉前一条，但平台侧两条都显示执行中；
/// - 一次只补发一条（优先级高、创建早的先跑），其余等这条上报终态后再补。
pub(crate) async fn dispatch_pending_tasks(
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

    // 按优先级取一批，再在内存里过「计划时间窗」：没到点的定时巡检不能补发，
    // 但排在它后面的临时任务照样要能跑，所以不能只取一条就放弃。
    let candidates: Vec<Task> = sqlx::query_as::<_, crate::models::TaskRow>(&format!(
        "{} WHERE d.device_id = ?1 AND t.task_status = '待执行'
            AND (t.park_id IS NULL OR t.park_id = ?2)
         ORDER BY t.level DESC, t.created_at ASC, t.id ASC LIMIT 20",
        crate::handlers::tasks::TASK_SELECT
    ))
    .bind(device_id)
    .bind(park_id)
    .fetch_all(db)
    .await?
    .into_iter()
    .map(Task::from)
    .collect();
    let clock = chrono::Local::now();
    let task = candidates.into_iter().find(|task| {
        matches!(
            crate::schedule::due_state(task, clock),
            crate::schedule::DueState::Due | crate::schedule::DueState::Immediate
        )
    });
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

    // 与 `push_task_dispatch` 一致：记下今天这个窗口跑过了，避免跑完回位后被当成
    // 「到点该跑」反复下发。
    sqlx::query("UPDATE tasks SET last_dispatched_on = ?1 WHERE id = ?2")
        .bind(chrono::Local::now().format("%Y-%m-%d").to_string())
        .bind(task.id)
        .execute(db)
        .await
        .ok();

    info!("dispatched task {} to device {}", task.task_code, device_id);
    Ok(())
}
