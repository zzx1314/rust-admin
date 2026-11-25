//! 远程指令：本体动作、地图操作与媒体同步。

use crate::locate::LOCATE_TIMEOUT;
use crate::locate::parse_initial_pose;
use crate::maps::push_map_to_body;
use crate::maps::sync_maps_from_robot;
use crate::media_sync;
use crate::record::Recorder;
use crate::speak;
use crate::state::ActiveTask;
use crate::state::RobotState;
use crate::state::Shared;
use crate::wire::DeviceEventPayload;
use crate::wire::WsMessage;
use crate::wire::command_request_id;
use crate::wire::send;
use crate::wire::send_command_result;
use crate::wire::send_command_result_with;
use crate::zhpro;
use crate::zhpro::client::ZhProClient;
use crate::zhpro::mapping::MappingSession;
use crate::zhpro::route::RouteRunner;
use chrono::Local;
use robodog_device_client::config::Config;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::info;
use tracing::warn;
use zhpro::mapping::MapManager;
use zhpro::route::Control;
use zhpro::teleop::SharedTeleop;

use zhpro::protocol::*;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle(
    config: &Config,
    msg: WsMessage,
    out_tx: &mpsc::Sender<WsMessage>,
    state: &Shared<RobotState>,
    active: &Shared<Option<ActiveTask>>,
    zhpro: &Option<Arc<ZhProClient>>,
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    local_mapping: &Shared<Option<Arc<MappingSession>>>,
    teleop: &SharedTeleop,
    body_reconnect: &mpsc::Sender<()>,
    recorder: &Arc<Recorder>,
) {
    let command = msg.payload["command"].as_str().unwrap_or("unknown");
    info!("收到远程指令: {command}");

    // 「本体重连」：进程内重建 control/bulkdata，不退出进程、业务 WebSocket 不断。
    // 放在所有门禁之前：本体已经断链时（`zhpro` 为空）也要能触发重连。
    // 只在空闲时执行：带任务/建图/接管重建会丢运行态。
    if command == "本体重连" {
        let refused = {
            let engaged = zhpro::teleop::is_engaged(teleop).await;
            let busy_task = active.lock().await.is_some();
            let busy_mapping = local_mapping.lock().await.is_some();
            if engaged {
                Some("当前处于远程接管中，请先「释放接管」")
            } else if busy_task {
                Some("有任务在执行，请先暂停/取消任务")
            } else if busy_mapping {
                Some("建图会话进行中，请先结束建图")
            } else {
                None
            }
        };
        if let Some(reason) = refused {
            warn!("「本体重连」被拒绝: {reason}");
            let event = DeviceEventPayload {
                event_type: "error".to_string(),
                message: format!("重连本体被拒绝: {reason}"),
                data: None,
            };
            send(
                out_tx,
                "event",
                serde_json::to_value(event).unwrap_or_default(),
            )
            .await;
            return;
        }

        // 先补一帧停车并清接管态/摇杆缓存，避免带着非零摇杆重建
        // （旧链已断时 `prepare_body_reconnect` 里还会再做一次，都是幂等的）
        if let Some(client) = zhpro.as_ref() {
            zhpro::teleop::reset_after_control_lost(client, teleop).await;
        }
        let event = DeviceEventPayload {
            event_type: "warn".to_string(),
            message: "收到「本体重连」：正在原地重建到本体的连接（约 5~15 秒）。\
                              客户端不退出、平台连接保持，期间设备会短暂显示「设备初始化中」"
                .to_string(),
            data: None,
        };
        send(
            out_tx,
            "event",
            serde_json::to_value(event).unwrap_or_default(),
        )
        .await;
        // 交给主循环：与「链路意外断开」走同一条重建路径（退避、接管态清理、
        // 任务置异常、窗口内失败后交 systemd 兜底）
        if body_reconnect.try_send(()).is_err() {
            warn!("本体重连请求未入队（已有一次重连在进行中）");
        }
        return;
    }

    // 「呼叫」/「停止呼叫」：任务详情页的人工喊话。只跟同机的板端 Agent 说话
    // （`127.0.0.1:8000`），不碰本体，所以放在本体门禁之前 —— 本体没连（关机 /
    // 断链）时现场依然要能喊话。失败只回 command_result，绝不抛错打断主循环。
    if matches!(command, "呼叫" | "停止呼叫") {
        // 服务端把附加参数包在 `params.params`（同「重新定位」），兼容平铺写法
        let outer = msg.payload.get("params");
        let params = outer.and_then(|p| p.get("params")).or(outer);
        let result: Result<String, String> = if command == "停止呼叫" {
            speak::manual_stop(&config.ai_agent_url, "平台手动停止").await
        } else {
            let text = params
                .and_then(|p| p.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let looping = params
                .and_then(|p| p.get("loop"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let seconds = params
                .and_then(|p| p.get("seconds"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            speak::manual_call(&config.ai_agent_url, text, looping, seconds).await
        };
        finish_command(out_tx, &msg, command, result).await;
        return;
    }

    // 「同步媒体」只动板子自己的磁盘与上行网络，不碰本体：本体没连（关机 /
    // 断链）也应该能把已经采好的巡检图传回中台，所以放在本体门禁之前。
    if command == "同步媒体" {
        let outer = msg.payload.get("params");
        // 进度上报：一轮同步在 4G 上可能跑几分钟，中台设备页要能看到
        // 「已上传 8/24」而不是干等 final 的 command_result。
        let progress_tx = out_tx.clone();
        let progress_request_id = command_request_id(&msg);
        let progress = move |p: media_sync::SyncProgress| {
            let Some(request_id) = progress_request_id.clone() else {
                return;
            };
            let _ = progress_tx.try_send(WsMessage {
                msg_type: "command_progress".to_string(),
                ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                payload: json!({
                    "requestId": request_id,
                    "command": "同步媒体",
                    "uploaded": p.uploaded,
                    "failed": p.failed,
                    "total": p.total,
                    "batchesDone": p.batches_done,
                    "batchesTotal": p.batches_total,
                }),
            });
        };
        let summary = media_sync::sync(outer, config, Some(&progress)).await;
        // 结构化结果比纯文案好判：中台要区分「目录已清空」与「还剩几张」，
        // 前端才能把「完成」和「部分完成 / 再点一次」分开显示。
        let result: Result<String, String> = match &summary {
            Ok(summary) => Ok(summary.to_string()),
            Err(error) => Err(error.clone()),
        };
        let extra = match &summary {
            Ok(summary) => json!({
                "drained": summary.drained(),
                "scanned": summary.scanned,
                "uploaded": summary.uploaded,
                "existing": summary.existing,
                "failed": summary.failed,
                "deleted": summary.deleted,
                "remaining": summary.remaining,
            }),
            Err(_) => json!({}),
        };
        send_command_result_with(out_tx, &msg, command, &result, extra).await;
        match result {
            Ok(note) => info!("指令 {command} 执行: {note}"),
            Err(error) => {
                warn!("指令 {command} 执行失败: {error}");
                let event = DeviceEventPayload {
                    event_type: "error".to_string(),
                    message: format!("指令 {command} 执行失败: {error}"),
                    data: None,
                };
                send(
                    out_tx,
                    "event",
                    serde_json::to_value(event).unwrap_or_default(),
                )
                .await;
            }
        }
        return;
    }

    // 「回传录像」：与「同步媒体」同一类长任务——只动板子自己的磁盘与上行网络，
    // 不碰本体，所以同样放在本体门禁之前。
    //
    // **录像不再自动回传**（任务终态 / 客户端启动都不传），只有这条指令会调：
    // 平台要哪一轮就点哪一轮，录像攒在板端 `RECORD_DIR/<计划>/`。
    //
    // 必须 spawn 出去：一场巡检的录像可能几百 MB，4G 上要跑几十分钟，绝不能把
    // 业务 WebSocket 的消息循环 await 死（后面的「暂停 / 取消 / 任务下发」会全堵住）。
    if command == "回传录像" {
        let recorder = recorder.clone();
        let task_out = out_tx.clone();
        let progress_tx = out_tx.clone();
        let progress_request_id = command_request_id(&msg);
        // 消息本身搬进后台任务：回执要按原 `requestId` / `msgId` 对回去
        let item = msg;
        tokio::spawn(async move {
            let progress = move |p: crate::record::SyncProgress| {
                let Some(request_id) = progress_request_id.clone() else {
                    return;
                };
                let _ = progress_tx.try_send(WsMessage {
                    msg_type: "command_progress".to_string(),
                    ts: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    payload: json!({
                        "requestId": request_id,
                        "command": "回传录像",
                        "uploaded": p.uploaded,
                        "failed": p.failed,
                        "total": p.total,
                        "remaining": p.remaining,
                    }),
                });
            };
            let summary = recorder.sync_all(Some(&progress)).await;
            let result: Result<String, String> = match &summary {
                Ok(summary) => Ok(summary.to_string()),
                Err(error) => Err(error.clone()),
            };
            let extra = match &summary {
                Ok(summary) => json!({
                    "drained": summary.drained(),
                    "nothingToDo": summary.nothing_to_do(),
                    "scanned": summary.scanned,
                    "uploaded": summary.uploaded,
                    "existing": summary.existing,
                    "failed": summary.failed,
                    "deleted": summary.deleted,
                    "remaining": summary.remaining,
                    "totalBytes": summary.total_bytes,
                }),
                Err(_) => json!({}),
            };
            send_command_result_with(&task_out, &item, "回传录像", &result, extra).await;
            match result {
                Ok(note) => info!("指令 回传录像 执行: {note}"),
                Err(error) => {
                    warn!("指令 回传录像 执行失败: {error}");
                    let event = DeviceEventPayload {
                        event_type: "error".to_string(),
                        message: format!("指令 回传录像 执行失败: {error}"),
                        data: None,
                    };
                    send(
                        &task_out,
                        "event",
                        serde_json::to_value(event).unwrap_or_default(),
                    )
                    .await;
                }
            }
        });
        return;
    }

    // 对齐官方 App 的 ready 门禁：恢复类动作（急停/解除急停/避障）
    // 永远放行；运动/任务类动作必须先过 sdk_node、急停、控制权、
    // 姿态检查，避免“平台显示已下发、本体直接拒收”。
    let action = msg.payload["params"]["action"]
        .as_str()
        .or_else(|| msg.payload["action"].as_str())
        .unwrap_or("");
    let recovery = matches!(command, "急停" | "解除急停" | "避障开启" | "避障关闭")
        || matches!(
            action,
            "emergency_stop" | "clear_emergency_stop" | "avoid_on" | "avoid_off"
        );
    let motion_command = matches!(
        command,
        "站立"
            | "卧倒"
            | "通用模式"
            | "原地模式"
            | "导航模式"
            | "开始巡检"
            | "暂停任务"
            | "继续任务"
            | "召回充电"
            | "重新定位"
            | "下发地图"
    ) || matches!(
        action,
        "stand_up" | "lie_down" | "generic_mode" | "in_place_mode" | "navigation_mode"
    );
    // 强制接管标记：遥控器关了但本体仍报 `ctrl_source = 3` 时，平台会带
    // `force=true` 下发「远程接管」。客户端据此只放行 ctrl_source 这一条
    // 门禁（见 [`RobotState::motion_block_reason_with`]），其余门禁不变。
    let force = msg.payload["force"]
        .as_bool()
        .or_else(|| msg.payload["params"]["force"].as_bool())
        .unwrap_or(false);
    match command {
        "远程接管" => {
            state.lock().await.teleop_force = force;
            if force {
                warn!("平台强制接管：暂时忽略 ctrl_source 抢占门禁（请确认遥控器已断电）");
            }
        }
        "释放接管" => state.lock().await.teleop_force = false,
        _ => {}
    }
    if !recovery && motion_command {
        let require_posture =
            matches!(
                command,
                "通用模式"
                    | "原地模式"
                    | "导航模式"
                    | "开始巡检"
                    | "暂停任务"
                    | "继续任务"
                    | "召回充电"
            ) || matches!(action, "generic_mode" | "in_place_mode" | "navigation_mode");
        // 强制接管只放行手动动作（站立/卧倒/模式切换），任务与导航类指令
        // 仍然要按 `ctrl_source = 3` 拒绝：本体在遥控器手里时导航能力不可用。
        let manual_action = matches!(
            command,
            "站立" | "卧倒" | "通用模式" | "原地模式" | "导航模式"
        ) || matches!(
            action,
            "stand_up" | "lie_down" | "generic_mode" | "in_place_mode" | "navigation_mode"
        );
        let command_block = {
            let guard = state.lock().await;
            let allow_preempt = guard.teleop_force && manual_action;
            guard.motion_block_reason_with(require_posture, allow_preempt)
        };
        if let Some(reason) = command_block {
            warn!("指令 {command} 被拒绝: {reason}");
            // 门禁拒绝也必须回执，否则平台只能等到超时才知道没执行
            send_command_result(out_tx, &msg, command, &Err(reason.clone())).await;
            let event = DeviceEventPayload {
                event_type: "error".to_string(),
                message: format!("指令 {command} 被拒绝: {reason}"),
                data: None,
            };
            send(
                out_tx,
                "event",
                serde_json::to_value(event).unwrap_or_default(),
            )
            .await;
            return;
        }
    }

    let Some(client) = zhpro.clone() else {
        warn!("未连接本体，忽略远程指令");
        send_command_result(
            out_tx,
            &msg,
            command,
            &Err("本体未连接（客户端正在重连或本体离线），指令未执行".to_string()),
        )
        .await;
        return;
    };
    // 遥控类指令（接管 / 释放 / 急停 / 本体动作）由 teleop 模块处理，
    // 其余是业务任务指令，仍走原来的路径
    let teleop_result =
        zhpro::teleop::handle_command(&client, teleop, command, Some(&msg.payload)).await;

    let result: Result<String, String> = match teleop_result {
        Some(result) => result,
        None => match command {
            // 暂停 / 继续必须用**真实的**本体任务 ID：固件不认 `task_id: "current"`，
            // 会回 13896 task not found（之前这两个按钮实际上什么都没做）。
            "暂停任务" | "继续任务" => {
                let control = if command == "暂停任务" {
                    Control::Pause
                } else {
                    Control::Resume
                };
                let runner = local_runner.lock().await.clone();
                match runner {
                    None => Err("本地没有正在执行的航线任务（可能未下发或客户端已重启）；\
                                 请在控制台重新下发任务"
                        .to_string()),
                    Some(runner) => match runner.control(control).await {
                        Ok(()) => Ok(if command == "暂停任务" {
                            "已请求暂停".to_string()
                        } else {
                            "已请求继续".to_string()
                        }),
                        Err(e) => Err(e),
                    },
                }
            }
            "召回充电" => {
                let map_id = state.lock().await.active_map.clone().unwrap_or_default();
                // 5014 同样在非白名单区间，没控制权会被网关丢弃
                if let Err(e) = client.ensure_control().await {
                    warn!("取 RoamerX 控制权失败（5014 可能被网关丢弃）: {e}");
                }
                client
                    .call(T_RETURN_TO_CHARGE, json!({ "map_id": map_id }))
                    .await
                    .map(|_| "已请求返回充电点".to_string())
            }
            "同步地图" => {
                // 服务端在指令里带上了台账当前园区归属（`params.params.parkId`），
                // 显式 null 表示设备确实没绑园区，不能拿本地缓存的旧值充数
                let outer = msg.payload.get("params");
                let params = outer.and_then(|p| p.get("params")).or(outer);
                let park_hint = params
                    .and_then(|p| p.get("parkId"))
                    .map(|value| value.as_i64());
                // 刷新语义由服务端 `mode` / `refresh` / `onlyMissing` 共同决定：
                // 中台按钮（manual）要求全量刷新，auto / onlyMissing 只补缺。
                let mode = params
                    .and_then(|p| p.get("mode"))
                    .and_then(Value::as_str)
                    .unwrap_or("manual");
                let refresh = params
                    .and_then(|p| p.get("refresh"))
                    .and_then(Value::as_bool)
                    .unwrap_or_else(|| {
                        let only_missing = params
                            .and_then(|p| p.get("onlyMissing"))
                            .and_then(Value::as_bool);
                        only_missing.map(|only| !only).unwrap_or(mode != "auto")
                    });
                sync_maps_from_robot(config, &client, out_tx, park_hint, refresh, true).await
            }
            "下发地图" => {
                let result =
                    push_map_to_body(config, &client, state, local_runner, local_mapping, &msg)
                        .await;
                // 成功也要留一条设备事件：中台审计只知道「已下发」，
                // 真正写本体 + 重新载图的结果要看这里。
                if let Ok(note) = &result {
                    let event = DeviceEventPayload {
                        event_type: "info".to_string(),
                        message: note.clone(),
                        data: None,
                    };
                    send(
                        out_tx,
                        "event",
                        serde_json::to_value(event).unwrap_or_default(),
                    )
                    .await;
                }
                result
            }
            "重新定位" => {
                // 服务端把指令附加参数包在 `params.params`（见 server 的 push_command），
                // 这里兼容直接平铺在 `params` 上的写法
                let outer = msg.payload.get("params");
                let params = outer.and_then(|p| p.get("params")).or(outer);
                let map_id = params
                    .and_then(|p| p.get("mapId"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or(state.lock().await.active_map.clone());
                let pose = parse_initial_pose(params.and_then(|p| p.get("pose")));
                match map_id {
                    None => Err(
                        "未指定 mapId，且机器人当前没有已载入地图；请在设备上指定地图后重试"
                            .to_string(),
                    ),
                    Some(map_id) => {
                        let map_dir_hint = params
                            .and_then(|p| p.get("mapDir"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        // 4030 的 map_path 必须是本体上真实存在的目录
                        let manager = MapManager::new(client.clone());
                        match manager.resolve(&[&map_dir_hint, &map_id]).await {
                            Err(e) => Err(format!("重定位失败：{e}")),
                            Ok(robot_map) => {
                                let map_path = robot_map.path.clone();
                                info!(
                                    "重新定位：map={map_id} 目录={} 路径={map_path} 初始位姿={}",
                                    robot_map.map_id,
                                    if pose.is_some() {
                                        "平台指定"
                                    } else {
                                        "用上次位姿文件"
                                    }
                                );
                                match zhpro::mapping::relocalize(
                                    &client,
                                    &map_path,
                                    pose,
                                    LOCATE_TIMEOUT,
                                )
                                .await
                                {
                                    Ok(status) => {
                                        state.lock().await.active_map = Some(map_id.clone());
                                        // 重定位完就切导航模式：否则后续 5000 会被回
                                        // 13897 capability busy（本体仍在手动模式）。
                                        if let Err(e) =
                                            zhpro::teleop::enter_navigation_mode(&client).await
                                        {
                                            warn!("重定位后切导航模式失败: {e}");
                                        }
                                        Ok(format!("重定位完成：{map_id}（status={status}）"))
                                    }
                                    Err(e) => Err(format!("重定位失败：{e}")),
                                }
                            }
                        }
                    }
                }
            }
            other => Err(format!("未知指令 {other}")),
        },
    };
    // 先把真实结果回报给业务平台（按 requestId 关联），再落本地日志/事件。
    send_command_result(out_tx, &msg, command, &result).await;
    match result {
        Ok(note) => info!("指令 {command} 执行: {note}"),
        Err(e) => {
            warn!("指令 {command} 执行失败: {e}");
            let event = DeviceEventPayload {
                event_type: "error".to_string(),
                message: format!("指令 {command} 执行失败: {e}"),
                data: None,
            };
            send(
                out_tx,
                "event",
                serde_json::to_value(event).unwrap_or_default(),
            )
            .await;
        }
    }
}

/// 指令结果的统一收尾：先按 `requestId` 回执（平台据此把审计从 pending 推进到结论），
/// 再落一条本地日志；失败额外发一条设备事件，控制台才看得到「点了没反应」的原因。
///
/// 「呼叫 / 电话」这类**不碰本体**的指令直接用它，避免每个分支抄一遍这段。
async fn finish_command(
    out_tx: &mpsc::Sender<WsMessage>,
    msg: &WsMessage,
    command: &str,
    result: Result<String, String>,
) {
    send_command_result(out_tx, msg, command, &result).await;
    match result {
        Ok(note) => info!("指令 {command} 执行: {note}"),
        Err(error) => {
            warn!("指令 {command} 执行失败: {error}");
            let event = DeviceEventPayload {
                event_type: "error".to_string(),
                message: format!("指令 {command} 执行失败: {error}"),
                data: None,
            };
            send(
                out_tx,
                "event",
                serde_json::to_value(event).unwrap_or_default(),
            )
            .await;
        }
    }
}
