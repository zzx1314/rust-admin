//! 5009 任务状态快照（V2）解析与本地状态维护。
//!
//! 规则（v1.0.3 §2.6.10）：
//! - 每帧整体替换，不跨帧拼接；
//! - 同 `boot_id` 内按 `snapshot_seq` 丢弃重复/更旧帧，`boot_id` 变化时清空；
//! - 运行判定只看 `current_task.lifecycle`，业务阶段看 `stage`；
//! - 离散事件用 `boot_id + task_id + last_event.seq` 去重。

use super::protocol::lifecycle_is_terminal;
use serde_json::Value;

/// 任务快照
#[derive(Debug, Clone, Default)]
pub struct TaskSnapshot {
    pub task_id: String,
    pub name: String,
    pub task_type: String,
    pub lifecycle: String,
    pub stage: String,
    pub progress: Option<f64>,
    pub last_event_seq: i64,
    pub last_event_type: String,
    pub stop_reason: Option<String>,
    pub error_message: Option<String>,
    pub detail_index: Option<i64>,
    pub detail_total: Option<i64>,
}

impl TaskSnapshot {
    pub fn from_value(value: &Value) -> Option<Self> {
        let task_id = value.get("task_id").and_then(Value::as_str)?;
        if task_id.is_empty() {
            return None;
        }
        let last_event = value.get("last_event");
        Some(Self {
            task_id: task_id.to_string(),
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            task_type: value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            lifecycle: value
                .get("lifecycle")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            stage: value
                .get("stage")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            progress: value.get("progress").and_then(Value::as_f64),
            last_event_seq: last_event
                .and_then(|e| e.get("seq"))
                .and_then(Value::as_i64)
                .unwrap_or(0),
            last_event_type: last_event
                .and_then(|e| e.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            stop_reason: value
                .get("stop_reason")
                .and_then(|r| r.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string),
            error_message: value
                .get("error")
                .and_then(|r| r.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string),
            detail_index: find_index(value),
            detail_total: find_total(value),
        })
    }

    pub fn is_terminal(&self) -> bool {
        lifecycle_is_terminal(&self.lifecycle)
    }
}

/// 从 `detail.last_arrived_point` / `last_arrived_goal` 或 `last_event.data` 提取点位序号
fn find_index(value: &Value) -> Option<i64> {
    for key in ["last_arrived_point", "last_arrived_goal"] {
        if let Some(idx) = value
            .get("detail")
            .and_then(|d| d.get(key))
            .and_then(|p| p.get("index"))
            .and_then(Value::as_i64)
        {
            return Some(idx);
        }
    }
    value
        .get("last_event")
        .and_then(|e| e.get("data"))
        .and_then(|d| d.get("index"))
        .and_then(Value::as_i64)
}

fn find_total(value: &Value) -> Option<i64> {
    for key in ["last_arrived_point", "last_arrived_goal"] {
        if let Some(total) = value
            .get("detail")
            .and_then(|d| d.get(key))
            .and_then(|p| p.get("total"))
            .and_then(Value::as_i64)
        {
            return Some(total);
        }
    }
    value
        .get("last_event")
        .and_then(|e| e.get("data"))
        .and_then(|d| d.get("total"))
        .and_then(Value::as_i64)
}

/// 一帧 5009（已拆解出关心部分）
#[derive(Debug, Clone, Default)]
pub struct StatusFrame {
    pub boot_id: String,
    pub snapshot_seq: i64,
    pub revision: i64,
    pub reason: String,
    pub robot_task_state: String,
    pub loc_status: i64,
    pub loc_fresh: bool,
    pub current: Option<TaskSnapshot>,
    pub pending: Option<TaskSnapshot>,
    pub terminals: Vec<TaskSnapshot>,
}

impl StatusFrame {
    pub fn parse(data: &Value) -> Option<Self> {
        let schema = data
            .get("schema_version")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if schema != 2 {
            return None;
        }
        let robot = data.get("robot");
        let loc = robot.and_then(|r| r.get("loc_state"));
        let terminals = data
            .get("recent_terminal_tasks")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(TaskSnapshot::from_value)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Some(Self {
            boot_id: data
                .get("boot_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            snapshot_seq: data
                .get("snapshot_seq")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            revision: data
                .get("revision")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            reason: data
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            robot_task_state: robot
                .and_then(|r| r.get("task_state"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            loc_status: loc
                .and_then(|l| l.get("status"))
                .and_then(Value::as_i64)
                .unwrap_or(255),
            loc_fresh: loc
                .and_then(|l| l.get("fresh"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            current: data.get("current_task").and_then(TaskSnapshot::from_value),
            pending: data.get("pending_task").and_then(TaskSnapshot::from_value),
            terminals,
        })
    }

    /// 在 current / pending / 最近终态中按 task_id 查找任务。
    /// 终态被回收后仍会在 `recent_terminal_tasks` 中短暂存在（最多 3 条、约 30s）。
    pub fn task_for(&self, task_id: &str) -> Option<&TaskSnapshot> {
        self.current
            .as_ref()
            .filter(|t| t.task_id == task_id)
            .or_else(|| self.pending.as_ref().filter(|t| t.task_id == task_id))
            .or_else(|| self.terminals.iter().find(|t| t.task_id == task_id))
    }
}

/// 单帧丢帧器：同一 `boot_id` 内按 `snapshot_seq` 丢弃重复 / 更旧的快照。
///
/// `TaskMonitor` 持有跨帧状态（事件游标、终态幂等），只适合长期存活的消费循环；
/// 「等某个航点 / 等定位收敛」这类一次性等待没有地方存跨调用状态，用这个轻量版。
#[derive(Debug, Clone)]
pub struct SeqGate {
    boot_id: String,
    last_seq: i64,
}

impl Default for SeqGate {
    fn default() -> Self {
        Self {
            boot_id: String::new(),
            last_seq: -1,
        }
    }
}

/// `SeqGate::accept` 的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateResult {
    /// 是否采用这一帧（重复 / 更旧帧为 false）
    pub accepted: bool,
    /// `boot_id` 是否变化（算法进程重启，调用方应作废旧任务视图）
    pub boot_changed: bool,
}

impl SeqGate {
    pub fn accept(&mut self, frame: &StatusFrame) -> GateResult {
        let boot_changed = frame.boot_id != self.boot_id;
        if boot_changed {
            // 算法进程重启：新 boot_id 下的 seq 重新计数，游标必须重置
            self.boot_id.clone_from(&frame.boot_id);
            self.last_seq = -1;
        }
        if frame.snapshot_seq <= self.last_seq {
            return GateResult {
                accepted: false,
                boot_changed,
            };
        }
        self.last_seq = frame.snapshot_seq;
        GateResult {
            accepted: true,
            boot_changed,
        }
    }
}

/// `TaskMonitor::accept` 通过校验的帧
#[derive(Debug, Clone, Copy)]
pub struct Accepted<'a> {
    pub frame: &'a StatusFrame,
    /// `boot_id` 变了：调用方要清空本地任务快照（文档 §2.6.10 第 2 条）
    pub boot_changed: bool,
}

/// 本地维护的机器人任务视图，负责丢帧与事件去重
pub struct TaskMonitor {
    gate: SeqGate,
    last_revision: i64,
    /// (task_id, event_seq) 已消费的离散事件
    seen_events: Vec<(String, i64)>,
    /// (task_id, lifecycle) 已上报过的终态
    ///
    /// 同一条终态可能先出现在 `current_task`，回收后再出现在
    /// `recent_terminal_tasks`，两处都要识别、但不能重复上报。
    reported_terminals: Vec<(String, String)>,
}

impl Default for TaskMonitor {
    fn default() -> Self {
        Self {
            gate: SeqGate::default(),
            last_revision: -1,
            seen_events: Vec::new(),
            reported_terminals: Vec::new(),
        }
    }
}

impl TaskMonitor {
    /// 接受一帧，返回需要处理的帧；重复或更旧的帧返回 None
    pub fn accept<'a>(&mut self, frame: &'a StatusFrame) -> Option<Accepted<'a>> {
        let gate = self.gate.accept(frame);
        if gate.boot_changed {
            // 算法进程重启：事件游标、终态去重全部作废
            self.last_revision = -1;
            self.seen_events.clear();
            self.reported_terminals.clear();
        }
        if !gate.accepted {
            return None;
        }
        self.last_revision = frame.revision;
        Some(Accepted {
            frame,
            boot_changed: gate.boot_changed,
        })
    }

    /// 终态上报幂等：同一任务的同一 lifecycle 只放行一次。
    ///
    /// `current_task` 与 `recent_terminal_tasks` 会给出同一条终态，必须按
    /// `task_id + lifecycle` 去重（文档 §2.6.10 第 5 条）。
    pub fn take_terminal(&mut self, task: &TaskSnapshot) -> bool {
        let key = (task.task_id.clone(), task.lifecycle.clone());
        if self.reported_terminals.contains(&key) {
            return false;
        }
        self.reported_terminals.push(key);
        if self.reported_terminals.len() > 64 {
            self.reported_terminals.drain(0..32);
        }
        true
    }

    /// 判断该离散事件是否首次出现（用于触发业务动作）
    pub fn take_event(&mut self, task: &TaskSnapshot) -> bool {
        if task.last_event_seq <= 0 {
            return false;
        }
        let key = (task.task_id.clone(), task.last_event_seq);
        if self.seen_events.contains(&key) {
            return false;
        }
        self.seen_events.push(key);
        if self.seen_events.len() > 256 {
            self.seen_events.drain(0..128);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(seq: i64) -> StatusFrame {
        StatusFrame::parse(&json!({
            "schema_version": 2,
            "boot_id": "boot-a",
            "snapshot_seq": seq,
            "revision": seq / 2,
            "reason": "heartbeat",
            "robot": { "task_state": "idle",
                "loc_state": { "status": 100, "fresh": true, "age_ms": 20 } },
            "current_task": null,
            "pending_task": null,
            "recent_terminal_tasks": []
        }))
        .expect("frame parses")
    }

    #[test]
    fn parses_v2_frame() {
        let f = frame(3);
        assert_eq!(f.boot_id, "boot-a");
        assert_eq!(f.snapshot_seq, 3);
        assert_eq!(f.loc_status, 100);
        assert!(f.loc_fresh);
        assert!(f.current.is_none());
    }

    #[test]
    fn drops_older_frames_and_resets_on_boot_change() {
        let mut monitor = TaskMonitor::default();
        assert!(monitor.accept(&frame(5)).is_some());
        assert!(monitor.accept(&frame(4)).is_none());
        assert!(monitor.accept(&frame(9)).is_some());

        let mut other = frame(1);
        other.boot_id = "boot-b".to_string();
        // boot_id 变化后允许接受更小的 seq
        assert!(monitor.accept(&other).is_some());
    }

    #[test]
    fn ignores_schema_v1() {
        assert!(StatusFrame::parse(&json!({"schema_version": 1})).is_none());
    }

    #[test]
    fn parses_task_detail_index() {
        let task = TaskSnapshot::from_value(&json!({
            "task_id": "t-1",
            "type": "patrol",
            "lifecycle": "running",
            "stage": "patrol.point_arrived",
            "progress": 40.0,
            "last_event": { "seq": 2, "type": "patrol.point_arrived", "data": { "index": 1, "total": 5 } },
            "detail": { "last_arrived_point": { "index": 1, "total": 5 } }
        }))
        .expect("task parses");
        assert_eq!(task.task_id, "t-1");
        assert_eq!(task.task_type, "patrol");
        assert_eq!(task.detail_index, Some(1));
        assert_eq!(task.detail_total, Some(5));
        assert!(!task.is_terminal());
    }

    #[test]
    fn seq_gate_resets_on_boot_change() {
        let mut gate = SeqGate::default();
        let first = frame(0);
        let r = gate.accept(&first);
        assert!(r.accepted && r.boot_changed, "首个帧必须被采用");
        assert!(!gate.accept(&frame(0)).accepted, "seq 相同视为重复帧");
        assert!(!gate.accept(&frame(0)).boot_changed);

        let mut other = frame(1);
        other.boot_id = "boot-b".to_string();
        let r = gate.accept(&other);
        assert!(r.accepted && r.boot_changed);
    }

    #[test]
    fn terminal_report_is_idempotent_per_lifecycle() {
        let mut monitor = TaskMonitor::default();
        let task = TaskSnapshot::from_value(&json!({
            "task_id": "t-1", "type": "navigation", "lifecycle": "success",
            "stage": "navigation.succeeded", "progress": 100.0
        }))
        .unwrap();
        assert!(monitor.take_terminal(&task));
        assert!(
            !monitor.take_terminal(&task),
            "current_task 与 recent_terminal_tasks 会给出同一条终态"
        );

        // 换了 lifecycle（例如 cancelled 后被重跑成 success）仍算新终态
        let mut again = task.clone();
        again.lifecycle = "cancelled".to_string();
        assert!(monitor.take_terminal(&again));
    }

    #[test]
    fn boot_change_clears_terminal_dedup() {
        let mut monitor = TaskMonitor::default();
        let task = TaskSnapshot::from_value(&json!({
            "task_id": "t-1", "type": "navigation", "lifecycle": "success"
        }))
        .unwrap();
        assert!(monitor.take_terminal(&task));

        let mut restarted = frame(1);
        restarted.boot_id = "boot-b".to_string();
        let accepted = monitor.accept(&restarted).expect("boot 变化后必须采用");
        assert!(accepted.boot_changed);
        assert!(
            monitor.take_terminal(&task),
            "算法进程重启后旧终态去重键应作废"
        );
    }

    #[test]
    fn dedupes_events() {
        let mut monitor = TaskMonitor::default();
        let task = TaskSnapshot::from_value(&json!({
            "task_id": "t-1", "type": "patrol", "lifecycle": "running",
            "stage": "patrol.waiting",
            "last_event": { "seq": 3, "type": "patrol.point_arrived" }
        }))
        .unwrap();
        assert!(monitor.take_event(&task));
        assert!(!monitor.take_event(&task));
    }
}
