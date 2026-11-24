//! 业务服务下行消息处理：按消息类型分发到各业务分支。

use crate::agent_relay;
use crate::ai_patrol::AiPatrol;
use crate::ai_patrol::AiStart;
use crate::lights::PointLights;
use crate::record::Recorder;
use crate::report::clear_teleop_blocked;
use crate::report::notify_teleop_blocked;
use crate::speak::Speaker;
use crate::state::ActiveTask;
use crate::state::RobotState;
use crate::state::Shared;
use crate::wire::WsMessage;
use crate::zhpro;
use crate::zhpro::client::ZhProClient;
use crate::zhpro::mapping::MappingSession;
use crate::zhpro::route::RouteRunner;
use agent_relay::AgentRelay;
use robodog_device_client::config::Config;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::error;
use tracing::info;
use tracing::warn;
use zhpro::teleop::SharedTeleop;

mod command;
mod control;
mod task;

/// 处理业务服务下发消息
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_server_message(
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
    ai: &Arc<AiPatrol>,
    lights: &Arc<PointLights>,
    recorder: &Arc<Recorder>,
    speaker: &Arc<Speaker>,
    relay: &Arc<AgentRelay>,
) {
    match msg.msg_type.as_str() {
        "register_ack" => {
            if msg.payload["success"].as_bool().unwrap_or(false) {
                // 服务端在注册回执里下发设备绑定的园区；没绑也能用，只是地图会先不归属园区
                let park_id = msg.payload["parkId"].as_i64();
                *config.bound_park_id.lock().await = park_id;
                match park_id {
                    Some(park_id) => info!("设备注册成功: {}（园区 {park_id}）", config.device_id),
                    None => info!(
                        "设备注册成功: {}（未绑定园区：地图会先入库为设备地图，创建园区时再挑图绑定）",
                        config.device_id
                    ),
                }
                // 注册成功后立刻打个招呼：本机 Agent 真的连得上，平台才会把背包页的
                // 请求（模型目录 / 运行态 / AI 启停）转到这条连接上
                relay.announce(out_tx).await;
            } else {
                error!(
                    "设备注册失败: {}",
                    msg.payload["message"].as_str().unwrap_or("未知错误")
                );
            }
        }

        // 心跳回执：服务端在这里同步「强制接管」标记（客户端重启后本地标记会丢，
        // 不同步的话摇杆又会被 `ctrl_source = 3` 门禁拦下）
        "heartbeat_ack" => {
            if let Some(force) = msg.payload["teleopForce"].as_bool() {
                let mut guard = state.lock().await;
                if guard.teleop_force != force {
                    guard.teleop_force = force;
                    if force {
                        warn!("平台处于强制接管态：暂时忽略 ctrl_source 抢占门禁");
                    } else {
                        info!("平台已退出强制接管态：恢复遥控器抢占检测");
                    }
                }
            }
        }

        // 航点 AI 识别：平台已经按路线里的动作算好场景/类别/阈值/窗口秒数，
        // 这里只管在本机启停板端 Agent 并回传检出（服务端够不到板端 Agent）
        "ai_start" => match AiStart::from_value(&msg.payload) {
            Some(start) => ai.start(out_tx, start).await,
            None => warn!("ai_start 载荷缺少 windowId，忽略: {}", msg.payload),
        },

        // 背包页的请求（模型目录 / 运行态 / AI 启停）：本机代调 Agent 后回 agent_result
        "agent_request" => relay.on_request(out_tx, &msg.payload).await,

        // 平台关窗（任务终态 / 暂停 / 取消）：窗口任务自己停 Agent 并回报 ai_end
        "ai_stop" => {
            let reason = msg.payload["reason"].as_str().unwrap_or("平台关闭窗口");
            ai.stop(reason).await;
        }

        // 证据图已经进了企业微信群：删掉板端这一轮的存证
        // （`detect_jobs/<计划>/pt<航点>/` 里的画框 JPG + result.json）
        "ai_cleanup" => {
            let plan_id = msg.payload["planId"].as_str().unwrap_or("");
            let point_index = msg.payload["pointIndex"].as_i64().unwrap_or(-1);
            let reason = msg.payload["reason"].as_str().unwrap_or("平台已推送证据图");
            ai.cleanup_evidence(plan_id, point_index, reason).await;
        }

        // 事件回执：客户端发的 event 由服务端落库后回 ack，不需要额外处理
        "event_ack" => {}

        // 指令结果回执：业务平台确认收到 `command_result`，不需要额外处理
        "command_result_ack" => {}

        // 进度上报的逐条回执：客户端已经在本地合流上报，不需要逐条处理，
        // 也不能当「未处理的消息类型」打 WARN（任务执行时每 5s 一条会刷爆日志）
        "task_progress_ack" => {}

        "park_binding" => {
            // 控制台绑定 / 解绑园区后实时下发，无需重连
            let park_id = msg.payload["parkId"].as_i64();
            *config.bound_park_id.lock().await = park_id;
            match park_id {
                Some(park_id) => info!(
                    "园区绑定已更新: parkId={park_id} {}",
                    msg.payload["parkName"].as_str().unwrap_or("")
                ),
                None => warn!("园区绑定已解除，地图会先入库为设备地图，创建园区时再挑图绑定"),
            }
        }

        "task_dispatch" => {
            task::dispatch(
                config,
                msg,
                out_tx,
                state,
                active,
                zhpro,
                local_runner,
                local_mapping,
                ai,
                lights,
                recorder,
                speaker,
            )
            .await
        }

        "task_control" => {
            control::apply(
                msg,
                out_tx,
                state,
                active,
                local_runner,
                local_mapping,
                ai,
                lights,
                recorder,
                speaker,
            )
            .await
        }

        "command" => {
            command::handle(
                config,
                msg,
                out_tx,
                state,
                active,
                zhpro,
                local_runner,
                local_mapping,
                teleop,
                body_reconnect,
            )
            .await
        }

        // 本体链路断开前客户端会上报一帧 `teleop_state`（holder=false），服务端回这个 ack；
        // 只是确认收到，不需要处理
        "teleop_state_ack" => {}

        "teleop" => {
            let Some(client) = zhpro.clone() else {
                return;
            };
            // 官方 App 的 JoystickOutboundGate：只有 ready + RoamerX 才放 1304。
            // 不满足时先补一帧停车，避免最后一帧非零摇杆继续生效。
            let teleop_block = {
                let guard = state.lock().await;
                let allow_preempt = guard.teleop_force;
                guard.motion_block_reason_with(true, allow_preempt)
            };
            if let Some(reason) = teleop_block {
                zhpro::teleop::send_stop(&client).await;
                notify_teleop_blocked(state, out_tx, &reason).await;
                return;
            }
            // 门禁通过：清掉上次拒绝原因，下一次拒绝立刻重新上报
            clear_teleop_blocked(state).await;
            if let Err(e) = zhpro::teleop::handle_teleop(&client, teleop, &msg.payload).await {
                warn!("遥控摇杆处理失败: {e}");
                notify_teleop_blocked(state, out_tx, &format!("接管失败：{e}")).await;
            }
        }

        "error" => {
            error!(
                "业务服务返回错误: {}",
                msg.payload["message"].as_str().unwrap_or("未知错误")
            );
        }

        // 服务端链路保活探针：WS 层已用 Ping/Pong 维持连接，这里忽略载荷
        "ping" => {}

        other => warn!("未处理的消息类型: {other}"),
    }
}
