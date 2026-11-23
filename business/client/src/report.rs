//! 上报合流与限频：事件去重、进度降频、摇杆门禁提示。

use crate::state::RobotState;
use crate::state::Shared;
use crate::state::TELEOP_BLOCK_NOTICE_INTERVAL;
use crate::wire::DeviceEventPayload;
use crate::wire::TaskProgressPayload;
use crate::wire::WsMessage;
use crate::wire::send;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use super::*;

pub(crate) fn progress_pct(done: usize, total: usize) -> i64 {
    if total == 0 {
        return 0;
    }
    ((done * 100) / total) as i64
}

/// 本体故障 / 诊断 / 充电结果都是**周期性推送**（实测 4021 每 5s 一条、1317 每秒一条），
/// 内容不变时只上报一次，否则会在服务端刷出成百上千条 task_events / alerts。
#[derive(Default)]
pub(crate) struct EventDedup {
    pub(crate) seen: HashMap<String, Instant>,
}

impl EventDedup {
    /// 窗口期内出现过同样的 `event_type + message` 就丢弃
    pub(crate) fn accept(&mut self, event_type: &str, message: &str) -> bool {
        const WINDOW: Duration = Duration::from_secs(300);
        let now = Instant::now();
        self.seen.retain(|_, at| now.duration_since(*at) < WINDOW);
        let key = format!("{event_type}|{message}");
        if self.seen.contains_key(&key) {
            return false;
        }
        self.seen.insert(key, now);
        true
    }
}

/// 去重后上报一条设备事件
pub(crate) async fn send_deduped_event(
    out_tx: &mpsc::Sender<WsMessage>,
    dedup: &Arc<Mutex<EventDedup>>,
    event: DeviceEventPayload,
) {
    if !dedup.lock().await.accept(&event.event_type, &event.message) {
        debug!("跳过重复事件: {} / {}", event.event_type, event.message);
        return;
    }
    send(
        out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
}

/// 进度内容没变时的上报窗口
///
/// 必须明显小于服务端 `task_timeout_secs`（默认 120s）：看门狗靠 `tasks.updated_at`
/// 判「执行中但不上报」，彻底停发会把正在跑的任务翻成异常。
pub(crate) const PROGRESS_DEDUP_WINDOW: Duration = Duration::from_secs(30);

/// `task_progress` 上报器：去重 + 打印整份载荷。
///
/// 两个消费路径（5s 定时器、5009 快照）内容不变时都会重复产生同一份 payload：
/// - 不能原样刷：暂停一小时能刷出几千条一样的进度，服务端事件流也会被冲掉；
/// - 也不能彻底不发：服务端看门狗按 `tasks.updated_at` 判 120s 超时。
///
/// 所以按「整份 JSON 不变就 [`PROGRESS_DEDUP_WINDOW`] 一次」处理，
/// 并且只要真的发出去就把载荷打到 `info`，不用开 debug 就能对账。
#[derive(Default)]
pub(crate) struct ProgressReporter {
    pub(crate) last: Option<(String, Instant)>,
}

impl ProgressReporter {
    pub(crate) async fn report(
        &mut self,
        out_tx: &mpsc::Sender<WsMessage>,
        payload: &TaskProgressPayload,
        source: &str,
    ) {
        let value = serde_json::to_value(payload).unwrap_or_default();
        let key = value.to_string();
        if let Some((previous, at)) = self.last.as_ref()
            && previous == &key
            && at.elapsed() < PROGRESS_DEDUP_WINDOW
        {
            debug!("task_progress({source}) 内容未变，不上报: {key}");
            return;
        }
        info!("→ 上报 task_progress({source}): {key}");
        self.last = Some((key, Instant::now()));
        send(out_tx, "task_progress", value).await;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn report_progress(
    out_tx: &mpsc::Sender<WsMessage>,
    task_code: &str,
    status: &str,
    progress: i64,
    message: Option<String>,
    stage: Option<String>,
    robot_task_id: Option<String>,
    total_points: Option<i64>,
) {
    let payload = TaskProgressPayload {
        task_code: task_code.to_string(),
        status: status.to_string(),
        progress,
        point_index: None,
        waypoint_index: None,
        message,
        stage,
        robot_task_id,
        total_points,
    };
    // 一次性上报（定位未就绪 / 执行失败 / 航点跑完）：没有历史可比，直接发并打日志
    ProgressReporter::default()
        .report(out_tx, &payload, "事件")
        .await;
}

/// 摇杆未放行（门禁拒绝 / 接管失败）：按「原因变化或 30s」限频上报一条 `warn` 事件
///
/// 以前只打 `debug!`，平台侧完全看不到原因，现场只能看到「页面显示已接管、狗子不动」。
/// 用 `warn` 而不是 `error`：门禁拒绝不是本体故障，服务端不应因此生成告警工单。
pub(crate) async fn notify_teleop_blocked(
    state: &Shared<RobotState>,
    out_tx: &mpsc::Sender<WsMessage>,
    reason: &str,
) {
    let changed = {
        let mut guard = state.lock().await;
        let changed = guard
            .teleop_block_notice
            .as_ref()
            .map(|(last, at)| last != reason || at.elapsed() > TELEOP_BLOCK_NOTICE_INTERVAL)
            .unwrap_or(true);
        if changed {
            guard.teleop_block_notice = Some((reason.to_string(), Instant::now()));
        }
        changed
    };
    if !changed {
        debug!("摇杆未放行（同原因已在窗口内上报）: {reason}");
        return;
    }
    warn!("摇杆未放行: {reason}");
    let event = DeviceEventPayload {
        event_type: "warn".to_string(),
        message: format!("摇杆被拒绝: {reason}"),
        data: Some(json!({ "blocked": true })),
    };
    send(
        out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
}

/// 摇杆重新放行：清掉上次的拒绝原因
pub(crate) async fn clear_teleop_blocked(state: &Shared<RobotState>) {
    state.lock().await.teleop_block_notice = None;
}

#[cfg(test)]
mod event_dedup_tests {
    use super::EventDedup;

    #[test]
    fn repeated_fault_is_reported_once() {
        let mut dedup = EventDedup::default();
        assert!(dedup.accept("fault", "感知障碍物检测延迟"));
        // 本体 4021 周期性重推同一条故障：不再上报
        assert!(!dedup.accept("fault", "感知障碍物检测延迟"));
        // 换一条故障照常上报
        assert!(dedup.accept("fault", "电池通信异常"));
        // 事件类型不同不互相压制
        assert!(dedup.accept("info", "感知障碍物检测延迟"));
    }
}

#[cfg(test)]
mod progress_reporter_tests {
    use super::{ProgressReporter, TaskProgressPayload};

    fn payload(status: &str, stage: &str) -> TaskProgressPayload {
        TaskProgressPayload {
            task_code: "TK-1".to_string(),
            status: status.to_string(),
            progress: 40,
            point_index: Some(2),
            waypoint_index: Some(1),
            message: Some("已完成 2/5 个航点".to_string()),
            stage: Some(stage.to_string()),
            robot_task_id: Some("nav-1".to_string()),
            total_points: Some(5),
        }
    }

    /// 5009 是 500ms 一帧，同一份内容不能每次都往业务库推
    #[tokio::test]
    async fn identical_payload_is_suppressed() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut reporter = ProgressReporter::default();
        reporter
            .report(&tx, &payload("执行中", "navigation.navigating"), "测试")
            .await;
        reporter
            .report(&tx, &payload("执行中", "navigation.navigating"), "测试")
            .await;

        let first = rx.try_recv().expect("首次必须上报");
        assert_eq!(first.msg_type, "task_progress");
        assert!(rx.try_recv().is_err(), "内容完全相同的重复帧不该再上报");
    }

    /// 阶段一变（或终态）必须立刻下发，不能被 30s 窗口压住
    #[tokio::test]
    async fn changed_payload_is_sent_immediately() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let mut reporter = ProgressReporter::default();
        reporter
            .report(&tx, &payload("执行中", "navigation.navigating"), "测试")
            .await;
        reporter
            .report(&tx, &payload("已完成", "navigation.succeeded"), "测试")
            .await;
        reporter
            .report(&tx, &payload("执行中", "navigation.navigating"), "测试")
            .await;

        assert!(rx.try_recv().is_ok());
        let terminal = rx.try_recv().expect("终态必须立刻上报");
        assert_eq!(terminal.payload["status"], "已完成");
        // 窗口只比对「紧邻的上一份」：内容换回去也得发，不能把某个状态永久封死
        let back = rx.try_recv().expect("内容变化必须立刻上报");
        assert_eq!(back.payload["status"], "执行中");
        assert!(rx.try_recv().is_err());
    }
}
