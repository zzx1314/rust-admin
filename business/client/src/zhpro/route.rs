//! 航线任务（巡检）执行引擎。
//!
//! 职责：把业务服务下发的任务（路线航点 + 动作 + 地图）翻译成智航 Pro 的任务，
//! 并按 5009 状态机推进、把进度回传业务服务。
//!
//! 下发形态只有一种：**巡检 `5000 kind=4`**（与官方 App 一致）——先把路线写成本体
//! `nav_paths/<path_id>`（`4024` 添加 / `4028` 覆盖），再下发一条巡检任务；航点、
//! 到点动作、返程全部由本体 ALG 执行，客户端只跟 `5009 patrol.*`。
//! 字段依据 2026-11-14 抓到官方 App 的真实报文（见 [`patrol_task_payload`]）。
//!
//! 旧的 `5000 kind=2` 导航模式（逐段 goals + 客户端到点作业）**已移除**：
//! `.1` 的 `dog_task`（`robots_dog_msgs 0.9.0`）与 `.234`（`0.9.8`）的
//! `NavigationState` 消息定义不一致，一导航就 `bad_alloc` 崩溃，等厂商修复。
//! - 整条航线只下发**一条** `5000 kind=4` 巡检任务：航点、到点动作、返程都由本体执行，
//!   客户端只负责「写路径（4024/4028）+ 建任务 + 跟 5009」。
//! - 暂停 = 5004；恢复 = 5006；取消 = 5002 + 等本体释放导航能力。
//! - 结束动作按 `endTask` 映射成巡检的 `end_task`：0 原地 / 1 返回起点 / 2 返回充电点。

use super::client::ZhProClient;
use super::protocol::*;
use super::status::TaskSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

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

    /// 这个点是否需要**停下来做作业**（停留 / 拍照 / 其它动作）。
    ///
    /// 巡检里它决定路径点的 `properties.type`：有作业就是 `1`（作业点），否则 `0`
    /// （途径点）；路径点的 `tasks` 由停留/拍照翻成 `[0 等待] / [1 拍照]`
    /// （见 [`point_path_tasks`]）。
    pub fn has_on_point_work(&self) -> bool {
        self.dwell > 0 || self.wants_photo() || !self.actions.is_empty()
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
    /// 航段是否为「沿道路」：决定 5000 用 2D 栅格还是路网地图
    #[serde(default)]
    pub road_network: bool,
    /// 电子围栏文件路径；空表示不带围栏
    #[serde(default)]
    pub fence_path: Option<String>,
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

    /// 本体地图目录名（4013 `path` 的最后一段）。
    ///
    /// 5014 返充、4030 重定位这类接口要的都是**目录名**，不是业务地图 ID
    /// （`ROS-MAP-3@v1.0` 在本体上并不存在）。下发前 `map_path` 已被替换成本体
    /// 上报的绝对路径，取最后一段即可；没有路径时退回业务 ID 合成名。
    pub fn robot_map_dir(&self) -> String {
        let trimmed = self.map_path.trim().trim_end_matches('/');
        let last = trimmed.rsplit('/').next().unwrap_or_default();
        if last.is_empty() {
            super::mapping::to_map_dir_name(&self.map_id)
        } else {
            last.to_string()
        }
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

/// 本体「能力忙」错误码：`13897 resource conflict (capability busy)`
///
/// 现场含义：本体上还有一个没结束的任务（典型是被 `5004` 暂停后一直挂着的导航任务），
/// 新的 `5000` 会被直接拒。属于**可重试**的忙。
pub const CODE_CAPABILITY_BUSY: &str = "13897";

/// 本体「任务不存在」错误码：`13896 task not found`
///
/// 拿它当「代理任务已不在本体上」的判据（例如固件不认 `task_id: "current"`）。
pub const CODE_TASK_NOT_FOUND: &str = "13896";

/// 能力忙的重试次数与间隔（合计约 30s，等本体把上一个任务收尾）
pub const BUSY_RETRIES: u32 = 6;
pub const BUSY_BACKOFF: Duration = Duration::from_secs(5);

/// 建任务前 / 5002 取消后，等本体 `5009 robot.task_state` 回到 `idle` 的最长时间。
///
/// 本体同一时刻只允许一个导航任务，t 从"终态"到"能力真的释放"有几百毫秒延迟，
/// 不等就会拿到 13897。
pub const IDLE_WAIT: Duration = Duration::from_secs(15);

/// 5000 `params.speed_limit` 缺省线速度（航线没配速度时用）
const DEFAULT_SPEED_LIMIT: f64 = 1.2;

/// 5000 `params.speed_limit.vz`：**角速度上限**，单位 rad/s
///
/// 取协议文档示例的 1.5。以前写死 0.0，等于告诉本体「不允许转向」，
/// 需要拐弯的航点会被规划器判为「无有效绕障轨迹，搜索失败」。
const YAW_LIMIT: f64 = 1.5;

/// 错误串里是否为「能力忙」（13897）
pub fn is_capability_busy(error: &str) -> bool {
    error.contains(CODE_CAPABILITY_BUSY)
}

/// 错误串里是否为「任务不存在」（13896）
pub fn is_task_not_found(error: &str) -> bool {
    error.contains(CODE_TASK_NOT_FOUND)
}

/// 巡检任务（`5000 kind=4`）载荷。
///
/// 字段以 2026-11-14 抓到的官方 App 原文为准（手机 → 网关 `192.168.234.8`）：
///
/// ```json
/// {"kind":4,"name":"2026_11_14_13_35_50","schedule":0,
///  "execute_at":"2026-11-14 13:36:29","priority":0,"record":true,
///  "params":{"map_id":"2026_11_10_16_15_06","map_type":0,"end_task":1,
///            "min_recharge_battery":25.0,
///            "speed_limit":{"vx":0.5,"vy":0.0,"vz":1.5},
///            "path_id":"自定义路径1"}}
/// ```
///
/// 与 `kind=2` 的关键差别：**请求里不带任何坐标**，航点/到点动作全部来自本体
/// `nav_paths/<path_id>` 文件（见 [`add_nav_path_payload`]）；`map_id` 是本体**目录名**；
/// `map_type` 走巡检自己的枚举（`0`=2D 栅格 / `1`=路网）；`end_task` 是数字；
/// `execute_at` 是**本地时间字符串**（不是毫秒）。
pub(crate) fn patrol_task_payload(
    plan: &FlightPlan,
    map_id: &str,
    path_id: &str,
    min_recharge_battery: f64,
) -> Value {
    json!({
        "kind": TaskKind::Patrol.code(),
        "name": plan.task_name,
        "schedule": 0,
        "execute_at": local_timestamp(),
        "priority": 0,
        // 与官方一致：巡检要落 5010/5011 记录
        "record": true,
        "params": {
            "map_id": map_id,
            // 巡检枚举：0 = 2D 栅格、1 = 路网（不是 kind=2 的 1/3！）
            "map_type": patrol_map_type(plan),
            "end_task": patrol_end_task(plan),
            "min_recharge_battery": min_recharge_battery,
            "speed_limit": { "vx": speed_vx(plan), "vy": 0.0, "vz": YAW_LIMIT },
            "path_id": path_id
        }
    })
}

/// `4024 add_nav_path` 载荷：`data.add_nav_path = [map_id, path_id, points, forbidden_zones]`
///
/// `points` 是 `[点名, {position, orientation, properties:{type, tasks}}]`，
/// `properties.type` = `0` 途径点 / `1` 作业点，`tasks` = `[0 等待(固定 2 s) / 1 拍照]`。
pub(crate) fn add_nav_path_payload(map_id: &str, path_id: &str, points: &[Waypoint]) -> Value {
    json!({
        "add_nav_path": [map_id, path_id, nav_path_points(points), []]
    })
}

/// `4028 modify_nav_path` 载荷：`[map_id, old_path, new_path, points, forbidden_zones]`
pub(crate) fn modify_nav_path_payload(
    map_id: &str,
    old_path: &str,
    new_path: &str,
    points: &[Waypoint],
) -> Value {
    json!({
        "modify_nav_path": [map_id, old_path, new_path, nav_path_points(points), []]
    })
}

fn nav_path_points(points: &[Waypoint]) -> Vec<Value> {
    points
        .iter()
        .map(|point| {
            let pose = point.pose();
            let (kind, tasks) = point_path_tasks(point);
            json!([
                point.name,
                {
                    "position": { "x": pose.position.x, "y": pose.position.y, "z": 0.0 },
                    "orientation": {
                        "x": pose.orientation.x,
                        "y": pose.orientation.y,
                        "z": pose.orientation.z,
                        "w": pose.orientation.w
                    },
                    "properties": { "type": kind, "tasks": tasks }
                }
            ])
        })
        .collect()
}

/// 航点 → 本体路径点的 `(properties.type, properties.tasks)`。
///
/// - `type`：有停留/拍照/其它动作 → `1`（作业点），否则 `0`（途径点）；
/// - `tasks`：协议只认 `0`（等待，**固定 2 s**）/ `1`（拍照），可重复；
///   途径点即使写了 `tasks` 也会被忽略，所以这里对 `type=0` 直接给空数组。
fn point_path_tasks(point: &Waypoint) -> (u8, Vec<u8>) {
    if !point.has_on_point_work() {
        return (0, Vec::new());
    }
    let mut tasks = Vec::new();
    if point.dwell > 0 {
        tasks.push(0); // 等待（本体固定 2 s，秒数不可配）
    }
    if point.wants_photo() {
        tasks.push(1); // 拍照（调 /capture_image）
    }
    (1, tasks)
}

/// 巡检的 `map_type` 枚举：`0` = 2D 栅格、`1` = 路网（`plan.map_type` 里 `3` 才是路网）
fn patrol_map_type(plan: &FlightPlan) -> u32 {
    if plan.map_type == 3 { 1 } else { 0 }
}

/// 巡检的 `end_task` 数字枚举：`0` = 原地 / `1` = 返回起点 / `2` = 返回充电点
///
/// 闭环航线且没显式指定结束动作时，按「返回起点」下发（与旧 `kind=2` 的闭环语义一致）。
fn patrol_end_task(plan: &FlightPlan) -> u32 {
    match plan.end_task() {
        EndTask::ReturnToStart => 1,
        EndTask::ReturnToCharge => 2,
        EndTask::None if plan.loop_mode == "闭环" => 1,
        EndTask::None => 0,
    }
}

/// 官方 `execute_at` 用的是**本地时间字符串**（`YYYY-MM-DD HH:MM:SS`），不是毫秒时间戳
fn local_timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 航线没配速度时给一个可跑的值（0 会让本体拒任务）
fn speed_vx(plan: &FlightPlan) -> f64 {
    if plan.speed_limit > 0.0 {
        plan.speed_limit
    } else {
        DEFAULT_SPEED_LIMIT
    }
}

/// 航点执行状态机
pub struct RouteRunner {
    client: Arc<ZhProClient>,
    plan: Mutex<Option<FlightPlan>>,
    /// 当前正在执行的智航导航任务 ID
    nav_task_id: Mutex<Option<String>>,
    /// 取消标记
    cancelled: AtomicBool,
    /// 暂停标记（客户端侧作业暂停）
    paused: AtomicBool,
    total: Mutex<usize>,
    completed: Mutex<usize>,
    skipped: Mutex<usize>,
    /// 巡检路径名前缀（本体 `nav_paths/<前缀>-<task_code>`）
    patrol_path_prefix: Mutex<String>,
    /// 巡检任务返充电量阈值
    min_recharge_battery: Mutex<f64>,
    /// 本体可用电量（双电池取较低那节，%）：巡检下发前做低电守卫
    battery_percent: Mutex<i64>,
}

impl RouteRunner {
    pub fn new(client: Arc<ZhProClient>) -> Self {
        Self {
            client,
            plan: Mutex::new(None),
            nav_task_id: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            total: Mutex::new(0),
            completed: Mutex::new(0),
            skipped: Mutex::new(0),
            patrol_path_prefix: Mutex::new("平台".to_string()),
            min_recharge_battery: Mutex::new(25.0),
            battery_percent: Mutex::new(100),
        }
    }

    /// 本体可用电量（双电池取较低那节）。巡检下发前用它和 `min_recharge_battery` 比：
    /// 低于阈值时本体 ALG 会在起步阶段中断巡检，报的却是
    /// `origin pose unavailable for return-to-start`（因为起点位姿还没采），
    /// 现场完全看不出是电量问题，所以这里提前拦掉。
    pub async fn set_battery_percent(&self, percent: i64) {
        *self.battery_percent.lock().await = percent;
    }

    /// 巡检相关参数：本体路径名前缀、返充电量阈值
    pub async fn set_patrol_options(&self, prefix: &str, min_recharge_battery: f64) {
        *self.patrol_path_prefix.lock().await = prefix.trim().to_string();
        *self.min_recharge_battery.lock().await = min_recharge_battery.clamp(0.0, 100.0);
    }

    /// 准备执行计划：总点数 = 路线点数（巡检由本体按 `nav_paths` 里的点执行，不做闭环补点）
    pub async fn prepare(&self, plan: FlightPlan) {
        *self.total.lock().await = plan.waypoints.len();
        *self.completed.lock().await = 0;
        *self.skipped.lock().await = 0;
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
                    // 5002 只保证受理，终态以 5009 为准。
                    // 本体上已经没有这个任务（已终态 / 固件不认这个 id）时目的已达到，不报错。
                    match self
                        .client
                        .call(T_TASK_CANCEL, json!({ "task_id": task_id }))
                        .await
                    {
                        Ok(_) => {}
                        Err(e) if is_task_not_found(&e) => {
                            warn!("取消任务时本体报 task not found，视为已收尾: {e}");
                        }
                        Err(e) => return Err(e),
                    }
                    *self.nav_task_id.lock().await = None;
                }
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

    /// 执行整条航线（巡检 `5000 kind=4`）
    pub async fn run(&self) -> Result<ExecutionOutcome, String> {
        let plan = self
            .plan
            .lock()
            .await
            .clone()
            .ok_or_else(|| "未准备执行计划".to_string())?;
        let total = *self.total.lock().await;
        self.run_patrol(&plan, total).await
    }

    /// 巡检模式（`5000 kind=4`，与官方 App 一致）执行整条航线。
    ///
    /// 流程：切导航模式 → 设避障等级 → **把路线写成本体路径**（`4024`，已存在则 `4028`）
    /// → 下发一条巡检任务 → 跟 `5009 patrol.*` 到终态。
    /// 到点动作（停留/拍照）与返程都由本体 ALG 执行，客户端不再逐点作业。
    async fn run_patrol(
        &self,
        plan: &FlightPlan,
        total: usize,
    ) -> Result<ExecutionOutcome, String> {
        // 巡检只用原始航点：闭环/返程由 `end_task` 让本体处理，不再补回首点
        let points = plan.waypoints.clone();
        if points.is_empty() {
            return Ok(self.outcome(false, "航线没有航点".to_string(), total).await);
        }
        let map_id = plan.robot_map_dir();
        let path_id = format!(
            "{}-{}",
            self.patrol_path_prefix.lock().await.as_str(),
            plan.task_code
        );
        let min_battery = *self.min_recharge_battery.lock().await;

        // 0) 低电守卫：本体 ALG 的 `min_recharge_battery` 语义是「低于阈值就中断巡检回起点」，
        //    如果**起步时**就已经不高于阈值，它会先中断、再因为还没采到起点位姿而报
        //    `origin pose unavailable for return-to-start`——现场完全看不出是电量问题。
        //    这里提前拦掉，给一句人能看懂的话。
        let threshold = *self.min_recharge_battery.lock().await;
        let battery = *self.battery_percent.lock().await;
        if threshold > 0.0 && (battery as f64) <= threshold {
            let message = format!(
                "本体可用电量 {battery}% 不高于巡检阈值 {threshold:.0}%（min_recharge_battery）：\
                 本体低电保护会在起步阶段中断巡检，并报成 “origin pose unavailable for \
                 return-to-start”。请先充电到 {:.0}% 以上，或调低 --min-recharge-battery",
                threshold + 10.0
            );
            warn!("巡检任务拒绝下发: {message}");
            return Ok(self.outcome(false, message, total).await);
        }

        // 1) 控制权 + 导航模式：4044/5000 都在 4000~5999 非白名单区间，没控制权网关不转发
        if let Err(e) = super::teleop::enter_navigation_mode(&self.client).await {
            warn!("切导航模式失败（继续尝试下发，但本体可能拒任务）: {e}");
        }
        // 1) 避障等级：导航每次回到 STANDBY 会恢复为 40，任务启动后必须重设
        self.apply_avoid_level(plan.avoid_level).await;

        // 2) 把航线写成本体路径（巡检任务的航点/动作全在路径文件里）
        self.ensure_nav_path(&map_id, &path_id, &points).await?;

        // 3) 下发巡检任务并跟到终态（能力忙按 §13897 退避重试）
        let data = patrol_task_payload(plan, &map_id, &path_id, min_battery);
        debug!("5000 kind=4 载荷（{total} 个航点）: {data}");
        let mut busy_waits = 0_u32;
        loop {
            if self.is_cancelled() {
                return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
            }
            self.wait_body_idle(IDLE_WAIT).await;
            let error = match self.create_patrol_task(&data, total).await {
                Ok(()) => {
                    info!("巡检任务完成: {path_id}（{total} 个航点）");
                    return Ok(self.outcome(true, "巡检完成".to_string(), total).await);
                }
                Err(e) => e,
            };
            if self.is_cancelled() {
                return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
            }
            self.abort_task().await;
            if is_capability_busy(&error) {
                busy_waits += 1;
                if busy_waits > BUSY_RETRIES {
                    return Ok(self
                        .outcome(
                            false,
                            format!("巡检任务失败: {error}（本体上仍有未结束的任务，请先取消或重启本体）"),
                            total,
                        )
                        .await);
                }
                warn!(
                    "本体能力忙（13897），{}s 后重试 {busy_waits}/{BUSY_RETRIES}",
                    BUSY_BACKOFF.as_secs()
                );
                tokio::time::sleep(BUSY_BACKOFF).await;
                continue;
            }
            return Ok(self
                .outcome(
                    false,
                    format!("巡检任务失败: {error}（path={path_id} map={map_id}）"),
                    total,
                )
                .await);
        }
    }

    /// 确保本体上存在这条巡检路径：先 `4024` 添加，失败（多半是已存在）再 `4028` 覆盖。
    async fn ensure_nav_path(
        &self,
        map_id: &str,
        path_id: &str,
        points: &[Waypoint],
    ) -> Result<(), String> {
        let add = add_nav_path_payload(map_id, path_id, points);
        match self.client.call(T_PATH_ADD, add).await {
            Ok(_) => {
                info!("已写入本体巡检路径: {path_id}（{} 个点）", points.len());
                Ok(())
            }
            Err(first) => {
                debug!("4024 添加路径失败（{first}），改用 4028 覆盖");
                let modify = modify_nav_path_payload(map_id, path_id, path_id, points);
                match self.client.call(T_PATH_MODIFY, modify).await {
                    Ok(_) => {
                        info!("已覆盖本体巡检路径: {path_id}（{} 个点）", points.len());
                        Ok(())
                    }
                    Err(second) => Err(format!("写本体路径失败（4024: {first}；4028: {second}）")),
                }
            }
        }
    }

    /// 下发一次 `5000 kind=4` 并跟到终态。
    ///
    /// 进度取 `5009 detail.last_arrived_point.index`（巡检是**0-based**，到点即 `index+1`）；
    /// 其余（终态、boot_id 变化、取消）与导航模式一致。
    async fn create_patrol_task(&self, data: &Value, total: usize) -> Result<(), String> {
        let mut rx = self.client.subscribe();
        let reply = self.client.call(T_TASK_CREATE, data.clone()).await?;
        debug!("5001 应答（巡检）: {reply}");
        let task_id = reply
            .get("task_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if task_id.is_empty() {
            return Err("巡检任务未返回 task_id".to_string());
        }
        *self.nav_task_id.lock().await = Some(task_id.clone());
        info!("巡检任务已受理: {task_id}（{total} 个航点，path 见 5000 载荷）");

        // 官方实测约 80 s/点（vx≈0.5），这里按 180 s/点 给足余量
        let deadline = tokio::time::Instant::now() + Duration::from_secs(180 * total.max(1) as u64);
        let mut gate = super::status::SeqGate::default();
        loop {
            if self.is_cancelled() {
                return Err("任务已取消".to_string());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(format!("巡检任务超时（{total} 个航点）"));
            }
            let env = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(env)) => env,
                Ok(Err(_)) => return Err("推送通道关闭".to_string()),
                Err(_) => return Err(format!("巡检任务超时（{total} 个航点）")),
            };
            if env.head.msg_type != T_TASK_STATUS {
                continue;
            }
            let Some(frame) = super::status::StatusFrame::parse(&env.data) else {
                continue;
            };
            let gate_result = gate.accept(&frame);
            if !gate_result.accepted {
                continue;
            }
            if gate_result.boot_changed {
                return Err("算法节点重启（boot_id 变化），本体任务已失效".to_string());
            }
            let Some(task) = frame.task_for(&task_id) else {
                continue;
            };
            // 巡检 `last_arrived_point.index` 从 0 开始；`point_completed` 时才算本点作业做完，
            // 这里用它推进度（至少到点就记 1，避免拍照失败时进度卡住）
            if let Some(index) = task.detail_index.filter(|index| *index >= 0) {
                let done = (index + 1) as usize;
                let mut completed = self.completed.lock().await;
                if done > *completed && done <= total {
                    *completed = done;
                }
            }
            if task.is_terminal() {
                let threshold = *self.min_recharge_battery.lock().await;
                let battery = *self.battery_percent.lock().await;
                let result =
                    terminal_result(task).map_err(|e| explain_patrol_error(&e, threshold, battery));
                if result.is_ok() {
                    *self.nav_task_id.lock().await = None;
                }
                return result;
            }
        }
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

    /// 等本体的导航能力回到 idle（`5009 robot.task_state`），最多 `max_wait`。
    ///
    /// 只发 5002 不够：固件受理后还要几百毫秒才把 `task_state` 改回 `idle`，
    /// 这期间发下一条 `5000` 依旧被回 13897——这就是「逐点下发从第 2 个点开始全 busy」
    /// 的直接原因。
    async fn wait_body_idle(&self, max_wait: Duration) {
        let mut rx = self.client.subscribe();
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                warn!("等待本体释放导航能力超时（{}s）", max_wait.as_secs());
                return;
            }
            let env = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(env)) => env,
                _ => return,
            };
            if env.head.msg_type != T_TASK_STATUS {
                continue;
            }
            let Some(frame) = super::status::StatusFrame::parse(&env.data) else {
                continue;
            };
            if frame.robot_task_state == "idle" && frame.current.is_none() {
                return;
            }
        }
    }

    /// 失败 / 取消后把当前导航任务收尾，并**等本体真的释放导航能力**再继续。
    async fn abort_task(&self) {
        let task_id = self.nav_task_id.lock().await.take();
        let Some(task_id) = task_id else {
            return;
        };
        match self
            .client
            .call(T_TASK_CANCEL, json!({ "task_id": task_id }))
            .await
        {
            Ok(_) => info!("已 5002 取消导航任务 {task_id}，等本体释放导航能力"),
            Err(e) if is_task_not_found(&e) => {
                debug!("取消导航任务时本体报 task not found，视为已收尾: {e}");
            }
            Err(e) => warn!("取消导航任务失败: {e}"),
        }
        self.wait_body_idle(IDLE_WAIT).await;
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

/// 把巡检任务终态里的「本体原话」翻成能操作的提示。
///
/// 现场踩过的两条：
/// - `origin pose unavailable for return-to-start`：**绝大多数是低电保护**在起步阶段中断巡检
///   （`min_recharge_battery` ≥ 当前电量），因为起点位姿是任务第 2 步才采的，还没采就中断；
///   其次才是定位没收敛/地图没载好。
/// - `capture_image(/capture_image): service not ready`（14005）：上装拍照服务没起，
///   拍照会失败但巡检本身还能继续。
fn explain_patrol_error(raw: &str, threshold: f64, battery: i64) -> String {
    if raw.contains("origin pose unavailable") {
        return format!(
            "本体拒绝返回起点（origin pose 缺失）：最常见原因是**低电保护**在起步阶段中断巡检——\
             本体读到的电量 {battery}% 不高于巡检阈值 {threshold:.0}%（min_recharge_battery）；\
             其次可能是定位未收敛或地图未载好。请先充电或调低 --min-recharge-battery。本体原话: {raw}"
        );
    }
    if raw.contains("service not ready") && raw.contains("capture_image") {
        return format!(
            "上装拍照服务未就绪（payload /capture_image）：到点拍照会记为失败，但不影响导航与\
             整体成功；需要拍照时先在本体启动上装/感知服务。本体原话: {raw}"
        );
    }
    raw.to_string()
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
            road_network: false,
            fence_path: None,
        }
    }

    #[test]
    fn patrol_error_explains_origin_pose_as_low_battery() {
        let raw = "导航失败: patrol step failed: origin pose unavailable for return-to-start";
        let text = explain_patrol_error(raw, 25.0, 23);
        assert!(text.contains("低电保护"), "要说清是低电: {text}");
        assert!(text.contains("23%") && text.contains("25%"), "要带上实测电量和阈值: {text}");
        assert!(text.contains("min_recharge_battery"), "要给出可操作的配置项: {text}");
        assert!(text.contains(raw), "还要保留本体原话方便对日志: {text}");
    }

    #[test]
    fn patrol_error_explains_capture_image_service() {
        let raw = "payload capture image failed: capture_image(/capture_image): service not ready";
        let text = explain_patrol_error(raw, 25.0, 80);
        assert!(text.contains("/capture_image"), "{text}");
        assert!(text.contains("不影响导航"), "要说清「不影响整体」: {text}");
    }

    #[test]
    fn patrol_error_passes_through_unknown_messages() {
        let raw = "5001 响应失败: code=13897 resource conflict (capability busy)";
        assert_eq!(explain_patrol_error(raw, 25.0, 80), raw);
    }

    #[test]
    fn patrol_payload_matches_official_shape() {
        let mut p = plan("闭环");
        p.end_task = String::new(); // 未指定 → 闭环按「返回起点」
        let data = patrol_task_payload(&p, "2026_11_10_16_15_06", "平台-TK-1", 25.0);

        assert_eq!(data["kind"], 4, "巡检 = kind 4");
        assert_eq!(data["schedule"], 0);
        assert_eq!(data["priority"], 0);
        assert_eq!(data["record"], true, "巡检要落 5010/5011 记录");
        assert_eq!(data["params"]["map_id"], "2026_11_10_16_15_06");
        assert_eq!(data["params"]["map_type"], 0, "巡检枚举 0 = 2D 栅格");
        assert_eq!(data["params"]["path_id"], "平台-TK-1");
        assert_eq!(
            data["params"]["end_task"], 1,
            "闭环且未指定结束动作 → 返回起点"
        );
        assert_eq!(data["params"]["min_recharge_battery"], 25.0);
        assert_eq!(data["params"]["speed_limit"]["vx"], 1.2);
        assert!(
            data["params"].get("goals").is_none(),
            "巡检请求不带坐标，航点在本体路径文件里"
        );
        // 官方 execute_at 是本地时间字符串，不是毫秒
        let ts = data["execute_at"].as_str().unwrap_or_default();
        assert_eq!(ts.len(), 19, "YYYY-MM-DD HH:MM:SS，实际 {ts:?}");
        assert!(ts.contains('-') && ts.contains(':') && ts.contains(' '));
    }

    #[test]
    fn patrol_enums_follow_patrol_vocabulary() {
        // kind=2 的路网用 3，巡检用 1；end_task 用数字枚举
        let mut p = plan("往返");
        p.map_type = 3;
        p.end_task = "返回充电".to_string();
        let data = patrol_task_payload(&p, "m", "p", 20.0);
        assert_eq!(data["params"]["map_type"], 1, "巡检路网 = 1（不是 3）");
        assert_eq!(data["params"]["end_task"], 2, "返回充电");

        p.end_task = "原地".to_string();
        let data = patrol_task_payload(&p, "m", "p", 20.0);
        assert_eq!(data["params"]["end_task"], 0, "原地待命");

        p.end_task = "返回起点".to_string();
        let data = patrol_task_payload(&p, "m", "p", 20.0);
        assert_eq!(data["params"]["end_task"], 1, "返回起点");

        p.map_type = 1;
        let data = patrol_task_payload(&p, "m", "p", 20.0);
        assert_eq!(data["params"]["map_type"], 0, "2D 栅格 = 0");
    }

    #[test]
    fn nav_path_points_carry_type_and_tasks() {
        let mut p = plan("往返");
        p.waypoints[0].dwell = 3;
        p.waypoints[0].actions = vec![WaypointAction {
            action_type: "可见光拍照".to_string(),
            ..Default::default()
        }];
        let data = add_nav_path_payload("m", "pid", &p.waypoints);
        let tuple = data["add_nav_path"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            tuple.len(),
            4,
            "add_nav_path = [map_id, path_id, points, fences]"
        );
        assert_eq!(tuple[0], json!("m"));
        assert_eq!(tuple[1], json!("pid"));
        assert_eq!(tuple[3], json!([]), "无禁区时传空数组");

        let points = tuple[2].as_array().cloned().unwrap_or_default();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0][0], json!("A"));
        // 有停留 + 拍照 → 作业点，tasks = [0 等待, 1 拍照]
        assert_eq!(points[0][1]["properties"]["type"], 1);
        assert_eq!(points[0][1]["properties"]["tasks"], json!([0, 1]));
        // 纯导航点 → 途径点，tasks 为空
        assert_eq!(points[1][1]["properties"]["type"], 0);
        assert_eq!(points[1][1]["properties"]["tasks"], json!([]));
        // 位姿来自统一的 pose()
        assert_eq!(points[0][1]["position"]["x"], 0.0);
        assert!(points[0][1]["orientation"]["w"].as_f64().is_some());
    }

    #[test]
    fn modify_nav_path_reuses_point_builder() {
        let p = plan("往返");
        let data = modify_nav_path_payload("m", "old", "new", &p.waypoints);
        let tuple = data["modify_nav_path"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            tuple.len(),
            5,
            "modify_nav_path = [map_id, old, new, points, fences]"
        );
        assert_eq!(tuple[1], json!("old"));
        assert_eq!(tuple[2], json!("new"));
        assert_eq!(tuple[3].as_array().map(|v| v.len()), Some(3));
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
    fn detects_photo_action() {
        let mut p = point("A", 0.0);
        assert!(!p.wants_photo());
        p.actions.push(WaypointAction {
            action_type: "可见光拍照".to_string(),
            ..Default::default()
        });
        assert!(p.wants_photo());
    }

    #[test]
    fn classifies_body_error_codes() {
        // 现场真实报文：5001 响应失败: code=13897 resource conflict (capability busy)
        let busy = "5001 响应失败: code=13897 resource conflict (capability busy)";
        assert!(is_capability_busy(busy));
        assert!(!is_task_not_found(busy));

        let missing = "5005 响应失败: code=13896 task not found";
        assert!(is_task_not_found(missing));
        assert!(!is_capability_busy(missing));

        // 普通失败不能误判成忙，否则会白等 30s
        assert!(!is_capability_busy(
            "5001 响应失败: code=13898 map not found or invalid"
        ));
    }
}
