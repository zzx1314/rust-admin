//! 本体推送泵：5009/1315/4001 等帧的分发与状态帧处理。

use crate::ai_patrol::AiPatrol;
use crate::ai_patrol::ai_point_at;
use crate::ai_patrol::is_arrival_stage;
use crate::ai_patrol::is_point_stage;
use crate::lights::PointLights;
use crate::lights::light_point_at;
use crate::maps::send_map_saved_event;
use crate::maps::upload_mapping_to_zentai;
use crate::mapupload::UploadOutcome;
use crate::report::EventDedup;
use crate::report::ProgressReporter;
use crate::report::report_progress;
use crate::report::send_deduped_event;
use crate::state::ActiveTask;
use crate::state::RobotState;
use crate::state::Shared;
use crate::wire::DeviceEventPayload;
use crate::wire::TaskProgressPayload;
use crate::wire::WsMessage;
use crate::wire::send;
use crate::zhpro::client::ZhProClient;
use crate::zhpro::mapping::MappingSession;
use crate::zhpro::teleop;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use zhpro::status::StatusFrame;
use zhpro::status::TaskMonitor;
use zhpro::status::TaskSnapshot;
use zhpro::teleop::SharedTeleop;

use super::*;
use zhpro::protocol::*;

/// 本体推送处理：5009 状态、1315 本体状态、1401 位姿、4001 建图收尾、4021 故障
///
/// 参数都是运行期共享句柄（含 `teleop`：控制权丢失时要在同一处复位接管态），
/// 拆成结构体只会多一层搬运，故保留平铺签名。
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_zhpro_pump(
    client: Arc<ZhProClient>,
    out_tx: mpsc::Sender<WsMessage>,
    state: Shared<RobotState>,
    active: Shared<Option<ActiveTask>>,
    monitor: Arc<Mutex<TaskMonitor>>,
    local_mapping: Shared<Option<Arc<MappingSession>>>,
    teleop: SharedTeleop,
    config: Arc<Config>,
    ai: Arc<AiPatrol>,
    lights: Arc<PointLights>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = client.subscribe();
        let dedup = Arc::new(Mutex::new(EventDedup::default()));
        // 只在任务快照的 lifecycle/stage/error 变化时打一条，避免 5009 500ms 心跳刷屏
        let mut last_task_log: Option<String> = None;
        // 5009 驱动的任务进度（本体任务 / 返充出桩）：自己一套去重窗口，
        // 不能真按 500ms 原样上报，也不能傻到一帧不漏地刷日志
        let mut progress_reporter = ProgressReporter::default();
        while let Ok(env) = rx.recv().await {
            match env.head.msg_type {
                T_TASK_STATUS => {
                    let Some(frame) = StatusFrame::parse(&env.data) else {
                        continue;
                    };
                    {
                        let mut guard = state.lock().await;
                        guard.loc_status = frame.loc_status;
                        guard.loc_fresh = frame.loc_fresh;
                    }
                    // 同一 boot_id 内按 snapshot_seq 丢弃重复帧，revision 变化才意味着任务内容变化
                    let Some(accepted) = monitor.lock().await.accept(&frame) else {
                        continue;
                    };
                    if accepted.boot_changed {
                        // 算法节点重启（boot_id 变了）：上一轮的 5009 快照全部作废，
                        // 本地任务视图也要清空，否则界面/台账会拿着不存在的任务继续跑。
                        // 业务任务上下文（active / runner）不在这里清：终态由 runner 自己
                        // 的上报或看门狗收尾，清了反而永远报不上去。
                        warn!(
                            "5009 boot_id 变化（算法节点重启，boot={}），清空本地任务快照",
                            accepted.frame.boot_id
                        );
                        state.lock().await.current_task = None;
                        last_task_log = None;
                    }
                    if accepted.frame.reason != "heartbeat" {
                        debug!(
                            "5009 帧: reason={} revision={} task_state={} loc={}",
                            accepted.frame.reason,
                            accepted.frame.revision,
                            accepted.frame.robot_task_state,
                            accepted.frame.loc_status
                        );
                    }
                    // 把 current/pending/最近终态的关键字段留痕：5001 code=0 但任务
                    // 不执行时，只有 stage/error/stop_reason 能说明卡在哪一步。
                    let snapshot = accepted
                        .frame
                        .current
                        .as_ref()
                        .map(|task| (task, "current"))
                        .or_else(|| {
                            accepted
                                .frame
                                .pending
                                .as_ref()
                                .map(|task| (task, "pending"))
                        })
                        .or_else(|| {
                            accepted
                                .frame
                                .terminals
                                .first()
                                .map(|task| (task, "terminal"))
                        });
                    if let Some((task, source)) = snapshot {
                        let key = format!(
                            "{}|{}|{}|{}|{:?}|{:?}",
                            task.task_id,
                            task.lifecycle,
                            task.stage,
                            task.task_type,
                            task.error_message,
                            task.stop_reason
                        );
                        if last_task_log.as_deref() != Some(key.as_str()) {
                            // 本体的 5009 响应：只在 lifecycle/stage/error 变化时打一条，
                            // 所以放在 info —— 默认 `journalctl` 就能看到「本体回什么」。
                            // （原始帧本身是 500ms 一条，只在 debug/trace 或帧落盘里出现）
                            info!(
                                "5009 任务({source}): id={} name={} type={} lifecycle={} stage={}                                  progress={:?} point={:?}/{:?} error={:?} stop={:?}                                  task_state={} loc={}",
                                task.task_id,
                                task.name,
                                task.task_type,
                                task.lifecycle,
                                task.stage,
                                task.progress,
                                task.detail_index,
                                task.detail_total,
                                task.error_message,
                                task.stop_reason,
                                accepted.frame.robot_task_state,
                                accepted.frame.loc_status
                            );
                            last_task_log = Some(key);
                        }
                    } else {
                        last_task_log = None;
                    }
                    let frame = accepted.frame.clone();
                    handle_status_frame(
                        &frame,
                        &out_tx,
                        &state,
                        &active,
                        &monitor,
                        &mut progress_reporter,
                        &ai,
                        &lights,
                    )
                    .await;
                }
                T_ROBOT_STATUS => {
                    // 1315 里的运动状态 / 机器模式 / 控制源变化要留痕：
                    // 现场争议最多的两个问题是「它什么时候自己趴下的」和
                    // 「谁把控制权拿走了」，以前只能靠 journalctl 猜。
                    let mut notices: Vec<(String, Value)> = Vec::new();
                    let mut invalidate_control = false;
                    {
                        let mut guard = state.lock().await;
                        let before_motion = guard.motion_status.clone();
                        let before_mode = guard.robot_mode.clone();
                        let before_ctrl = guard.ctrl_source;
                        guard.apply_robot_status(&env.data);
                        if before_motion != guard.motion_status {
                            let from = before_motion.unwrap_or_else(|| "未知".to_string());
                            let to = guard.motion_status.clone().unwrap_or_default();
                            notices.push((
                                format!("本体运动状态变更: {from} → {to}"),
                                json!({ "from": from, "to": to }),
                            ));
                        }
                        if before_mode != guard.robot_mode {
                            let from = before_mode.unwrap_or_else(|| "未知".to_string());
                            let to = guard.robot_mode.clone().unwrap_or_default();
                            notices.push((
                                format!("本体机器模式变更: {from} → {to}"),
                                json!({ "from": from, "to": to }),
                            ));
                        }
                        if before_ctrl != guard.ctrl_source
                            && let Some(source) = guard.ctrl_source
                        {
                            // 控制权被外部遥控器/未知来源拿走时，本地的 1204
                            // 标记必须失效；否则下一次“远程接管”会误以为还持有
                            // RoamerX，跳过 1204，摇杆帧被网关静默丢弃。
                            invalidate_control = matches!(source, 0 | 3);
                            notices.push((
                                format!(
                                    "本体控制源变更: ctrl_source={source}\
                                     （0 未知 / 1 APP / 2 SDK / 3 遥控器）"
                                ),
                                json!({ "ctrlSource": source }),
                            ));
                        }
                    }
                    if invalidate_control {
                        client.invalidate_control().await;
                        // 1204 的本地标记失效时，`teleop` 的接管态与摇杆缓存必须一起清：
                        // 只清一半会让摇杆首帧跳过 1204，1304 在没有 owner 的情况下被网关
                        // 丢掉，现场表现就是「平台显示已接管、狗子不动且没有任何报错」。
                        teleop::reset_after_control_lost(&client, &teleop).await;
                    }
                    for (message, data) in notices {
                        let event = DeviceEventPayload {
                            event_type: "info".to_string(),
                            message,
                            data: Some(data),
                        };
                        send(
                            &out_tx,
                            "event",
                            serde_json::to_value(event).unwrap_or_default(),
                        )
                        .await;
                    }
                }
                T_MODULE_NOTIFY => {
                    // 1207 modules[]：官方 App 用 sdk_node 是否可用决定是否让
                    // 用户进入控制/任务；我们以前只打 DEBUG，导致 sdk_node=false
                    // 时仍然照发 1302/1304/5000。
                    if let Some(modules) = env.data.get("modules").and_then(Value::as_array) {
                        let mut available = None;
                        for module in modules {
                            if module.get("name").and_then(Value::as_str) == Some("sdk_node") {
                                available = module.get("available").and_then(Value::as_bool);
                            }
                        }
                        if let Some(available) = available {
                            let mut guard = state.lock().await;
                            if guard.sdk_node_available != Some(available) {
                                if available {
                                    info!("本体 SDK 节点已就绪（sdk_node=true）");
                                } else {
                                    warn!(
                                        "本体 SDK 节点未就绪（sdk_node=false）：运动/导航指令会被拒"
                                    );
                                }
                                guard.sdk_node_available = Some(available);
                            }
                        }
                    }
                }
                T_CURRENT_POSE => {
                    state.lock().await.apply_pose(&env.data);
                }
                T_MAP_SAVED => {
                    let ok = env.is_ok();
                    info!(
                        "收到 4001 地图保存结果: {} {}",
                        if ok { "成功" } else { "失败" },
                        env.message()
                    );

                    // 交给建图会话解析（会话只认第一次，避免重复收尾）
                    let session = local_mapping.lock().await.clone();
                    let Some(session) = session else {
                        // 不在建图流程中（例如运维手动建图），只记录事件
                        send_map_saved_event(&out_tx, ok, env.message(), None).await;
                        continue;
                    };
                    let Some(saved) = session.on_map_saved(&env.data).await else {
                        continue;
                    };

                    // 建图任务收尾：给业务侧一个明确的终态，而不是只发一条 event
                    let task_code = {
                        let guard = active.lock().await;
                        guard
                            .as_ref()
                            .map(|a| a.task_code.clone())
                            .unwrap_or_default()
                    };
                    let map_hint = saved.dir_name().or_else(|| saved.map_path.clone());

                    if saved.success {
                        // 成功：先把「已保存到本体」报上去，再尝试回传中台
                        report_progress(
                            &out_tx,
                            &task_code,
                            "执行中",
                            90,
                            Some(match &map_hint {
                                Some(name) => format!("地图已保存到本体: {name}"),
                                None => "地图已保存到本体".to_string(),
                            }),
                            Some("mapping.saved".to_string()),
                            Some("mapping".to_string()),
                            None,
                        )
                        .await;

                        // 地图回传中台（A 方案）
                        let outcome = upload_mapping_to_zentai(&config, &saved).await;
                        let (status, progress, message) = match outcome {
                            UploadOutcome::Skipped(reason) => {
                                ("已完成", 100, format!("建图完成（未回传中台：{reason}）"))
                            }
                            UploadOutcome::Uploaded { map_id } => {
                                ("已完成", 100, format!("建图完成，地图已入库中台: {map_id}"))
                            }
                            UploadOutcome::Failed { reason } => {
                                ("异常", 90, format!("建图完成，但回传中台失败: {reason}"))
                            }
                        };
                        report_progress(
                            &out_tx,
                            &task_code,
                            status,
                            progress,
                            Some(message.clone()),
                            Some("mapping.done".to_string()),
                            Some("mapping".to_string()),
                            None,
                        )
                        .await;
                        send_map_saved_event(&out_tx, true, &saved.message, map_hint.clone()).await;
                        info!("建图任务 {task_code} 收尾: {message}");
                        *local_mapping.lock().await = None;
                        *active.lock().await = None;
                        state.lock().await.current_task = None;
                    } else {
                        let message = format!("建图保存失败: {}", saved.message);
                        report_progress(
                            &out_tx,
                            &task_code,
                            "异常",
                            0,
                            Some(message.clone()),
                            Some("mapping.failed".to_string()),
                            Some("mapping".to_string()),
                            None,
                        )
                        .await;
                        send_map_saved_event(&out_tx, false, &saved.message, None).await;
                        *local_mapping.lock().await = None;
                        *active.lock().await = None;
                        state.lock().await.current_task = None;
                    }
                }
                T_FAULT_CODE => {
                    if let Some(items) = env.data.get("items").and_then(Value::as_array) {
                        for item in items {
                            let desc = item
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("未知故障");
                            let severity =
                                item.get("severity").and_then(Value::as_i64).unwrap_or(0);
                            // 码值必须落到事件文案里：平台 task_events 只存 description，
                            // 没有 code/severity 时没法对着本体文档 / 厂家排查。
                            let message = match item.get("code").and_then(Value::as_i64) {
                                Some(code) => format!("{desc}（code={code}, 级别={severity}）"),
                                None => desc.to_string(),
                            };
                            let event = DeviceEventPayload {
                                event_type: if severity >= 1 { "fault" } else { "info" }
                                    .to_string(),
                                message,
                                data: Some(item.clone()),
                            };
                            send_deduped_event(&out_tx, &dedup, event).await;
                        }
                    }
                }
                T_FAULT_STATUS => {
                    // 1317 只上报最严重的一条（level>=2 才算故障）
                    let worst =
                        env.data
                            .get("faults")
                            .and_then(Value::as_array)
                            .and_then(|faults| {
                                faults.iter().max_by_key(|f| {
                                    f.get("level").and_then(Value::as_i64).unwrap_or(0)
                                })
                            });
                    if let Some(worst) = worst
                        && worst.get("level").and_then(Value::as_i64).unwrap_or(0) >= 2
                    {
                        let event = DeviceEventPayload {
                            event_type: "fault".to_string(),
                            message: format!(
                                "{} / {}",
                                worst
                                    .get("module")
                                    .and_then(Value::as_str)
                                    .unwrap_or("body"),
                                worst
                                    .get("fault")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown")
                            ),
                            data: Some(worst.clone()),
                        };
                        send_deduped_event(&out_tx, &dedup, event).await;
                    }
                }
                T_CHARGE_RESULT => {
                    let status = env.data.get("status").and_then(Value::as_i64).unwrap_or(0);
                    let label = match status {
                        1 => "回充成功，已进入充电",
                        3 => "满电保持",
                        _ => "回充失败",
                    };
                    info!("5019 底层充电结果: {label}");
                    let event = DeviceEventPayload {
                        event_type: if status == 2 { "error" } else { "info" }.to_string(),
                        message: format!("{label}: {}", env.message()),
                        data: Some(env.data.clone()),
                    };
                    send_deduped_event(&out_tx, &dedup, event).await;
                }
                _ => {}
            }
        }
    })
}

/// 把 5009 快照翻译成业务侧进度
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_status_frame(
    frame: &StatusFrame,
    out_tx: &mpsc::Sender<WsMessage>,
    state: &Shared<RobotState>,
    active: &Shared<Option<ActiveTask>>,
    monitor: &Arc<Mutex<TaskMonitor>>,
    reporter: &mut ProgressReporter,
    ai: &Arc<AiPatrol>,
    lights: &Arc<PointLights>,
) {
    // 文档 §2.6.10 第 5 条：即时终态可能已经在 `current_task` 里被回收，只留在
    // `recent_terminal_tasks` 里（最多 3 条 / 约 30s）。两处都要识别，否则
    // 「已建链但终态帧丢了/断开重连期间到终态」时业务侧永远收不到终态，
    // 只能等看门狗把一条成功的任务判成「异常」。
    let bound_id = {
        let guard = active.lock().await;
        guard.as_ref().and_then(|a| a.nav_task_id.clone())
    };
    let task = match frame.current.as_ref() {
        Some(task) => task.clone(),
        None => {
            // 没有 current_task 时只认「本端已绑定的那个 nav_task_id」的终态，
            // 别人的历史终态一律忽略。
            let Some(id) = bound_id.as_deref() else {
                return;
            };
            let Some(task) = frame.terminals.iter().find(|t| t.task_id == id) else {
                return;
            };
            task.clone()
        }
    };

    // 只跟随我们下发的那个任务
    let bound = {
        let guard = active.lock().await;
        match guard.as_ref() {
            Some(a) => match &a.nav_task_id {
                Some(id) => *id == task.task_id,
                // 尚未绑定（刚下发）时用类型相近判定
                None => a.task_code == task.name || task.task_type == "patrol",
            },
            None => false,
        }
    };
    if !bound {
        return;
    }

    // 终态上报幂等：同一条任务同一个 lifecycle 只报一次。
    // 后面每一帧心跳都会带同一个终态（current_task 一次 + recent_terminal_tasks 多次），
    // 不去重就是每 500ms 往业务库推一条「已完成」。
    if task.is_terminal() && !monitor.lock().await.take_terminal(&task) {
        return;
    }

    {
        let mut guard = active.lock().await;
        if let Some(a) = guard.as_mut() {
            a.nav_task_id = Some(task.task_id.clone());
            a.stage = Some(task.stage.clone());
            a.point_index = task.detail_index;
        }
    }

    // 阶段 → 业务状态
    let (status, message) = classify_stage(&task);
    let index = task.detail_index;
    let progress = task
        .progress
        .map(|p| p.round() as i64)
        .or_else(|| index.map(|i| (i + 1) * 10))
        .unwrap_or(0);

    let payload = TaskProgressPayload {
        task_code: {
            let guard = active.lock().await;
            guard
                .as_ref()
                .map(|a| a.task_code.clone())
                .unwrap_or_default()
        },
        status: status.to_string(),
        progress,
        point_index: index,
        waypoint_index: index,
        message: Some(message),
        stage: Some(task.stage.clone()),
        robot_task_id: Some(task.task_id.clone()),
        total_points: None,
    };
    // 每次真正发出去的 payload 都打到日志：现场排查「平台显示的状态与本体不一致」时，
    // 先看这里上报了什么，再去对上面那条 5009 快照行，比猜更直接
    reporter.report(out_tx, &payload, "5009").await;

    // 航点 AI 识别：走到带「AI 识别」动作的航点就本地开识别窗口，终态时关窗。
    // 与服务端版本同一条规则（`patrol.point_arrived` / `patrol.waiting` + 该点有 AI 动作），
    // 只是把「谁去调 Agent」从公网平台换成了同机的客户端。
    // 识别失败只打日志，绝不影响巡检本身。
    if task.is_terminal() {
        ai.stop("任务终态").await;
        // 灯光：跑完 / 失败 / 取消都要收——灯亮着没人管就是白耗电
        lights.leave("任务终态").await;
        // 任务跑完了（成功 / 失败 / 取消都一样）：整个计划的板端存证都没用了。
        // 只靠「推过图就删这个航点」（`ai_cleanup`）清不干净——数量任务的检出不建单
        // 不推群、一个都没命中的航点只留一份 result.json，它们等不到 `ai_cleanup`。
        let plan_id = active
            .lock()
            .await
            .as_ref()
            .map(|task| task.task_code.clone());
        if let Some(plan_id) = plan_id {
            ai.finish_plan(&plan_id, "任务终态").await;
        }
    } else if is_arrival_stage(&task.stage) {
        let current = active.lock().await.clone();
        if let (Some(current), Some(index)) = (current, index) {
            // 灯光先开：AI 要亮着灯看，顺序反了前几帧还是黑的（补光灯从写 GPIO
            // 到真正亮起来只要几十毫秒，但识别窗口开得比这快）。开灯放到后台——
            // `POST /api/lights` 里有几次 GPIO shell 调用，不能让 5009 处理链路等它
            match light_point_at(&current.light_points, index) {
                Some(point) => {
                    let lights = lights.clone();
                    let task_code = current.task_code.clone();
                    let point = point.clone();
                    let night_only = current.light_night_only;
                    tokio::spawn(async move {
                        lights.point_arrived(&task_code, &point, night_only).await;
                    });
                }
                // 这个点没配灯：上一个点的灯别继续亮着（狗已经走了）
                None => lights.leave("到达无灯光动作的航点").await,
            }
            match ai_point_at(&current.ai_points, index) {
                Some(point) => {
                    ai.point_arrived(out_tx, &current.task_code.clone(), point)
                        .await
                }
                // 这个点没有 AI 动作：上一个点的窗口别继续跑（画面已经变了）
                None => ai.stop("到达无 AI 动作的航点").await,
            }
        }
    } else if !is_point_stage(&task.stage) {
        // 狗已经离开航点（前往下一点 / 返航 / 别的任务阶段）：窗口时长跟着停留时间走，
        // 收口交给这里，别让相机把路过的东西当成这个点的目标
        ai.stop("已离开航点").await;
        // 灯同一个道理：录像/识别已经结束，补光灯没必要一路亮到下一个点
        lights.leave("已离开航点").await;
    }

    // 离散事件（去重后只报一次）
    if monitor.lock().await.take_event(&task) {
        let event = DeviceEventPayload {
            event_type: "progress".to_string(),
            message: format!("{}: {}", task.last_event_type, task.stage),
            data: Some(json!({
                "taskId": task.task_id,
                "stage": task.stage,
                "index": task.detail_index,
                "total": task.detail_total,
            })),
        };
        send(
            out_tx,
            "event",
            serde_json::to_value(event).unwrap_or_default(),
        )
        .await;
    }

    // 终态：清空占用
    if task.is_terminal() {
        if let Some(lo) = task.detail_index {
            let _ = lo;
        }
        let mut guard = active.lock().await;
        if let Some(a) = guard.as_ref() {
            info!(
                "任务 {} [{}] 进入终态: {}",
                a.task_code, a.task_name, task.lifecycle
            );
        }
        *guard = None;
        let mut st = state.lock().await;
        st.current_task = None;
    }
}

/// 5009 `stage` → 业务任务状态
pub(crate) fn classify_stage(task: &TaskSnapshot) -> (&'static str, String) {
    if task.is_terminal() {
        return match task.lifecycle.as_str() {
            "success" => ("已完成", "任务完成".to_string()),
            "cancelled" => (
                "已暂停",
                task.stop_reason
                    .clone()
                    .unwrap_or_else(|| "任务被取消".to_string()),
            ),
            _ => (
                "异常",
                task.error_message
                    .clone()
                    .or_else(|| task.stop_reason.clone())
                    .unwrap_or_else(|| "任务失败".to_string()),
            ),
        };
    }
    if task.lifecycle == "paused" {
        return ("已暂停", "任务已暂停".to_string());
    }
    let message = match task.stage.as_str() {
        "mapping.preparing" => "建图准备中".to_string(),
        "mapping.running" => "建图中".to_string(),
        "mapping.saving" => "地图保存中".to_string(),
        "navigation.localizing" => "定位中".to_string(),
        "navigation.navigating" | "patrol.navigating" => "前往航点".to_string(),
        "patrol.point_arrived" => "已到达航点".to_string(),
        "patrol.waiting" => "航点停留".to_string(),
        "patrol.photographing" => "拍照采集".to_string(),
        "patrol.point_completed" => "航点作业完成".to_string(),
        "patrol.returning" => "返航中".to_string(),
        "patrol.returning_to_charge" | "return_to_charge.navigating" => "返回充电点".to_string(),
        "patrol.recharging" | "return_to_charge.recharging" | "recharge.charging" => {
            "充电中".to_string()
        }
        other => other.to_string(),
    };
    ("执行中", message)
}
