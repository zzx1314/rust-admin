//! 任务控制：暂停 / 继续 / 取消。

use crate::ai_patrol::AiPatrol;
use crate::lights;
use crate::report::report_progress;
use crate::state::ActiveTask;
use crate::state::RobotState;
use crate::state::Shared;
use crate::state::clear_active_task;
use crate::state::clear_active_task_if;
use crate::wire::WsMessage;
use crate::zhpro;
use crate::zhpro::mapping::MappingSession;
use crate::zhpro::route::RouteRunner;
use lights::PointLights;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::info;
use tracing::warn;
use zhpro::route::Control;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply(
    msg: WsMessage,
    out_tx: &mpsc::Sender<WsMessage>,
    state: &Shared<RobotState>,
    active: &Shared<Option<ActiveTask>>,
    local_runner: &Shared<Option<Arc<RouteRunner>>>,
    local_mapping: &Shared<Option<Arc<MappingSession>>>,
    ai: &Arc<AiPatrol>,
    lights: &Arc<PointLights>,
) {
    let action = msg.payload["action"].as_str().unwrap_or("unknown");
    let task_code = msg.payload["taskCode"].as_str().unwrap_or("");
    let reason = msg.payload["reason"].as_str().unwrap_or("");
    info!(
        "任务控制: {task_code} → {action}{}",
        if reason.is_empty() {
            String::new()
        } else {
            format!("（{reason}）")
        }
    );
    let control = match action {
        "pause" => Some(Control::Pause),
        "resume" => Some(Control::Resume),
        "cancel" => Some(Control::Cancel),
        _ => None,
    };
    let Some(control) = control else {
        warn!("未知控制指令: {action}");
        return;
    };
    // resume 只对「本地正在跑的任务」有意义：平台用「执行中」触发 resume 时，
    // 如果本地没有对应任务的执行上下文（客户端重启过 / 平台只发了 resume
    // 没发 task_dispatch），回一条假的「执行中」会让平台任务永久卡住。
    if action == "resume" {
        let has_context = active
            .lock()
            .await
            .as_ref()
            .map(|task| task.task_code == task_code)
            .unwrap_or(false)
            || local_runner.lock().await.is_some()
            || local_mapping.lock().await.is_some();
        if !has_context {
            warn!("任务 {task_code} 本地没有执行上下文，拒绝 resume");
            report_progress(
                out_tx,
                task_code,
                "异常",
                0,
                Some(
                    "本地没有该任务的执行上下文（任务未下发或客户端已重启），请从控制台重新下发"
                        .to_string(),
                ),
                None,
                None,
                None,
            )
            .await;
            return;
        }
    }
    if let Some(runner) = local_runner.lock().await.clone()
        && let Err(e) = runner.control(control).await
    {
        warn!("任务控制失败: {e}");
    }
    // 建图任务的取消走建图会话
    if action == "cancel" {
        if let Some(session) = local_mapping.lock().await.take() {
            let saved = session.stop(Duration::from_secs(30)).await;
            match saved {
                Some(s) if s.success => {
                    info!("建图已停止并保存成功");
                }
                Some(s) => warn!("建图停止，保存失败: {}", s.message),
                None => warn!("建图停止，未收到 4001"),
            }
        }
        // 任务取消：识别窗口没有继续的意义（Agent 单路，别占着）
        ai.stop("任务已取消").await;
        // 灯光也是：取消后狗不再跑这条路线，灯必须当场收掉
        lights.leave("任务已取消").await;
        // 取消也是计划收尾：板端整个计划目录都不需要了（理由同终态那条）
        ai.finish_plan(task_code, "任务已取消").await;
        // 只清自己那条：平台可能「先取消旧任务、再下发新任务」，取消的收尾
        // 晚到时把新任务的上下文清掉，平台就再也收不到这条新任务的进度了。
        if task_code.is_empty() {
            clear_active_task(active, state).await;
        } else {
            clear_active_task_if(active, state, task_code).await;
        }
    }
    let status = match control {
        Control::Pause => "已暂停",
        Control::Resume => "执行中",
        Control::Cancel => "异常",
    };
    report_progress(
        out_tx,
        task_code,
        status,
        0,
        Some(format!("任务控制: {action}")),
        None,
        None,
        None,
    )
    .await;
}
