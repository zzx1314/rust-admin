//! 本体推送泵：5009/1315/4001 等帧的分发与状态帧处理。

use crate::ai_patrol::AiPatrol;
use crate::ai_patrol::ai_point_at;
use crate::ai_patrol::is_arrival_stage;
use crate::ai_patrol::is_point_stage;
use crate::lights::PointLights;
use crate::maps::send_map_saved_event;
use crate::maps::upload_mapping_to_zentai;
use crate::mapupload::UploadOutcome;
use crate::photo::Photographer;
use crate::record::Recorder;
use crate::report::EventDedup;
use crate::report::ProgressReporter;
use crate::report::report_progress;
use crate::report::send_deduped_event;
use crate::speak::Speaker;
use crate::state::ActiveTask;
use crate::state::RobotState;
use crate::state::Shared;
use crate::state::clear_active_task_if;
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
    recorder: Arc<Recorder>,
    speaker: Arc<Speaker>,
    photographer: Arc<Photographer>,
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
                    let cleanup = handle_status_frame(
                        &frame,
                        &out_tx,
                        &state,
                        &active,
                        &monitor,
                        &mut progress_reporter,
                        &ai,
                        &lights,
                        &recorder,
                        &speaker,
                        &photographer,
                    )
                    .await;
                    // 终态后本体没释放导航能力：后台补 5002 + 等 idle。
                    // 必须 spawn —— 它最长 15s，不能堵住 5009/1315 的处理链路。
                    if let Some(cleanup) = cleanup {
                        let client = client.clone();
                        tokio::spawn(async move {
                            crate::zhpro::route::release_nav_capability(
                                &client,
                                &cleanup.task_id,
                                &cleanup.task_state,
                            )
                            .await;
                        });
                    }
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
                        // 建图收尾可能晚于「新任务下发」：只清自己那条（见
                        // `clear_active_task_if`），否则会把新任务的上下文清掉
                        clear_active_task_if(&active, &state, &task_code).await;
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
                        clear_active_task_if(&active, &state, &task_code).await;
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
                            // 码值同时记进客户端本地故障日志：航线执行引擎要靠它判断
                            // 「卡在最后一个点」是跑得慢还是规划器根本进不去（route.rs）。
                            if let Some(code) = item.get("code").and_then(Value::as_i64) {
                                client.note_fault(code).await;
                            }
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
/// 终态任务的收尾请求：本体还占着导航能力时，由 pump 在后台补 `5002` 并等 idle。
///
/// 不能在这里同步做：`5002 + 等 idle` 最长 15s，卡在 5009 处理链路上会让 1315
/// 心跳一起停摆（平台会把设备判离线）。所以只返回「要清谁」，由 [`spawn_zhpro_pump`]
/// 抛到后台任务里执行。
pub(crate) struct TerminalCleanup {
    pub(crate) task_id: String,
    pub(crate) task_state: String,
}

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
    recorder: &Arc<Recorder>,
    speaker: &Arc<Speaker>,
    photographer: &Arc<Photographer>,
) -> Option<TerminalCleanup> {
    // 文档 §2.6.10 第 5 条：即时终态可能已经在 `current_task` 里被回收，只留在
    // `recent_terminal_tasks` 里（最多 3 条 / 约 30s）。两处都要识别，否则
    // 「已建链但终态帧丢了/断开重连期间到终态」时业务侧永远收不到终态，
    // 只能等看门狗把一条成功的任务判成「异常」。
    //
    // **只跟随已经用 5001 的 `task_id` 绑定过的那条任务**。以前 `nav_task_id` 为空时
    // 用 `task.task_type == "patrol"` 兜底，于是「刚下发、5000 还没回」的那几帧会把
    // `recent_terminal_tasks` 里上一轮的终态当成这条新任务的终态上报：
    // 平台立刻显示「已完成」，而狗还没起步；之后 runner 再报「执行中」也会被服务端
    // 的「终态不可回退」挡掉（`handle_task_progress`），整条任务在平台上就废了。
    // 5009 一定在 5001 之后才可能带这条任务，所以不需要任何兜底匹配。
    let bound_id = {
        let guard = active.lock().await;
        guard.as_ref().and_then(|a| a.nav_task_id.clone())
    };
    let bound_id = bound_id?;
    // `task_for` 依次查 current_task / pending_task / recent_terminal_tasks。
    // **pending_task 必须认**：文档 §2.6.10 里它是「已受理但等待旧任务停稳 / 资源释放」，
    // 这时底盘还没被驱动（尾巴灯还是非任务色），只查 current_task 会让平台完全
    // 看不到「任务卡在排队」，一直以为执行中。
    let task = frame.task_for(&bound_id).cloned()?;
    if task.task_id != bound_id {
        return None;
    }
    let pending = frame
        .current
        .as_ref()
        .is_none_or(|current| current.task_id != task.task_id)
        && frame
            .pending
            .as_ref()
            .is_some_and(|pending| pending.task_id == task.task_id);
    if pending {
        info!(
            "5009 任务在 pending_task（已受理、等旧任务停稳/资源释放，底盘未驱动）: id={} \
             name={} stage={} task_state={}",
            task.task_id, task.name, task.stage, frame.robot_task_state
        );
    }

    // 终态上报幂等：同一条任务同一个 lifecycle 只报一次。
    // 后面每一帧心跳都会带同一个终态（current_task 一次 + recent_terminal_tasks 多次），
    // 不去重就是每 500ms 往业务库推一条「已完成」。
    if task.is_terminal() && !monitor.lock().await.take_terminal(&task) {
        return None;
    }

    // 断点续跑：本体路径里只有断点之后的航点，`detail_index` 是**本轮**下标；
    // 本地动作表（AI / 灯光 / 录像 / 播报 / 拍照）与平台侧的 `routeData.points` 都是
    // **原始**下标，这里加回偏移。不加的话续跑那一轮会把 P4 当成 P1——
    // 该拍照的点没拍、该识别的点跑了别的动作表。
    let start_index = active
        .lock()
        .await
        .as_ref()
        .map(|task| task.start_index)
        .unwrap_or(0);
    let index = task.detail_index.map(|index| index + start_index);
    {
        let mut guard = active.lock().await;
        if let Some(a) = guard.as_mut() {
            a.nav_task_id = Some(task.task_id.clone());
            a.stage = Some(task.stage.clone());
            a.point_index = index;
        }
    }

    // 阶段 → 业务状态
    let (status, message) = classify_stage(&task);
    // 断点续跑时本体的百分比只覆盖**本轮**（切片后的）航点，直接上报会让进度条
    // 在「已完成 3/10 = 30%」和「本轮第 1/7 = 14%」之间来回跳。按偏移换算成
    // 整条路线的百分比，与 5s 定时摘要（`progress_pct(done, total)`）同一口径。
    let progress = match (index, task.detail_total) {
        (Some(absolute), Some(round_total)) if start_index > 0 && round_total > 0 => {
            let total = round_total + start_index;
            ((absolute + 1) * 100 / total).clamp(0, 100)
        }
        _ => task
            .progress
            .map(|p| p.round() as i64)
            .or_else(|| index.map(|i| (i + 1) * 10))
            .unwrap_or(0),
    };

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
            // 灯光：跑完 / 失败 / 取消都要收（灯亮着没人管就是白耗电）；
            // `finish_task` 同时把这条任务标成已收尾，迟到的「到点开灯」不许再把灯点亮。
            lights.finish_task(&plan_id, "任务终态").await;
            // 录像：停掉航点 / 全程两路 ffmpeg，并把板端积压的 mp4 回传平台
            // （回传在后台跑，弱网下可能要几分钟；失败的文件留在板端等下一轮）。
            recorder.finish_task(&plan_id, "任务终态").await;
            // 语音播报：循环播报不能跟着狗回家，终态当场收口（板端看门狗只是兜底）。
            speaker.finish_task(&plan_id, "任务终态").await;
            // 航点照片：停拍 + 把板端积压的图回传平台（回传在后台跑，弱网下要几分钟）。
            photographer.finish_task(&plan_id, "任务终态").await;
        } else {
            lights.leave("任务终态").await;
            speaker.finish_task("", "任务终态").await;
            photographer.finish_task("", "任务终态").await;
        }
    } else {
        // 计划级自动开灯（`lightFill` / `lightLed`）：**全程开**，不跟航点动作。
        // 一旦 5009 里出现这条任务就开（不等第一个航点），离开航点也不关；
        // 5009 每 500ms 一帧，`PointLights` 按 `(taskCode, PLAN_LIGHT_INDEX)` 去重。
        if let Some(plan) = active.lock().await.as_ref().and_then(|task| {
            task.plan_lights.clone().map(|point| {
                (
                    task.task_code.clone(),
                    point,
                    task.light_night_only,
                    task.light_window.clone(),
                )
            })
        }) {
            let (task_code, point, night_only, window) = plan;
            let lights = lights.clone();
            tokio::spawn(async move {
                lights
                    .point_arrived(&task_code, &point, night_only, window.as_ref())
                    .await;
            });
        }
        if is_arrival_stage(&task.stage) {
            let current = active.lock().await.clone();
            if let (Some(current), Some(index)) = (current, index) {
                // 语音播报**必须排在到点分支的最前面**（2026-11-25 现场：狗在 P3 停住
                // 5 秒后还在喊）。它只做两件事：把上一段的航段播报收掉（`/api/speak/stop`）、
                // 起本点的航点播报（`/api/speak`），都是本机一次 HTTP POST，毫秒级；
                // 而排在它前面的「可见光拍照」是**同步 ffmpeg 抓图，现场 2.9s**（日志
                // `[photo] 航点 2/dogcam 抓拍 1 张（2.9s）`）—— 顺序反了，停播就晚 3 秒，
                // 加上板端/云端 TTS 把当前句子播完的尾巴，现场听到的就是「停了还在喊」。
                // **串行 await**（不像开灯那样 spawn）：与下面「离开航点停播 / 起步开播」
                // 必须严格有序，否则 stop 可能抢在 start 前面把新播报掐掉。
                // 航点动作的播报现在就播；**从这里出发的航段播报只记账**
                // （`Speaker::point_arrived` 里按 scope 分流），等狗真的起步再播——
                // 到点就播会让停留（AI 识别 / 拍照）先吃掉循环时长。
                speaker.point_arrived(&current.task_code, index).await;
                // 灯光：航点动作 / 计划灯现在开（AI 要亮着灯看，顺序反了前几帧还是黑的）。
                // 开灯放到后台——`POST /api/lights` 里有几次 GPIO shell 调用，不能让
                // 5009 处理链路等它。**航段灯例外**：`point_arrived` 收到 scope=leg 时
                // 只记账不开灯，等狗真的起步（下面「离开航点」）才点亮——到点后这个
                // 航点还要跑 AI 识别 / 拍照 / 停留，亮起来的补光灯会把画面和存证图污染。
                match current.light_at(index) {
                    Some(point) => {
                        let lights = lights.clone();
                        let task_code = current.task_code.clone();
                        let night_only = current.light_night_only;
                        let window = current.light_window.clone();
                        tokio::spawn(async move {
                            lights
                                .point_arrived(&task_code, &point, night_only, window.as_ref())
                                .await;
                        });
                    }
                    // 这个点没配灯：上一个点的灯别继续亮着（狗已经走了）
                    None => lights.leave("到达无灯光动作的航点").await,
                }
                // 航点录像：这个点配了「录像采集」就开录（`Recorder` 自己查路线配置，
                // 同一个点重复调用只起一次），离开航点时由下面那段收口。
                recorder.point_arrived(&current.task_code, index).await;
                // AI 识别必须**最早**开窗：狗在这个点只停「停留时长」那么久，
                // 抓拍（0.5~2s）和播报（asr 拉起可能 2s）排在前面会把可用帧吃掉。
                //
                // 但「到点」不等于「能开始算」：`patrol.point_arrived` 只是位置到了，
                // 本体还要原地转到路线里配的 `heading` 才进 `patrol.waiting`
                // （现场实测恒定 2~3s）。所以到点只让板端**预热**（模型 + 抓帧 +
                // 画框与转身重叠），`mark_settled`（下面的 `waiting` 帧）之后才开始
                // 计数/上报 —— 转身帧既无信息量又易误检（见 `AiPatrol::run_window`）。
                match ai_point_at(&current.ai_points, index) {
                    Some(point) => {
                        ai.point_arrived(out_tx, &current.task_code.clone(), point)
                            .await
                    }
                    // 这个点没有 AI 动作：上一个点的窗口别继续跑（画面已经变了）
                    None => ai.stop("到达无 AI 动作的航点").await,
                }
                if task.stage == "patrol.waiting" {
                    ai.mark_settled(&current.task_code, index).await;
                }
                // 航点拍照：这个点配了「可见光拍照」就抓一张（同步等：狗可能马上就走）
                photographer.point_arrived(&current.task_code, index).await;
            }
        } else if is_point_stage(&task.stage) {
            // 已经越过到点阶段（`photographing` / `point_completed`）但没等到
            // `patrol.waiting`：老本体 / `dwell=0` 的航点会这样，说明到点后的
            // 转身、对位已经做完（或者根本不做）—— AI 可以开始计数了。
            let current = active.lock().await.clone();
            if let Some(current) = current
                && let Some(index) = current.point_index
            {
                ai.mark_settled(&current.task_code, index).await;
            }
        } else if !is_point_stage(&task.stage) {
            // 狗已经离开航点（前往下一点 / 返航 / 别的任务阶段）：窗口时长跟着停留时间走，
            // 收口交给这里，别让相机把路过的东西当成这个点的目标
            ai.stop("已离开航点").await;
            // 语音播报同样排在最前面（与到点分支同一个理由）：停掉本点的航点播报、
            // 起步的航段播报开始。它只是本机一两次 HTTP POST，而下面的
            // `recorder.leave_point`（板端 `/api/record/stop` 要等 ffmpeg 写完 moov，
            // 最长 8s）和 `leg_started` 都可能要几百毫秒到几秒 —— 排在后面就是
            // 「狗起步走了，点上那几句还在喊」。
            speaker.leave_point("已离开航点").await;
            // 灯同一个道理：录像/识别已经结束，补光灯没必要一路亮到下一个点。
            // **计划灯除外**：它就是要全程亮着，这里关掉等于「离开第一个点就灭」；
            // **航段灯同理但要反过来看**：到点只记了账、没点亮，离开航点正是它开灯的
            // 时刻（`leave_point` 先把待开的航段灯点着，再按 scope 收航点灯）。
            let plan_lighting = active
                .lock()
                .await
                .as_ref()
                .is_some_and(|task| task.plan_lighting());
            if !plan_lighting {
                // 起步这一帧把两件事一起做（顺序不能反）：
                // 1) `leave_point`：先把到点时记下的航段灯点亮（狗真的起步了），
                //    再按 scope 收航点灯；
                // 2) `keep_alive`：这一段路上没有新的「到点」帧，板端看门狗（默认 900s）
                //    只认最近一次请求 → 按 `LIGHT_REFRESH_AFTER` 原地重发一次
                //    （航点灯已经关了 / 待开的航段灯还没点亮时什么都不发）。
                // **两个都放后台**：`POST /api/lights` 里有 GPIO 写 + `systemctl
                // start/stop`（LED 灯带第一次启动还要 pkill 老进程 + 两次 is-active，
                // 关灯要等 SIGINT 收尾），Agent 不可达时更要等满 5s，不能让 5009
                // 处理链路跟着卡；先后由 `PointLights` 的 gate（FIFO）保证。
                let lights = lights.clone();
                tokio::spawn(async move {
                    lights.leave_point("已离开航点").await;
                    lights.keep_alive().await;
                });
            }
            // 航点录像跟着「离开航点」收口；**航段录像在这里起步**——离开起点航点
            // 才是这一段真正开始（到点时只记了「站在哪个航点」，没开录：
            // 那时狗还在跑这个航点的作业，录下来大半是原地不动）。
            // 本段到下一个航点在 `point_arrived` 里收口。
            //
            // **`pending_task`（已受理、底盘未驱动）不算起步**：那时狗还在排队等上一条
            // 任务停稳 / 资源释放，起录只会录到静止画面，还会和上一条任务的录像
            // 同时写盘 / 抢 CPU。真正驱动起来（成为 `current_task`）之后再起。
            recorder.leave_point("已离开航点").await;
            if !pending
                && let Some(task_code) = active
                    .lock()
                    .await
                    .as_ref()
                    .map(|task| task.task_code.clone())
            {
                recorder.leg_started(&task_code).await;
            }
        }
    }

    // 离散事件（去重后只报一次）
    if monitor.lock().await.take_event(&task) {
        let event = DeviceEventPayload {
            event_type: "progress".to_string(),
            message: format!("{}: {}", task.last_event_type, task.stage),
            data: Some(json!({
                "taskId": task.task_id,
                "stage": task.stage,
                // 断点续跑时用加回偏移的原始下标，与事件里的「P{n}」对各
                "index": index,
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
        // 现场对账最重要的一行：终态时本体到底把导航能力放掉了没有。
        // `task_state` 不是 idle 就是「本体还占着任务/SDK 控制」——下一单 5000 会回
        // 13897 capability busy，尾巴灯也会一直停在任务/SDK 控制色，物理遥控器进不来。
        let body = {
            let st = state.lock().await;
            (
                frame.robot_task_state.clone(),
                st.ctrl_source,
                st.robot_mode.clone(),
            )
        };
        let mut guard = active.lock().await;
        if let Some(a) = guard.as_ref() {
            info!(
                "任务 {} [{}] 进入终态: {}（本体 task_state={} ctrl_source={} mode={}）",
                a.task_code,
                a.task_name,
                task.lifecycle,
                if body.0.is_empty() {
                    "—"
                } else {
                    body.0.as_str()
                },
                body.1
                    .map(|source| source.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                body.2.as_deref().unwrap_or("—")
            );
        }
        // 收尾是异步的：期间可能已经换成了新下发的任务。只有 active 还绑着这条任务
        // 的 `task_id` 时才清，否则会把新任务的上下文一起清掉（平台就再看不到进度）。
        let ours = guard
            .as_ref()
            .and_then(|a| a.nav_task_id.as_deref())
            .is_some_and(|id| id == task.task_id);
        if ours {
            *guard = None;
        }
        drop(guard);
        if ours {
            state.lock().await.current_task = None;
        }
        // 本体还占着导航能力（典型是被 5004 暂停过 / 返航刚结束 / 任务被替换）：
        // 交给后台补 5002 再等 idle。不清的话下一条 5000 会被 13897 拒，
        // 狗体也会一直停在「任务 / SDK 控制」状态，物理遥控器接不回来。
        if !body.0.is_empty() && body.0 != "idle" && ours {
            return Some(TerminalCleanup {
                task_id: task.task_id.clone(),
                task_state: body.0.clone(),
            });
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_patrol::AiPatrol;
    use crate::lights::PointLights;
    use crate::photo::Photographer;
    use crate::record::Recorder;
    use crate::speak::Speaker;
    use crate::state::ActiveTask;
    use robodog_device_client::config::CliArgs;
    use robodog_device_client::config::EnvConfig;
    use robodog_device_client::config::FileConfig;
    use serde_json::json;

    fn test_config() -> Config {
        Config::resolve(
            &FileConfig::default(),
            &EnvConfig::default(),
            &CliArgs {
                device_id: Some("TEST-DEVICE".to_string()),
                ..CliArgs::default()
            },
            None,
        )
        .expect("测试配置应能解析")
    }

    struct Harness {
        out_rx: mpsc::Receiver<WsMessage>,
        out_tx: mpsc::Sender<WsMessage>,
        state: Shared<RobotState>,
        active: Shared<Option<ActiveTask>>,
        monitor: Arc<Mutex<TaskMonitor>>,
        reporter: ProgressReporter,
        ai: Arc<AiPatrol>,
        lights: Arc<PointLights>,
        recorder: Arc<Recorder>,
        speaker: Arc<Speaker>,
        photographer: Arc<Photographer>,
    }

    impl Harness {
        fn new(active_task: ActiveTask) -> Self {
            let (out_tx, out_rx) = mpsc::channel(16);
            let mut state = RobotState::new(&test_config());
            state.mode = "robot";
            let state: Shared<RobotState> = Arc::new(Mutex::new(state));
            let active: Shared<Option<ActiveTask>> = Arc::new(Mutex::new(Some(active_task)));
            Self {
                out_rx,
                out_tx,
                state,
                active,
                monitor: Arc::new(Mutex::new(TaskMonitor::default())),
                reporter: ProgressReporter::default(),
                ai: Arc::new(AiPatrol::with_jobs_dir("http://127.0.0.1:1", false, None)),
                lights: Arc::new(PointLights::new("http://127.0.0.1:1", false)),
                recorder: Arc::new(Recorder::new(Arc::new(test_config()), false)),
                speaker: Arc::new(Speaker::new("http://127.0.0.1:1", false)),
                photographer: Arc::new(Photographer::new(Arc::new(test_config()), false)),
            }
        }

        /// 打开航点灯光（指向一个假 Agent）：用来断言「什么时候开、什么时候关」
        fn with_lights(mut self, agent: String) -> Self {
            self.lights = Arc::new(PointLights::new(agent, true));
            self
        }

        /// 打开语音播报（指向一个假 Agent）：用来断言到点播报 / 离开停播的时机
        fn with_speaker(mut self, agent: String) -> Self {
            self.speaker = Arc::new(Speaker::new(agent, true));
            self
        }

        /// 打开航点拍照（指向一个假 Agent）：用来断言到点抓拍的时机
        fn with_photographer(mut self, agent: String) -> Self {
            let mut config = test_config();
            config.ai_agent_url = agent;
            self.photographer = Arc::new(Photographer::new(Arc::new(config), true));
            self
        }

        /// 打开录像（指向一个假 Agent）：用来断言「什么时候起录、什么时候停」
        fn with_recorder(mut self, agent: String) -> Self {
            let mut config = test_config();
            config.ai_agent_url = agent;
            self.recorder = Arc::new(Recorder::new(Arc::new(config), true));
            self
        }

        async fn feed(&mut self, frame: &StatusFrame) -> Option<TerminalCleanup> {
            handle_status_frame(
                frame,
                &self.out_tx,
                &self.state,
                &self.active,
                &self.monitor,
                &mut self.reporter,
                &self.ai,
                &self.lights,
                &self.recorder,
                &self.speaker,
                &self.photographer,
            )
            .await
        }

        /// 取出所有发往平台的 `task_progress` 载荷（Value，方便按字段断言）
        fn drain_progress(&mut self) -> Vec<serde_json::Value> {
            let mut sent = Vec::new();
            while let Ok(msg) = self.out_rx.try_recv() {
                if msg.msg_type == "task_progress" {
                    sent.push(msg.payload);
                }
            }
            sent
        }
    }

    fn active_task(code: &str, nav_task_id: Option<&str>) -> ActiveTask {
        ActiveTask {
            task_code: code.to_string(),
            task_name: "A区巡检".to_string(),
            nav_task_id: nav_task_id.map(str::to_string),
            ..ActiveTask::default()
        }
    }

    /// 假 Agent：记录每一次 `/api/lights` 与 `/api/lights/off`
    type LightCalls = Arc<Mutex<Vec<String>>>;

    async fn spawn_lights_agent(calls: LightCalls) -> String {
        let app = axum::Router::new()
            .route(
                "/api/lights",
                axum::routing::post(
                    |axum::extract::State(calls): axum::extract::State<LightCalls>| async move {
                        calls.lock().await.push("on".to_string());
                        axum::Json(json!({"ok": true, "applied": {"fill": true, "led": true}}))
                    },
                ),
            )
            .route(
                "/api/lights/off",
                axum::routing::post(
                    |axum::extract::State(calls): axum::extract::State<LightCalls>| async move {
                        calls.lock().await.push("off".to_string());
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    /// 假 Agent：记录每一次 `/api/speak`（`say:<内容>`）与 `/api/speak/stop`（`stop`）。
    /// 语音播报是串行 await 的，但还是等一拍再断言，避免测试机调度抖动。
    type SpeakCalls = Arc<Mutex<Vec<String>>>;

    async fn spawn_speak_agent(calls: SpeakCalls) -> String {
        let app = axum::Router::new()
            .route(
                "/api/speak",
                axum::routing::post(
                    |axum::extract::State(calls): axum::extract::State<SpeakCalls>,
                     axum::Json(body): axum::Json<serde_json::Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(format!("say:{}", body["text"].as_str().unwrap_or("")));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/speak/stop",
                axum::routing::post(
                    |axum::extract::State(calls): axum::extract::State<SpeakCalls>| async move {
                        calls.lock().await.push("stop".to_string());
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    async fn wait_speak(calls: &SpeakCalls, expected: usize) -> Vec<String> {
        for _ in 0..100 {
            if calls.lock().await.len() >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        calls.lock().await.clone()
    }

    /// 假 Agent：记录 `/api/record/start|stop` 的调用（`start:<scope>` / `stop`）
    type RecordCalls = Arc<Mutex<Vec<String>>>;

    async fn spawn_record_agent(calls: RecordCalls) -> String {
        let app = axum::Router::new()
            .route(
                "/api/record/start",
                axum::routing::post(
                    |axum::extract::State(calls): axum::extract::State<RecordCalls>,
                     axum::Json(body): axum::Json<serde_json::Value>| async move {
                        calls.lock().await.push(format!(
                            "start:{}",
                            body["scope"].as_str().unwrap_or("point")
                        ));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/record/stop",
                axum::routing::post(
                    |axum::extract::State(calls): axum::extract::State<RecordCalls>| async move {
                        calls.lock().await.push("stop".to_string());
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    async fn wait_records(calls: &RecordCalls, expected: usize) -> Vec<String> {
        for _ in 0..100 {
            if calls.lock().await.len() >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        calls.lock().await.clone()
    }

    /// 开灯是 `tokio::spawn` 出去的：等它落地再断言，最多等 1s。
    async fn wait_lights(calls: &LightCalls, expected: usize) -> Vec<String> {
        for _ in 0..100 {
            if calls.lock().await.len() >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        calls.lock().await.clone()
    }

    fn frame(current: Value, pending: Value, terminals: Value) -> StatusFrame {
        frame_with_task_state(current, pending, terminals, None)
    }

    /// `task_state = None` 时按「有 current 就 running、否则 idle」推
    fn frame_with_task_state(
        current: Value,
        pending: Value,
        terminals: Value,
        task_state: Option<&str>,
    ) -> StatusFrame {
        let task_state = task_state.map(str::to_string).unwrap_or_else(|| {
            if current.is_null() {
                "idle".to_string()
            } else {
                "running".to_string()
            }
        });
        StatusFrame::parse(&json!({
            "schema_version": 2,
            "boot_id": "boot-a",
            "snapshot_seq": 1,
            "revision": 1,
            "reason": "heartbeat",
            "robot": {
                "task_state": task_state,
                "loc_state": { "status": 100, "fresh": true }
            },
            "current_task": current,
            "pending_task": pending,
            "recent_terminal_tasks": terminals
        }))
        .expect("5009 v2 帧要能解析")
    }

    fn task(id: &str, kind: &str, lifecycle: &str, stage: &str) -> Value {
        json!({
            "task_id": id,
            "type": kind,
            "name": "A区巡检",
            "lifecycle": lifecycle,
            "stage": stage,
            "progress": 100.0,
            "last_event": { "seq": 1, "type": stage },
        })
    }

    /// 带航点下标的 5009 任务帧（航点动作 / 计划灯的到点判定都看 `detail_index`）
    fn task_at(id: &str, lifecycle: &str, stage: &str, index: i64) -> Value {
        let mut value = task(id, "patrol", lifecycle, stage);
        value["last_event"]["data"] = json!({ "index": index });
        value
    }

    /// 刚下发（5000 还没回、没有 nav_task_id）时，上一轮留在
    /// `recent_terminal_tasks` 里的终态**不能**算到这条新任务头上。
    ///
    /// 旧逻辑用 `task_type == "patrol"` 兜底，会把平台侧的新任务立刻上报成
    /// 「已完成」；服务端 `task_status` 不可回退，之后 runner 报「执行中」也写不进去，
    /// 平台就永远停在「已完成」（狗其实才刚要起步）。
    #[tokio::test]
    async fn stale_terminal_is_not_attributed_to_a_fresh_dispatch() {
        let mut h = Harness::new(active_task("TK-NEW", None));
        let frame = frame(
            Value::Null,
            Value::Null,
            json!([task("robot-old", "patrol", "success", "patrol.completed")]),
        );
        h.feed(&frame).await;

        assert!(
            h.drain_progress().is_empty(),
            "没有绑定 task_id 时不许上报任何进度/终态"
        );
        assert!(
            h.active.lock().await.is_some(),
            "新任务的上下文不能被上一轮的终态清掉"
        );
    }

    /// 巡检（`TaskMode::Route`）的本体任务号只存在于本地 runner 里；拿到后必须补进
    /// `active.nav_task_id`，否则 5009 的跟随整条链路（航点 AI / 航点灯光 / 阶段上报）
    /// 都在 `bound_id?` 处静默返回——现场看到的就是「计划勾了自动开灯，到点灯不亮」。
    #[tokio::test]
    async fn route_task_must_bind_the_runner_task_id_before_5009_is_followed() {
        use crate::state::bind_nav_task;

        let mut h = Harness::new(active_task("TK-1", None));
        let frame = frame(
            task("robot-1", "patrol", "running", "patrol.point_arrived"),
            Value::Null,
            json!([]),
        );
        // 没绑定：不跟随（这是为了不把上一轮的终态/进度算到新任务上）
        h.feed(&frame).await;
        assert!(
            h.drain_progress().is_empty(),
            "没绑定本体任务号时不该跟随 5009"
        );

        // 只有「还是这条任务且尚未绑定」才写得进去
        assert!(!bind_nav_task(&h.active, "TK-2", "robot-9").await);
        assert!(bind_nav_task(&h.active, "TK-1", "robot-1").await);
        assert!(
            !bind_nav_task(&h.active, "TK-1", "robot-2").await,
            "已绑定的任务号不能被后来的帧改掉"
        );

        // 绑定之后：到点帧必须能翻译成业务进度（灯光 / AI 挂在这条路径上）
        h.feed(&frame).await;
        let sent = h.drain_progress();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0]["message"], "已到达航点");
    }

    /// 断点续跑：本体路径里只有断点之后的航点，`detail_index` 是本轮下标；
    /// 上报给平台的 `waypointIndex` 与本地动作表查询必须用**原始**下标。
    ///
    /// 不加偏移的后果：续跑那一轮平台会以为狗站在 P1（AI 窗口开到错的点上）、
    /// 本地拍照 / 录像 / 播报 / 灯光全部错位。
    #[tokio::test]
    async fn resumed_run_reports_the_original_waypoint_index() {
        let mut task = active_task("TK-1", Some("robot-1"));
        // 平台的 `startIndex = 3`：本轮本体路径从原始第 4 个航点（P4）开始
        task.start_index = 3;
        let mut h = Harness::new(task);
        // 本体报「本轮第 2 个点到了」= 原始下标 4（P5）；本轮路径共 7 个点（整条 10 个）
        let mut reported = task_at("robot-1", "running", "patrol.point_arrived", 1);
        reported["last_event"]["data"]["total"] = json!(7);
        h.feed(&frame(reported, Value::Null, json!([]))).await;

        let sent = h.drain_progress();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(
            sent[0]["waypointIndex"],
            json!(4),
            "对上平台 `routeData.points` 的下标：{}",
            sent[0]
        );
        assert_eq!(sent[0]["pointIndex"], json!(4));
        assert_eq!(
            sent[0]["progress"],
            json!(50),
            "进度要按整条路线算（(4+1)/10），不能拿本体的 1/7=14% 让进度条往回跳：{}",
            sent[0]
        );
        assert_eq!(
            h.active.lock().await.as_ref().map(|task| task.point_index),
            Some(Some(4)),
            "本地动作表（AI / 灯光 / 拍照 / 播报）也按原始下标查"
        );
    }

    /// `pending_task` 是「已受理、等旧任务停稳 / 释放资源」：必须能跟随、能上报，
    /// 否则平台只看到「执行中」，而狗其实还没被驱动（尾巴灯也不是任务色）。
    #[tokio::test]
    async fn pending_task_is_reported_while_the_body_is_still_busy() {
        let mut h = Harness::new(active_task("TK-1", Some("robot-1")));
        let frame = frame(
            Value::Null,
            task("robot-1", "patrol", "pending", "patrol.preparing"),
            json!([]),
        );
        h.feed(&frame).await;

        let sent = h.drain_progress();
        assert_eq!(sent.len(), 1, "pending 也要上报一次进度: {sent:?}");
        assert_eq!(sent[0]["taskCode"], "TK-1");
        assert_eq!(sent[0]["robotTaskId"], "robot-1");
        assert_eq!(sent[0]["status"], "执行中");
        // 底盘还没动，不能报「已完成」
        assert!(
            h.active.lock().await.is_some(),
            "pending 不是终态，任务上下文必须保留"
        );
    }

    /// 终态但本体 `task_state` 还不是 idle：必须把「要给它补 5002」这件事交出去，
    /// 否则导航能力一直被占着（下一条 5000 回 13897，狗体停在任务/SDK 控制态）。
    #[tokio::test]
    async fn terminal_with_capability_still_busy_asks_for_cleanup() {
        let mut h = Harness::new(active_task("TK-1", Some("robot-1")));
        let frame = frame_with_task_state(
            task("robot-1", "patrol", "success", "patrol.completed"),
            Value::Null,
            json!([]),
            Some("running"),
        );
        let cleanup = h.feed(&frame).await;
        let cleanup = cleanup.expect("本体没释放导航能力时必须请求收尾");
        assert_eq!(cleanup.task_id, "robot-1");
        assert_eq!(cleanup.task_state, "running");
        // 上报仍然是「已完成」（任务本身成功了），收尾只影响本体状态
        let sent = h.drain_progress();
        assert_eq!(
            sent.last().map(|p| p["status"].clone()),
            Some(json!("已完成"))
        );
    }

    /// 本体自己已经回到 idle：不要再发 5002（对已结束的任务只会有 13896 噪声）
    #[tokio::test]
    async fn terminal_with_idle_capability_needs_no_cleanup() {
        let mut h = Harness::new(active_task("TK-1", Some("robot-1")));
        let frame = frame_with_task_state(
            task("robot-1", "patrol", "success", "patrol.completed"),
            Value::Null,
            json!([]),
            Some("idle"),
        );
        assert!(h.feed(&frame).await.is_none(), "idle 时不该再补 5002");
    }

    /// 终态只清「自己那条」：如果 active 已经换成新任务（旧任务的终态帧晚到），
    /// 不能把新任务的上下文清掉。
    #[tokio::test]
    async fn terminal_of_another_task_does_not_clear_the_current_one() {
        let mut h = Harness::new(active_task("TK-NEW", Some("robot-new")));
        let frame = frame(
            Value::Null,
            Value::Null,
            json!([task("robot-old", "patrol", "success", "patrol.completed")]),
        );
        h.feed(&frame).await;

        assert!(h.drain_progress().is_empty(), "别人的终态不许上报");
        let guard = h.active.lock().await;
        assert_eq!(
            guard.as_ref().map(|task| task.task_code.as_str()),
            Some("TK-NEW"),
            "新任务上下文不能被旧任务的终态清掉"
        );
        // 清理通道没有收尾残留（保证没有「已完成」之类的误报）
        drop(guard);
    }

    /// 计划级自动开灯：整个任务期间只开一次、航点切换不关，终态才关。
    ///
    /// 现场要求「计划里配了自动开灯，航点上的开灯关灯不生效，以计划为准」——
    /// 所以哪怕这个航点的动作写着 `lightLed: false`，也不能把计划灯盖掉。
    #[tokio::test]
    async fn plan_lights_stay_on_for_the_whole_task() {
        use crate::lights::LightPoint;
        use crate::lights::LightScope;
        use crate::lights::PLAN_LIGHT_INDEX;

        let calls: LightCalls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_lights_agent(calls.clone()).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1"))).with_lights(agent);
        {
            let mut guard = h.active.lock().await;
            let task = guard.as_mut().unwrap();
            task.plan_lights = Some(LightPoint {
                index: PLAN_LIGHT_INDEX,
                scope: LightScope::Point,
                fill: true,
                led: true,
            });
            // 航点动作里只配了 LED：计划模式下必须被忽略
            task.light_points = vec![LightPoint {
                index: 0,
                scope: LightScope::Point,
                fill: false,
                led: true,
            }];
        }

        // 任务刚起来（还在导航）：计划灯立刻亮，不等第一个航点
        h.feed(&frame(
            task("robot-1", "patrol", "running", "patrol.navigating"),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(wait_lights(&calls, 1).await, vec!["on".to_string()]);

        // 到点 / 离开航点：一站都不许关（5009 每 500ms 一帧也只发一次开灯）
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.navigating", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_lights(&calls, 1).await,
            vec!["on".to_string()],
            "计划灯不能在航点切换时关掉"
        );

        // 终态：关灯
        h.feed(&frame(
            task("robot-1", "patrol", "success", "patrol.completed"),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_lights(&calls, 2).await,
            vec!["on".to_string(), "off".to_string()],
            "计划灯只在终态关"
        );
    }

    /// 没配计划灯（老计划）：还是「到点开、离开关」，航点动作照旧生效。
    #[tokio::test]
    async fn waypoint_lights_without_plan_still_flip_per_point() {
        use crate::lights::LightPoint;
        use crate::lights::LightScope;

        let calls: LightCalls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_lights_agent(calls.clone()).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1"))).with_lights(agent);
        h.active.lock().await.as_mut().unwrap().light_points = vec![LightPoint {
            index: 0,
            scope: LightScope::Point,
            fill: true,
            led: false,
        }];

        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(wait_lights(&calls, 1).await, vec!["on".to_string()]);

        // 离开航点：关灯（这条路径不能被计划模式的新增分支带坏）
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.navigating", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_lights(&calls, 2).await,
            vec!["on".to_string(), "off".to_string()]
        );
    }

    /// 航段灯光（行进任务上的「开灯」）：**起步才开（离开起点航点）、走完这一段才关**。
    ///
    /// 2026-11-25 现场（`gh-test`）：P1 的航点动作没配灯、航线 1-2 配了补光灯，
    /// 狗到 P1 之后航点任务（AI 识别 / 停留 10 s）还没跑完灯就亮了 —— 亮着的补光灯
    /// 会把识别画面与存证图一起污染。航段灯必须等狗真的起步
    /// （阶段离开 `patrol.point_*`）才点亮，到点那一刻只记账。
    #[tokio::test]
    async fn leg_lights_start_when_the_dog_leaves_the_waypoint() {
        use crate::lights::LightPoint;
        use crate::lights::LightScope;

        let calls: LightCalls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_lights_agent(calls.clone()).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1"))).with_lights(agent);
        // legs[0] 从 P1（下标 0）出发：只有这一段配了灯
        h.active.lock().await.as_mut().unwrap().light_points = vec![LightPoint {
            index: 0,
            scope: LightScope::Leg,
            fill: true,
            led: true,
        }];

        // 到点 + 这个点上的全部作业阶段（5009 每秒重复报）：一帧都不许开灯
        for stage in [
            "patrol.point_arrived",
            "patrol.waiting",
            "patrol.photographing",
            "patrol.point_completed",
        ] {
            h.feed(&frame(
                task_at("robot-1", "running", stage, 0),
                Value::Null,
                json!([]),
            ))
            .await;
        }
        // 到点帧可能已经把「到点开灯」的 spawn 发出去了：给后台任务一点时间
        // （航段灯这条路径本来就不该发请求，等一下才能证明是「没发」而不是「还没轮到」）
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            calls.lock().await.is_empty(),
            "航点任务还没跑完就开灯了: {:?}",
            calls.lock().await
        );

        // 起步走过这一段：现在才点亮，且一路不许关（5009 每 500ms 一帧）
        for _ in 0..3 {
            h.feed(&frame(
                task_at("robot-1", "running", "patrol.navigating", 0),
                Value::Null,
                json!([]),
            ))
            .await;
        }
        assert_eq!(
            wait_lights(&calls, 1).await,
            vec!["on".to_string()],
            "起步要开灯、行进途中不能关掉"
        );

        // 走到下一个点（P2 没配灯）：这时候才收灯
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_lights(&calls, 2).await,
            vec!["on".to_string(), "off".to_string()],
            "走完这一段要关灯"
        );
    }

    /// 航点拍照跟着 5009 走：到点抓一张（重复帧不重抓），没配拍照的点不发请求。
    #[tokio::test]
    async fn waypoint_photo_is_captured_on_arrival_only_once() {
        use crate::photo::{PhotoPlan, PhotoPoint};

        let calls: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_photo_agent(calls.clone()).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1"))).with_photographer(agent);
        h.photographer
            .start_task(
                "TK-1",
                PhotoPlan {
                    points: vec![PhotoPoint {
                        index: 0,
                        name: "P1".to_string(),
                        camera: "dogcam".to_string(),
                        frames: 1,
                    }],
                },
            )
            .await;

        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.waiting", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        // 没有拍照动作的点：不发请求
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 1),
            Value::Null,
            json!([]),
        ))
        .await;

        let calls = wait_photo_calls(&calls, 1).await;
        let captures: Vec<&Value> = calls
            .iter()
            .filter(|call| call["path"] == "photo")
            .collect();
        assert_eq!(captures.len(), 1, "同一个点只抓一次：{calls:?}");
        assert_eq!(captures[0]["body"]["planId"], "TK-1");
        assert_eq!(captures[0]["body"]["pointIndex"], 0);
    }

    /// 假 Agent：记录每一次 `/api/photo`（`capture:<航点>`）
    type PhotoCalls = Arc<Mutex<Vec<Value>>>;

    async fn spawn_photo_agent(calls: PhotoCalls) -> String {
        let app = axum::Router::new()
            .route(
                "/api/photo",
                axum::routing::post(
                    |axum::extract::State(calls): axum::extract::State<PhotoCalls>,
                     axum::Json(body): axum::Json<serde_json::Value>| async move {
                        let point = body["pointIndex"].as_i64().unwrap_or(-1);
                        calls
                            .lock()
                            .await
                            .push(json!({"path": "photo", "body": body}));
                        axum::Json(
                            json!({"ok": true, "files": [{"name": format!("pt{point}.jpg")}]}),
                        )
                    },
                ),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    async fn wait_photo_calls(calls: &PhotoCalls, expected: usize) -> Vec<Value> {
        for _ in 0..100 {
            if calls.lock().await.len() >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        calls.lock().await.clone()
    }

    /// 语音播报跟着 5009 走：到点播一次（重复帧不重播），离开航点停循环播报，
    /// 终态再兜一次收口。这条链路与灯光 / 录像同源，但时序要求更严（stop 不能抢在
    /// start 前面），所以这里用真实 5009 帧驱动一遍。
    #[tokio::test]
    async fn waypoint_speech_fires_on_arrival_and_stops_on_leave() {
        use crate::speak::{SpeakPlan, SpeakPoint};

        let calls: SpeakCalls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_speak_agent(calls.clone()).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1"))).with_speaker(agent);
        h.speaker
            .start_task(
                "TK-1",
                SpeakPlan {
                    points: vec![SpeakPoint {
                        index: 0,
                        name: "P1".to_string(),
                        text: "前方作业，请勿靠近".to_string(),
                        // 循环播报：离开航点必须发 stop，否则狗走了还在喊
                        seconds: 30,
                    }],
                    legs: vec![],
                },
            )
            .await;

        // 到点 + 5009 的重复帧：只播一次
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.waiting", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_speak(&calls, 1).await,
            vec!["say:前方作业，请勿靠近".to_string()],
            "同一个航点的重复帧不能重播"
        );

        // 离开航点：循环播报收口
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.navigating", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_speak(&calls, 2).await,
            vec!["say:前方作业，请勿靠近".to_string(), "stop".to_string()],
            "离开航点要停循环播报"
        );
    }

    /// 共享时间线：两个假 Agent（播报 / 拍照）按真实顺序各记一行，
    /// 用来验「停播要排在同步抓图之前」这种跨模块的先后。
    type Timeline = Arc<Mutex<Vec<String>>>;

    async fn spawn_timeline_speak_agent(timeline: Timeline) -> String {
        let app = axum::Router::new()
            .route(
                "/api/speak",
                axum::routing::post(
                    |axum::extract::State(timeline): axum::extract::State<Timeline>,
                     axum::Json(_body): axum::Json<serde_json::Value>| async move {
                        timeline.lock().await.push("speak:say".to_string());
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/speak/stop",
                axum::routing::post(
                    |axum::extract::State(timeline): axum::extract::State<Timeline>| async move {
                        timeline.lock().await.push("speak:stop".to_string());
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .with_state(timeline);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    /// 假 Agent：`/api/photo` 故意**慢**（真机 ffmpeg 抓图 2~3s），进出各记一行
    async fn spawn_slow_photo_agent(timeline: Timeline, delay: Duration) -> String {
        let app = axum::Router::new()
            .route(
                "/api/photo",
                axum::routing::post(
                    move |axum::extract::State(timeline): axum::extract::State<Timeline>| async move {
                        timeline.lock().await.push("photo:start".to_string());
                        tokio::time::sleep(delay).await;
                        timeline.lock().await.push("photo:done".to_string());
                        axum::Json(json!({"ok": true, "files": [{"name": "pt1.jpg"}]}))
                    },
                ),
            )
            .with_state(timeline);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    /// 假 Agent：`/api/record/stop` 故意**慢**（真机要等 ffmpeg 写完 moov，最长 8s），
    /// 进出各记一行
    async fn spawn_slow_timeline_record_agent(timeline: Timeline, delay: Duration) -> String {
        let app = axum::Router::new()
            .route(
                "/api/record/start",
                axum::routing::post(
                    |axum::extract::State(timeline): axum::extract::State<Timeline>| async move {
                        timeline.lock().await.push("record:start".to_string());
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/record/stop",
                axum::routing::post(
                    move |axum::extract::State(timeline): axum::extract::State<Timeline>| async move {
                        timeline.lock().await.push("record:stop".to_string());
                        tokio::time::sleep(delay).await;
                        timeline.lock().await.push("record:done".to_string());
                        axum::Json(json!({"ok": true, "stopped": 1}))
                    },
                ),
            )
            .with_state(timeline);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    /// 与到点同样的毛病在「离开航点」这一侧：点播报的收口排在
    /// `recorder.leave_point`（板端 `/api/record/stop` 要等 ffmpeg 收尾）后面，
    /// 就是「狗起步走了，点上那几句还在喊」。
    #[tokio::test]
    async fn leaving_stops_point_speech_before_the_slow_record_flush() {
        use crate::record::{RecordPlan, RecordPoint};
        use crate::speak::{SpeakPlan, SpeakPoint};

        let timeline: Timeline = Arc::new(Mutex::new(Vec::new()));
        let speak = spawn_timeline_speak_agent(timeline.clone()).await;
        let record =
            spawn_slow_timeline_record_agent(timeline.clone(), Duration::from_millis(300)).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1")))
            .with_speaker(speak)
            .with_recorder(record);
        h.speaker
            .start_task(
                "TK-1",
                SpeakPlan {
                    points: vec![SpeakPoint {
                        index: 1,
                        name: "P2".to_string(),
                        text: "前方作业，请勿靠近".to_string(),
                        seconds: 60,
                    }],
                    legs: vec![],
                },
            )
            .await;
        h.recorder
            .start_task(
                "TK-1",
                RecordPlan {
                    points: vec![RecordPoint {
                        index: 1,
                        name: "P2".to_string(),
                        seconds: 15,
                        cameras: vec![],
                    }],
                    legs: vec![],
                },
            )
            .await;

        // 到点：开播 + 开录
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        // 离开：先停播（毫秒级），再去停录（板端要等 ffmpeg 收尾）
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.navigating", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        let log = timeline.lock().await.clone();
        assert_eq!(
            log,
            [
                "speak:say",
                "record:start",
                "speak:stop",
                "record:stop",
                "record:done"
            ],
            "停播要排在停录之前（反过来就是狗走了还在喊）"
        );
    }

    /// 现场回归（2026-11-25 gh-test2 / TK-1125-19，P3）：狗 1:56 停住，播报 2:01 才结束。
    /// 日志对得上：`15:33:25.39` 到点 → `[photo] 航点 2/dogcam 抓拍 1 张（2.9s）`
    /// → `15:33:28.29` 才 `POST /api/speak/stop`。**停播被同步抓图挡在后面**，
    /// 所以到点分支里播报必须排第一（它只是本机一次 HTTP POST）。
    #[tokio::test]
    async fn speech_stop_goes_out_before_the_slow_photo_capture() {
        use crate::photo::{PhotoPlan, PhotoPoint};
        use crate::speak::{SpeakLeg, SpeakPlan};

        let timeline: Timeline = Arc::new(Mutex::new(Vec::new()));
        let speak = spawn_timeline_speak_agent(timeline.clone()).await;
        let photo = spawn_slow_photo_agent(timeline.clone(), Duration::from_millis(300)).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1")))
            .with_speaker(speak)
            .with_photographer(photo);
        h.speaker
            .start_task(
                "TK-1",
                SpeakPlan {
                    points: vec![],
                    legs: vec![SpeakLeg {
                        index: 1,
                        text: "巡检中，请注意避让".to_string(),
                        seconds: 120,
                    }],
                },
            )
            .await;
        h.photographer
            .start_task(
                "TK-1",
                PhotoPlan {
                    points: vec![PhotoPoint {
                        index: 2,
                        name: "P3".to_string(),
                        camera: "dogcam".to_string(),
                        frames: 1,
                    }],
                },
            )
            .await;

        // 到达 P2（这一段还没开始走：航段播报不播）
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        // 起步（离开 P2）：航段播报开始
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.navigating", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        // 到 P3：这一段走完 —— 先停播，再抓图
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 2),
            Value::Null,
            json!([]),
        ))
        .await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        let log = timeline.lock().await.clone();
        assert_eq!(
            log,
            ["speak:say", "speak:stop", "photo:start", "photo:done"],
            "停播必须排在同步抓图之前（反过来就是狗停住了还在喊）"
        );
    }

    /// 航段播报：**起步行才开播**，**走完整段**才收口（到下一个点 / 终态）。
    ///
    /// 两端都不能搞错：
    /// - 到点就开播 = 循环时长先被停留 / AI 识别吃掉，人还没走话已经说完（与航段补光灯同一个问题）；
    /// - 2026-11-25 gh-test「lrc」现场：P2→P3 配了「语音播报」，狗一离开 P2
    ///   就静音——「离开航点」把它当成航点播报收掉了，而离开起点航点正是航段的开始。
    #[tokio::test]
    async fn leg_speech_starts_on_departure_and_plays_until_the_next_point() {
        use crate::speak::{SpeakLeg, SpeakPlan};

        let calls: SpeakCalls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_speak_agent(calls.clone()).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1"))).with_speaker(agent);
        h.speaker
            .start_task(
                "TK-1",
                SpeakPlan {
                    points: vec![],
                    legs: vec![SpeakLeg {
                        index: 1,
                        text: "正在通过仓储区".to_string(),
                        seconds: 60,
                    }],
                },
            )
            .await;

        // 到达 P2 = 进入 P2→P3 航段：**到点不播**（航点任务 / 停留还没跑完）
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.waiting", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            calls.lock().await.is_empty(),
            "到点不能开播: {:?}",
            calls.lock().await
        );

        // 离开 P2（狗开始走这一段）：航段播报现在开播，而且**不能**停
        for _ in 0..2 {
            h.feed(&frame(
                task_at("robot-1", "running", "patrol.navigating", 1),
                Value::Null,
                json!([]),
            ))
            .await;
        }
        assert_eq!(
            wait_speak(&calls, 1).await,
            vec!["say:正在通过仓储区".to_string()],
            "航段播报要跟着狗走完整段：起步开播、不能一离开起点航点就停"
        );

        // 到达 P3：这一段走完了 → 收口（P3 没配播报，所以只多一条 stop）
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 2),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_speak(&calls, 2).await,
            vec!["say:正在通过仓储区".to_string(), "stop".to_string()]
        );

        // 到达 P3 的重复帧、离开 P3：都不再发 stop
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.waiting", 2),
            Value::Null,
            json!([]),
        ))
        .await;
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.navigating", 2),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(wait_speak(&calls, 2).await.len(), 2, "停止只发一次");
    }

    /// 航点录像（「录像采集」）的时机：**到点起、离开停**。到点就是该录的时刻
    /// （要录下这个点的作业），不去起步；导航途中不能提前起录。
    #[tokio::test]
    async fn waypoint_recording_starts_on_arrival_and_stops_when_leaving() {
        use crate::record::{RecordPlan, RecordPoint};

        let calls: RecordCalls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_record_agent(calls.clone()).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1"))).with_recorder(agent);
        h.recorder
            .start_task(
                "TK-1",
                RecordPlan {
                    points: vec![RecordPoint {
                        index: 0,
                        name: "P1".to_string(),
                        seconds: 15,
                        cameras: vec![],
                    }],
                    legs: vec![],
                },
            )
            .await;

        // 前往 P1 的路上：不该录
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.navigating", 0),
            Value::Null,
            json!([]),
        ))
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            calls.lock().await.is_empty(),
            "导航途中不该起录: {:?}",
            calls.lock().await
        );

        // 到点：开录（5009 每秒重复报 waiting 也只开一次）
        for stage in ["patrol.point_arrived", "patrol.waiting", "patrol.waiting"] {
            h.feed(&frame(
                task_at("robot-1", "running", stage, 0),
                Value::Null,
                json!([]),
            ))
            .await;
        }
        assert_eq!(
            wait_records(&calls, 1).await,
            vec!["start:point".to_string()]
        );

        // 离开航点：停掉**航点**录像（录不到下一段路；这一段由航段录像接手）
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.navigating", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_records(&calls, 2).await,
            vec!["start:point".to_string(), "stop".to_string()]
        );
    }

    /// 航段录像（段级）：**到点不开录**（航点作业还没跑完）、**起步才开**、
    /// 到下一个航点收；`pending_task`（已受理、底盘未驱动）不算起步。
    #[tokio::test]
    async fn leg_recording_starts_on_departure_and_stops_at_the_next_point() {
        use crate::record::{RecordLeg, RecordPlan};

        let calls: RecordCalls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_record_agent(calls.clone()).await;
        let mut h = Harness::new(active_task("TK-1", Some("robot-1"))).with_recorder(agent);
        h.recorder
            .start_task(
                "TK-1",
                RecordPlan {
                    points: vec![],
                    legs: vec![RecordLeg {
                        index: 0,
                        name: "航段 1→2".to_string(),
                        segment_seconds: 120,
                        cameras: vec![],
                    }],
                },
            )
            .await;

        // 排队（pending_task，底盘未驱动）：不录
        h.feed(&frame(
            Value::Null,
            task("robot-1", "patrol", "running", "patrol.navigating"),
            json!([]),
        ))
        .await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            calls.lock().await.is_empty(),
            "排队期间不该起录: {:?}",
            calls.lock().await
        );

        // 到点 + 航点作业阶段：只记「站在哪个航点」，不开录
        for stage in [
            "patrol.point_arrived",
            "patrol.waiting",
            "patrol.point_completed",
        ] {
            h.feed(&frame(
                task_at("robot-1", "running", stage, 0),
                Value::Null,
                json!([]),
            ))
            .await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            calls.lock().await.is_empty(),
            "航点作业还没跑完就开录了: {:?}",
            calls.lock().await
        );

        // 起步（离开 P1）：这一段开始录（行进帧每 500ms 一帧，只起一次）
        for _ in 0..2 {
            h.feed(&frame(
                task_at("robot-1", "running", "patrol.navigating", 0),
                Value::Null,
                json!([]),
            ))
            .await;
        }
        assert_eq!(
            wait_records(&calls, 1).await,
            vec!["start:leg".to_string()],
            "起步才开录、而且只开一次"
        );

        // 到达下一个航点：这一段走完 → 收口（不是任务终态才停）
        h.feed(&frame(
            task_at("robot-1", "running", "patrol.point_arrived", 1),
            Value::Null,
            json!([]),
        ))
        .await;
        assert_eq!(
            wait_records(&calls, 2).await,
            vec!["start:leg".to_string(), "stop".to_string()]
        );
    }
}
