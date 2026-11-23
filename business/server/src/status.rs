//! 计划执行状态（`tasks.task_status`）的取值与判断。
//!
//! 这些是**落库的中文字面量，也是前端契约**，所以保持字符串不变；
//! 但「有效集合」和「是否终态」以前在三处各抄了一份，并且已经抄歪了：
//!
//! - `gateway::handle_task_progress` 一份五元组（不认识的当「执行中」）；
//! - `handlers::tasks::update_task_status` 同一份五元组（不认识的报 400）；
//! - `ai_patrol::is_terminal_status` 另一份，多一个 `已取消`。
//!
//! 于是「客户端上报已取消」会被网关改写成「执行中」，而 AI 识别窗口
//! 却认为它已经结束。这里收成唯一一份，新增状态只改这个文件。
//!
//! 注意：这**不是**发布状态（`草稿` / `已发布`，见 `schedule::DRAFT` /
//! `schedule::PUBLISHED`）—— 发布状态决定要不要自动调度，执行状态只描述进度。

/// 已建单，等待下发 / 等待时间窗
pub const PENDING: &str = "待执行";
/// 已下发，本体正在跑
pub const RUNNING: &str = "执行中";
pub const DONE: &str = "已完成";
pub const ABNORMAL: &str = "异常";
/// 暂停（本体支持续跑）：客户端可能直接 resume，所以它算「终态」但不算「完成」
pub const PAUSED: &str = "已暂停";
/// 取消：只有设备客户端会报上来，网页端不提供这个操作
pub const CANCELLED: &str = "已取消";

/// 允许写入 / 允许客户端上报的执行状态。
///
/// `已取消` 不在这里：页面没有这个操作，只作为客户端上报的既有事实保存在
/// [`TERMINAL`] 里（历史数据里已经有这个值）。
pub const ALL: [&str; 5] = [PENDING, RUNNING, DONE, ABNORMAL, PAUSED];

/// 任务已经结束、不会再推进进度。
///
/// `已暂停` 也算：暂停期间 AI 识别窗口要关掉、任务看门狗不该再判它「停滞」。
pub const TERMINAL: [&str; 4] = [DONE, ABNORMAL, PAUSED, CANCELLED];

/// 是可以落库 / 可以上报的执行状态吗？
#[must_use]
pub fn is_valid(status: &str) -> bool {
    ALL.contains(&status)
}

/// 任务是否已经结束（终态）。
#[must_use]
pub fn is_terminal(status: &str) -> bool {
    TERMINAL.contains(&status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_writable_status_is_known() {
        for status in ALL {
            assert!(is_valid(status), "{status} 应该在 ALL 里");
        }
        assert!(!is_valid("跑完了"));
        assert!(!is_valid(""));
    }

    /// 暂停/取消都是终态：AI 窗口与看门狗都靠这个判断收口。
    #[test]
    fn terminal_covers_everything_that_stops_progress() {
        for status in TERMINAL {
            assert!(is_terminal(status), "{status} 应该是终态");
        }
        assert!(!is_terminal(PENDING));
        assert!(!is_terminal(RUNNING));
    }

    /// 客户端会上报 `已取消`（网页端不提供），所以它必须在终态里、不在 ALL 里。
    #[test]
    fn cancelled_is_reported_by_the_client_only() {
        assert!(is_terminal(CANCELLED));
        assert!(!is_valid(CANCELLED));
    }
}
