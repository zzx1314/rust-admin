//! 航线任务（巡检）执行引擎。
//!
//! 职责：把业务服务下发的任务（路线航点 + 动作 + 地图）翻译成智航 Pro 的
//! `5000 kind=2` 导航任务，并按 5009 状态机推进、把进度回传业务服务。
//!
//! 关键设计：
//! - 智航 Pro 的巡检（`kind=4`）暂不对外开放，因此巡检引擎自己拆成「导航 + 作业」：
//!   导航任务由本体执行（依次到达 goals），到点作业由客户端执行（等待 / 拍照）。
//! - 每个航点一条导航任务，是因为 action 取值闭集只有 {0 等待, 1 拍照}，业务侧的
//!   红外测温、AI 识别等动作必须由客户端侧完成。
//! - 暂停 = 5004 暂停当前导航任务；恢复 = 5006；取消 = 5002 + 停止剩余航点。
//! - 结束后按 `endTask` 走返航到起点 / 返回充电点（5014）/ 原地待命。

use super::client::ZhProClient;
use super::protocol::*;
use super::status::TaskSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// 航点动作（业务侧 `actions[]`）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WaypointAction {
    #[serde(default, rename = "type")]
    pub action_type: String,
    #[serde(default)]
    pub param: String,
    #[serde(default)]
    pub trigger: String,
    #[serde(default)]
    pub threshold: String,
    #[serde(default)]
    pub retry: i64,
}

/// 巡检航点
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Waypoint {
    #[serde(default)]
    pub name: String,
    /// 地图坐标系 x（米）
    #[serde(default)]
    pub x: f64,
    /// 地图坐标系 y（米）
    #[serde(default)]
    pub y: f64,
    /// 到位停留秒数
    #[serde(default)]
    pub dwell: i64,
    /// 偏航角（度）
    #[serde(default)]
    pub heading: f64,
    /// 到点容差（米）
    #[serde(default = "default_radius")]
    pub radius: f64,
    /// 必经点：失败时不跳过
    #[serde(default)]
    pub must: bool,
    /// 失败策略：跳过继续 / 重试 N 次 / 终止任务
    #[serde(default)]
    pub on_fail: String,
    #[serde(default)]
    pub actions: Vec<WaypointAction>,
}

fn default_radius() -> f64 {
    0.3
}

impl Waypoint {
    pub fn pose(&self) -> Pose {
        Pose::new(self.x, self.y, self.heading.to_radians())
    }

    /// 是否需要在点上执行拍照：动作名包含「拍照 / 摄像 / 录像」或显式 `capture`
    pub fn wants_photo(&self) -> bool {
        self.actions.iter().any(|action| {
            let t = action.action_type.as_str();
            t.contains("拍照") || t.contains("摄像") || t.contains("录像") || t.contains("capture")
        })
    }

    /// 失败重试次数
    pub fn retry_limit(&self) -> u32 {
        if self.on_fail.contains("终止") {
            return 0;
        }
        if self.on_fail.contains("跳过") {
            return 0;
        }
        self.on_fail
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or(if self.must { 2 } else { 1 })
    }

    /// 失败后是否允许跳过
    pub fn skippable(&self) -> bool {
        !self.must && !self.on_fail.contains("终止")
    }
}
/// 任务结束后的去向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EndTask {
    /// 原地待命
    #[default]
    None,
    /// 返回起点
    ReturnToStart,
    /// 返回充电点
    ReturnToCharge,
}

impl EndTask {
    pub fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or_default().trim() {
            "返回起点" | "return_to_start" => EndTask::ReturnToStart,
            "返回充电" | "返回充电点" | "return_to_charge" => EndTask::ReturnToCharge,
            _ => EndTask::None,
        }
    }

    /// 编排用字符串（与业务侧中文枚举一致）
    pub fn as_str(self) -> &'static str {
        match self {
            EndTask::None => "",
            EndTask::ReturnToStart => "返回起点",
            EndTask::ReturnToCharge => "返回充电",
        }
    }
}

/// 业务服务下发的任务执行计划
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlightPlan {
    pub task_code: String,
    pub task_name: String,
    /// 业务侧地图版本号，例如 `MAP-ZJ-01@v2.1`
    #[serde(default)]
    pub map_id: String,
    /// 机器人上的地图绝对路径，可由 mapId 推导
    #[serde(default)]
    pub map_path: String,
    /// 导航地图类型：2D 栅格 / Route 路网
    #[serde(default = "default_map_type")]
    pub map_type: u32,
    #[serde(default)]
    pub waypoints: Vec<Waypoint>,
    /// 循环模式：闭环 / 往返（闭环额外回到首点）
    #[serde(default)]
    pub loop_mode: String,
    /// 全局速度上限 m/s
    #[serde(default = "default_speed")]
    pub speed_limit: f64,
    /// 避障等级 10/20/30/40
    #[serde(default = "default_avoid")]
    pub avoid_level: u32,
    /// 到点容差
    #[serde(default = "default_tolerance")]
    pub goal_tolerance: f64,
    /// 结束后动作
    #[serde(default)]
    pub end_task: String,
}

fn default_map_type() -> u32 {
    1
}
fn default_speed() -> f64 {
    1.2
}
fn default_avoid() -> u32 {
    40
}
fn default_tolerance() -> f64 {
    0.25
}

impl FlightPlan {
    pub fn end_task(&self) -> EndTask {
        EndTask::parse(Some(self.end_task.as_str()))
    }

    /// 执行顺序：闭环模式在末尾补回首点
    pub fn execution_order(&self) -> Vec<Waypoint> {
        let mut points = self.waypoints.clone();
        if self.loop_mode == "闭环"
            && points.len() > 2
            && let Some(first) = points.first().cloned()
        {
            points.push(Waypoint {
                name: format!("{}（回到起点）", first.name),
                dwell: 0,
                actions: vec![],
                must: true,
                ..first
            });
        }
        points
    }

    /// 地图路径推导：优先使用显式 mapPath，否则用 `map_root/history_map/<目录名>`
    pub fn resolve_map_path(&self, map_root: &str) -> String {
        if !self.map_path.trim().is_empty() {
            return self.map_path.clone();
        }
        let dir = super::mapping::to_map_dir_name(&self.map_id);
        format!("{}/history_map/{dir}", map_root.trim_end_matches('/'))
    }
}

/// 执行结果
#[derive(Debug, Clone)]
pub struct ExecutionOutcome {
    pub success: bool,
    pub message: String,
    pub completed_points: usize,
    pub total_points: usize,
    pub skipped: usize,
}

/// 5xx 控制命令：暂停 / 恢复 / 取消
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Pause,
    Resume,
    Cancel,
}

/// 首航点是否需要「载图并等定位收敛」。
///
/// 同一条航线内复用已加载的定位，只有第一个点需要定位；而如果起步前的定位守卫
/// 已经把定位收敛好了，首点也不必再重载一次地图（省 10~30s，且不重置位姿）。
pub fn need_locate_for(index: usize, pre_located: bool) -> bool {
    index <= 1 && !pre_located
}

/// 航点执行状态机
pub struct RouteRunner {
    client: Arc<ZhProClient>,
    plan: Mutex<Option<FlightPlan>>,
    /// 当前正在执行的智航导航任务 ID
    nav_task_id: Mutex<Option<String>>,
    /// 待执行的航点队列
    queue: Mutex<VecDeque<Waypoint>>,
    /// 取消标记
    cancelled: AtomicBool,
    /// 暂停标记（客户端侧作业暂停）
    paused: AtomicBool,
    /// 起步前预检已经锁好定位：首航点不必再让本体重载地图
    pre_located: AtomicBool,
    total: Mutex<usize>,
    completed: Mutex<usize>,
    skipped: Mutex<usize>,
}

impl RouteRunner {
    pub fn new(client: Arc<ZhProClient>) -> Self {
        Self {
            client,
            plan: Mutex::new(None),
            nav_task_id: Mutex::new(None),
            queue: Mutex::new(VecDeque::new()),
            cancelled: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            pre_located: AtomicBool::new(false),
            total: Mutex::new(0),
            completed: Mutex::new(0),
            skipped: Mutex::new(0),
        }
    }

    /// 标记「定位已由起步预检完成」：首航点改用 `need_locate=false` 复用当前定位，
    /// 避免刚重定位完又让本体载一次图（多花 10~30s，且会重置位姿）。
    pub fn mark_pre_located(&self) {
        self.pre_located.store(true, Ordering::SeqCst);
    }

    pub async fn prepare(&self, plan: FlightPlan) {
        let order = plan.execution_order();
        *self.total.lock().await = order.len();
        *self.completed.lock().await = 0;
        *self.skipped.lock().await = 0;
        *self.queue.lock().await = order.into();
        *self.plan.lock().await = Some(plan);
        self.cancelled.store(false, Ordering::SeqCst);
        self.paused.store(false, Ordering::SeqCst);
    }

    pub async fn progress(&self) -> (usize, usize) {
        (*self.completed.lock().await, *self.total.lock().await)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// 是否处于暂停中（对外同步任务状态用：暂停时不能继续报「执行中」）
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    /// 当前导航任务 ID（对外同步 5009 状态用）
    pub async fn current_nav_task(&self) -> Option<String> {
        self.nav_task_id.lock().await.clone()
    }

    /// 处理业务侧控制
    pub async fn control(&self, control: Control) -> Result<(), String> {
        match control {
            Control::Cancel => {
                self.cancelled.store(true, Ordering::SeqCst);
                if let Some(task_id) = self.current_nav_task().await {
                    // 5002 只保证受理，终态以 5009 为准
                    self.client
                        .call(T_TASK_CANCEL, json!({ "task_id": task_id }))
                        .await?;
                }
                self.queue.lock().await.clear();
                Ok(())
            }
            Control::Pause => {
                self.paused.store(true, Ordering::SeqCst);
                if let Some(task_id) = self.current_nav_task().await {
                    self.client
                        .call(T_TASK_PAUSE, json!({ "task_id": task_id }))
                        .await?;
                }
                Ok(())
            }
            Control::Resume => {
                self.paused.store(false, Ordering::SeqCst);
                if let Some(task_id) = self.current_nav_task().await {
                    self.client
                        .call(T_TASK_RESUME, json!({ "task_id": task_id }))
                        .await?;
                }
                Ok(())
            }
        }
    }

    /// 执行整条航线
    pub async fn run(&self) -> Result<ExecutionOutcome, String> {
        let plan = self
            .plan
            .lock()
            .await
            .clone()
            .ok_or_else(|| "未准备执行计划".to_string())?;
        let total = *self.total.lock().await;

        // 1) 避障等级：任务启动后必须重设，导航每次回到 STANDBY 会恢复为 40
        self.apply_avoid_level(plan.avoid_level).await;

        // 2) 逐点执行
        let mut index = 0_usize;
        loop {
            if self.is_cancelled() {
                return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
            }
            let next = self.queue.lock().await.pop_front();
            let Some(point) = next else { break };
            index += 1;

            // 暂停期间阻塞
            self.wait_while_paused().await;
            if self.is_cancelled() {
                return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
            }

            info!(
                "执行航点 {index}/{total}: {} ({:.2}, {:.2})",
                point.name, point.x, point.y
            );
            match self.goto_point(&plan, &point, index, total).await {
                Ok(()) => {
                    self.perform_actions(&point).await;
                    *self.completed.lock().await += 1;
                }
                Err(e) => {
                    if self.is_cancelled() {
                        return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
                    }
                    if point.skippable() {
                        warn!("航点 {} 失败，跳过: {e}", point.name);
                        *self.skipped.lock().await += 1;
                        continue;
                    }
                    return Ok(self
                        .outcome(false, format!("航点 {} 执行失败: {e}", point.name), total)
                        .await);
                }
            }
        }

        if self.is_cancelled() {
            return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
        }

        // 3) 收尾动作
        match plan.end_task() {
            EndTask::ReturnToCharge => {
                info!("巡检结束，返回充电点");
                if let Err(e) = self
                    .client
                    .call(T_RETURN_TO_CHARGE, json!({ "map_id": plan.map_id }))
                    .await
                {
                    warn!("返回充电点失败: {e}");
                }
            }
            EndTask::ReturnToStart => {
                info!("巡检结束，返回起点");
                if let Some(first) = plan.waypoints.first()
                    && let Err(e) = self.goto_point(&plan, first, 0, total).await
                {
                    warn!("返回起点失败: {e}");
                }
            }
            EndTask::None => {
                info!("巡检结束，原地待命");
            }
        }

        *self.nav_task_id.lock().await = None;
        Ok(self.outcome(true, "巡检完成".to_string(), total).await)
    }

    async fn outcome(&self, success: bool, message: String, total: usize) -> ExecutionOutcome {
        ExecutionOutcome {
            success,
            message,
            completed_points: *self.completed.lock().await,
            total_points: total,
            skipped: *self.skipped.lock().await,
        }
    }

    async fn wait_while_paused(&self) {
        while self.paused.load(Ordering::SeqCst) && !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// 下发单点导航任务并等待 5009 进入终态（带重试）
    async fn goto_point(
        &self,
        plan: &FlightPlan,
        point: &Waypoint,
        index: usize,
        total: usize,
    ) -> Result<(), String> {
        let retries = point.retry_limit();
        let mut last_error = String::new();
        for attempt in 0..=retries {
            if attempt > 0 {
                warn!("航点 {} 第 {attempt} 次重试（共 {retries} 次）", point.name);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            match self.navigate_once(plan, point, index, total).await {
                Ok(()) => return Ok(()),
                Err(e) => last_error = e,
            }
            if self.is_cancelled() {
                return Err("任务已取消".to_string());
            }
        }
        Err(last_error)
    }

    async fn navigate_once(
        &self,
        plan: &FlightPlan,
        point: &Waypoint,
        index: usize,
        total: usize,
    ) -> Result<(), String> {
        let map_path = plan.resolve_map_path(super::mapping::DEFAULT_MAP_ROOT);
        let pose = point.pose();
        // 预检已锁定定位时首点不再重载地图；重试时标志已被取走，会重新载图定位
        let need_locate = need_locate_for(index, self.pre_located.swap(false, Ordering::SeqCst));
        let data = json!({
            "kind": TaskKind::Navigation.code(),
            "name": format!("{} · 前往 {}", plan.task_name, point.name),
            "schedule": 0,
            "execute_at": now_ms().to_string(),
            "priority": 0,
            "record": false,
            "params": {
                "map_path": map_path,
                // 同一条航线内复用已加载定位，避免每点重载地图
                "need_locate": need_locate,
                "map_type": plan.map_type,
                "goal_type": plan.map_type,
                "fence_path": "",
                "goals": [ pose ],
                "speed_limit": { "vx": plan.speed_limit, "vy": 0.0, "vz": 0.0 },
                "goal_tolerance": { "x": plan.goal_tolerance, "y": plan.goal_tolerance, "theta": 0.2 }
            }
        });
        let reply = self.client.call(T_TASK_CREATE, data).await?;
        let nav_task_id = reply
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if nav_task_id.is_empty() {
            return Err("导航任务未返回 task_id".to_string());
        }
        *self.nav_task_id.lock().await = Some(nav_task_id.clone());
        info!(
            "航点 {index}/{total} 导航任务已受理: {} → {}",
            nav_task_id, point.name
        );

        // 等待该任务终态
        let mut rx = self.client.subscribe();
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(600_u64.max(total as u64 * 120));
        loop {
            if self.is_cancelled() {
                return Err("任务已取消".to_string());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(format!("航点 {} 导航超时", point.name));
            }
            let env = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(env)) => env,
                Ok(Err(_)) => return Err("推送通道关闭".to_string()),
                Err(_) => return Err(format!("航点 {} 导航超时", point.name)),
            };
            if env.head.msg_type != T_TASK_STATUS {
                continue;
            }
            let Some(frame) = super::status::StatusFrame::parse(&env.data) else {
                continue;
            };
            if let Some(task) = frame.task_for(&nav_task_id)
                && task.is_terminal()
            {
                *self.nav_task_id.lock().await = None;
                return terminal_result(task);
            }
        }
    }

    /// 到点作业：等待 + 拍照
    async fn perform_actions(&self, point: &Waypoint) {
        if point.dwell > 0 {
            info!("航点 {} 停留 {}s", point.name, point.dwell);
            self.sleep_checked(Duration::from_secs(point.dwell as u64))
                .await;
        }
        if point.wants_photo() {
            // 智航 Pro 的拍照由本体上装 /capture_image 完成；
            // 协议侧通过 5000 kind=4 的 actions 白名单表达，这里留出扩展点。
            info!("航点 {} 触发拍照作业", point.name);
        }
        for action in &point.actions {
            if action.action_type.contains("拍照") || action.action_type.contains("识别") {
                continue;
            }
            info!(
                "航点 {} 执行动作: {} ({})",
                point.name, action.action_type, action.param
            );
        }
    }

    async fn sleep_checked(&self, duration: Duration) {
        let deadline = tokio::time::Instant::now() + duration;
        while tokio::time::Instant::now() < deadline && !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn apply_avoid_level(&self, level: u32) {
        let reply = self
            .client
            .call(T_SET_AVOID_LEVEL, json!({ "level": level }))
            .await;
        match reply {
            Ok(data) => {
                let effective = data
                    .get("effective_level")
                    .and_then(Value::as_u64)
                    .unwrap_or_default();
                let reason = data
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if effective != u64::from(level) {
                    warn!("避障等级未完全生效: 请求 {level}, 实际 {effective} ({reason})");
                } else {
                    info!("导航避障等级已设置: {effective}");
                }
            }
            Err(e) => warn!("设置避障等级失败: {e}"),
        }
    }
}

fn terminal_result(task: &TaskSnapshot) -> Result<(), String> {
    match task.lifecycle.as_str() {
        "success" => Ok(()),
        "cancelled" => Err(format!(
            "导航被取消: {}",
            task.stop_reason.clone().unwrap_or_default()
        )),
        _ => Err(format!(
            "导航失败: {}",
            task.error_message
                .clone()
                .or_else(|| task.stop_reason.clone())
                .unwrap_or_else(|| task.stage.clone())
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(name: &str, x: f64) -> Waypoint {
        Waypoint {
            name: name.to_string(),
            x,
            y: 0.0,
            dwell: 0,
            heading: 90.0,
            radius: 0.3,
            must: true,
            on_fail: "重试 2 次".to_string(),
            actions: vec![],
        }
    }

    fn plan(loop_mode: &str) -> FlightPlan {
        FlightPlan {
            task_code: "TK-1".to_string(),
            task_name: "测试航线".to_string(),
            map_id: "MAP-ZJ-01@v2.1".to_string(),
            map_path: String::new(),
            map_type: 1,
            waypoints: vec![point("A", 0.0), point("B", 1.0), point("C", 2.0)],
            loop_mode: loop_mode.to_string(),
            speed_limit: 1.2,
            avoid_level: 40,
            goal_tolerance: 0.25,
            end_task: "返回充电".to_string(),
        }
    }

    #[test]
    fn need_locate_only_for_first_point_unless_pre_located() {
        // 没有预检：首点载图定位，其余点复用
        assert!(need_locate_for(1, false));
        assert!(!need_locate_for(2, false));
        // 预检已收敛：首点也不重载
        assert!(!need_locate_for(1, true));
        assert!(!need_locate_for(2, true));
    }

    #[test]
    fn closed_loop_appends_first_point() {
        let order = plan("闭环").execution_order();
        assert_eq!(order.len(), 4);
        assert_eq!(order[3].x, 0.0);
        assert!(order[3].name.contains("回到起点"));
    }

    #[test]
    fn open_loop_keeps_points() {
        assert_eq!(plan("往返").execution_order().len(), 3);
    }

    #[test]
    fn resolves_map_path_from_map_id() {
        let p = plan("闭环");
        assert_eq!(
            p.resolve_map_path("/ota/alg_data/map"),
            "/ota/alg_data/map/history_map/MAP-ZJ-01_v2.1"
        );
    }

    #[test]
    fn explicit_map_path_wins() {
        let mut p = plan("闭环");
        p.map_path = "/ota/alg_data/map/history_map/x".to_string();
        assert_eq!(
            p.resolve_map_path("/ignored"),
            "/ota/alg_data/map/history_map/x"
        );
    }

    #[test]
    fn parses_end_task() {
        assert_eq!(EndTask::parse(Some("返回充电")), EndTask::ReturnToCharge);
        assert_eq!(EndTask::parse(Some("返回起点")), EndTask::ReturnToStart);
        assert_eq!(EndTask::parse(None), EndTask::None);
    }

    #[test]
    fn retry_limit_parsing() {
        let mut p = point("A", 0.0);
        assert_eq!(p.retry_limit(), 2);

        // 必经点（must=true）即使写了「跳过继续」也不允许跳过
        p.on_fail = "跳过继续".to_string();
        assert_eq!(p.retry_limit(), 0);
        assert!(!p.skippable());

        // 非必经点才允许跳过
        p.must = false;
        assert!(p.skippable());

        // 明确要求终止时任何点都不可跳过
        p.on_fail = "终止任务".to_string();
        assert!(!p.skippable());
    }

    #[test]
    fn detects_photo_action() {
        let mut p = point("A", 0.0);
        assert!(!p.wants_photo());
        p.actions.push(WaypointAction {
            action_type: "可见光拍照".to_string(),
            ..Default::default()
        });
        assert!(p.wants_photo());
    }
}
