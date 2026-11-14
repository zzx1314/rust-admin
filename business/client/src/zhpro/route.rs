//! 航线任务（巡检）执行引擎。
//!
//! 职责：把业务服务下发的任务（路线航点 + 动作 + 地图）翻译成智航 Pro 的任务，
//! 并按 5009 状态机推进、把进度回传业务服务。
//!
//! 两种下发形态（见 [`DispatchMode`]）：
//!
//! - **默认 `patrol`（`5000 kind=4`，与官方 App 一致）**：先把路线写成本体
//!   `nav_paths/<path_id>`（`4024` 添加 / `4028` 覆盖），再下发一条巡检任务；
//!   航点、到点动作、返程全部由本体 ALG 执行，客户端只跟 `5009 patrol.*`。
//!   字段依据 2026-11-14 抓到官方 App 的真实报文（见 [`patrol_task_payload`]）。
//! - **回退 `navigation`（`5000 kind=2`）**：旧实现，把航线拆成「导航 + 作业」：
//!   导航任务由本体执行（依次到达 goals），到点作业由客户端执行（等待 / 拍照）。
//!   注意：`.1` 的 `dog_task` 与 `.234` 的 `robots_dog_msgs` 版本不一致时这条路会崩。
//! - **一段连续航点只下发一条导航任务**（`5000 kind=2` 的 `params.goals` 是有序数组）：
//!   本体同一时刻只允许一个导航任务，逐点下发时上一个点还没结束就会把下一个点拒成
//!   `13897 capability busy`（现场：第 1 个点建任务成功，后面每点全 13897）。
//!   分段规则见 [`plan_segments`]：只在需要停点作业（停留/拍照/动作）的航点处断开。
//! - 暂停 = 5004 暂停当前导航任务；恢复 = 5006；取消 = 5002 + 停止剩余航点。
//! - 结束后按 `endTask` 走返航到起点 / 返回充电点（5014）/ 原地待命。

use super::client::ZhProClient;
use super::protocol::*;
use super::status::TaskSnapshot;
use robodog_device_client::config::DispatchMode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
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

    /// 这个点是否需要**停下来做作业**（停留 / 拍照 / 其它动作）。
    ///
    /// 只有这种点才必须独占一条导航任务的尾巴：本体跑到目标点即结束该任务，
    /// 客户端做完作业再发下一段。纯导航点全部并进同一条任务（见 [`plan_segments`]）。
    pub fn has_on_point_work(&self) -> bool {
        self.dwell > 0 || self.wants_photo() || !self.actions.is_empty()
    }
}

/// 把执行顺序切成若干「导航任务段」，一段对应一条 `5000 kind=2`。
///
/// 为什么要分段：本体同一时刻**只允许一个导航任务**，第二个 `5000` 会被回
/// `13897 resource conflict (capability busy)`。逐点下发（每点一条任务、点间不释放能力）
/// 会让第 2 个点起全部失败——现场 5 点航线的后 4 点就是这样全撞 13897 的。
///
/// 分段规则：
/// - 段尾是最后一个「需要停点作业」的航点（`dwell>0` / 拍照 / 其它动作）；
/// - 连续纯导航航点合并成同一段，`params.goals` 按顺序一次发给本体；
/// - 常见纯巡检航线（5 点、都不拍照）→ **1 段 = 1 个任务**，与官方 App 的下发方式一致。
pub(crate) fn plan_segments(points: &[Waypoint]) -> Vec<std::ops::Range<usize>> {
    let mut segments = Vec::new();
    let mut start = 0_usize;
    for (index, point) in points.iter().enumerate() {
        let last = index + 1 == points.len();
        if point.has_on_point_work() || last {
            segments.push(start..index + 1);
            start = index + 1;
        }
    }
    if start < points.len() {
        segments.push(start..points.len());
    }
    segments
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

/// 航点起步时是否「载图并等定位收敛」（5000 `params.need_locate`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locate {
    /// `need_locate=false`：复用当前已加载地图的定位（免重载、保住位姿）
    Reuse,
    /// `need_locate=true`：载图并等定位收敛（协议安全默认）
    Reload,
}

impl Locate {
    pub fn need_locate(self) -> bool {
        matches!(self, Locate::Reload)
    }
}

/// 起步预检（4030）的可复用窗口。
///
/// 预检到首个 `5000` 之间还夹着取控制权、`1302 cmd=62` 切导航模式、`5002` 收尾旧任务，
/// 期间地图可能被换掉（本体 App 抢权、算法节点重启），所以「刚载过图」只在短时间内可信。
pub const PRE_LOCATE_REUSE_WINDOW: Duration = Duration::from_secs(60);

/// 起步预检记录：绑地图 + 时刻，避免只留一个裸 bool——久等或换图之后它就不再可信了。
#[derive(Debug, Clone)]
struct PreLocated {
    map_path: String,
    at: Instant,
}

/// 判定某个航点的起步定位方式。
///
/// 只有「首点 且 预检不可信」才重新载图；同一条航线内其余点一律复用定位。
/// 结束动作「返回起点」也走复用：那时地图肯定还加载在本体上，再载一次只是白花
/// 10~30s，还会把位姿重置回「上一份位姿文件」。
pub fn locate_decision(
    is_first: bool,
    pre_located: Option<(&str, Duration)>,
    map_path: &str,
) -> Locate {
    if !is_first {
        return Locate::Reuse;
    }
    match pre_located {
        Some((path, elapsed)) if path == map_path && elapsed <= PRE_LOCATE_REUSE_WINDOW => {
            Locate::Reuse
        }
        _ => Locate::Reload,
    }
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

/// 错误串是否属于「定位 / 地图未就绪」。
///
/// 这类错误**不能按航点跳过**：本体不知道自己在地图哪 / 图没加载时，后面每个航点
/// 若还带着 `need_locate=false` 就会继续秒失败，最后把整条航线静默跑成 0/N
/// 却回报「巡检完成」（全部航点可跳过时尤其如此）。
pub fn is_locate_error(error: &str) -> bool {
    [
        "定位",          // 定位未就绪 / SLAM 定位失败 / 重新定位失败
        "need_locate",   // 本体起步守卫：位未就绪
        "map not found", // 13898 map not found or invalid
        "missing map",   // 13898 missing map.yaml
        "map.yaml",
        "未加载地图",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

/// 一段导航任务的 5000 `kind=2` 载荷。
///
/// `points` 是该段全部航点（协议 §2.5.1.4：`params.goals` 是**有序目标点数组**，
/// 至少一个；本体按数组顺序执行，到点发 `navigation.goal_arrived`）。
///
/// 字段名与协议严格对齐；下面几个取值都是踩过坑的：
///
/// - `speed_limit.vz` 是**角速度上限**，填 0 等于「不允许转向」，需要拐弯的航点会被规划器
///   判为「无有效绕障轨迹，搜索失败，等待中」；文档示例是 1.5。
/// - 没有电子围栏时**不下发** `fence_path`：以前发空串 `""`，固件可能拿它当路径打开，
///   失败就回「导航指令非法」。业务侧还没建模围栏，将来接入时再补这个键。
/// - `goal_type` 必须与 `map_type` 一致（1 = 2D 栅格 / 3 = 路网）。
pub(crate) fn navigation_task_payload(
    plan: &FlightPlan,
    points: &[Waypoint],
    need_locate: bool,
) -> Value {
    // 导航协议只支持 1（2D 栅格）和 3（Route 路网）；0 是 SDK 的
    // kGrid，2 是 3D 预留（当前固件不支持）。业务载荷偶尔会透传 0，
    // 这里统一归一化，避免把非法值发给本体。
    let map_type = if plan.map_type == 3 { 3 } else { 1 };
    let goals: Vec<Pose> = points.iter().map(Waypoint::pose).collect();
    let name = match points {
        [] => plan.task_name.clone(),
        [only] => format!("{} · 前往 {}", plan.task_name, only.name),
        [first, .., last] => format!(
            "{} · {} 个航点（{} → {}）",
            plan.task_name,
            points.len(),
            first.name,
            last.name
        ),
    };
    let mut params = json!({
        "map_path": plan.resolve_map_path(super::mapping::DEFAULT_MAP_ROOT),
        // 同一条航线内复用已加载定位，避免每段重载地图
        "need_locate": need_locate,
        "map_type": map_type,
        "goal_type": map_type,
        "goals": goals,
        "speed_limit": { "vx": speed_vx(plan), "vy": 0.0, "vz": YAW_LIMIT },
        "goal_tolerance": {
            "x": plan.goal_tolerance,
            "y": plan.goal_tolerance,
            "theta": 0.2
        }
    });
    // 有围栏才带 fence_path：空串会被固件当成非法路径（实测回“导航指令非法”）
    if let Some(fence) = plan.fence_path.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        params["fence_path"] = json!(fence);
    }
    json!({
        "kind": TaskKind::Navigation.code(),
        "name": name,
        "schedule": 0,
        "execute_at": now_ms().to_string(),
        "priority": 0,
        "record": false,
        "params": params
    })
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
    /// 起步前预检已经锁好定位：首航点不必再让本体重载地图（带地图与时刻，见 [`PreLocated`]）
    pre_located: Mutex<Option<PreLocated>>,
    total: Mutex<usize>,
    completed: Mutex<usize>,
    skipped: Mutex<usize>,
    /// 下发形态：巡检 `kind=4`（默认）/ 导航 `kind=2`
    mode: Mutex<DispatchMode>,
    /// 巡检路径名前缀（本体 `nav_paths/<前缀>-<task_code>`）
    patrol_path_prefix: Mutex<String>,
    /// 巡检任务返充电量阈值
    min_recharge_battery: Mutex<f64>,
}

impl RouteRunner {
    pub fn new(client: Arc<ZhProClient>) -> Self {
        Self {
            client,
            plan: Mutex::new(None),
            nav_task_id: Mutex::new(None),
            cancelled: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            pre_located: Mutex::new(None),
            total: Mutex::new(0),
            completed: Mutex::new(0),
            skipped: Mutex::new(0),
            mode: Mutex::new(DispatchMode::default()),
            patrol_path_prefix: Mutex::new("平台".to_string()),
            min_recharge_battery: Mutex::new(25.0),
        }
    }

    /// 设置下发形态（巡检 `kind=4` / 导航 `kind=2`）
    pub async fn set_mode(&self, mode: DispatchMode) {
        *self.mode.lock().await = mode;
    }

    /// 巡检相关参数：本体路径名前缀、返充电量阈值
    pub async fn set_patrol_options(&self, prefix: &str, min_recharge_battery: f64) {
        *self.patrol_path_prefix.lock().await = prefix.trim().to_string();
        *self.min_recharge_battery.lock().await = min_recharge_battery.clamp(0.0, 100.0);
    }

    /// 标记「定位已由起步预检完成」：首航点改用 `need_locate=false` 复用当前定位，
    /// 避免刚重定位完又让本体载一次图（多花 10~30s，且会重置位姿）。
    ///
    /// 记录地图路径与时刻：换图或隔得太久（[`PRE_LOCATE_REUSE_WINDOW`]）就不认这条记录。
    pub async fn mark_pre_located(&self, map_path: &str) {
        *self.pre_located.lock().await = Some(PreLocated {
            map_path: map_path.to_string(),
            at: Instant::now(),
        });
    }

    /// 本点的起步定位方式：首点读取（并消费）预检记录，其余点一律复用。
    async fn locate_for(&self, plan: &FlightPlan, is_first: bool) -> Locate {
        if !is_first {
            return Locate::Reuse;
        }
        let map_path = plan.resolve_map_path(super::mapping::DEFAULT_MAP_ROOT);
        let pre = self.pre_located.lock().await.take();
        let pre = pre.map(|item| (item.map_path, item.at.elapsed()));
        locate_decision(
            true,
            pre.as_ref()
                .map(|(path, elapsed)| (path.as_str(), *elapsed)),
            &map_path,
        )
    }

    pub async fn prepare(&self, plan: FlightPlan) {
        let order = plan.execution_order();
        *self.total.lock().await = order.len();
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

    /// 执行整条航线
    pub async fn run(&self) -> Result<ExecutionOutcome, String> {
        let plan = self
            .plan
            .lock()
            .await
            .clone()
            .ok_or_else(|| "未准备执行计划".to_string())?;
        let total = *self.total.lock().await;
        if *self.mode.lock().await == DispatchMode::Patrol {
            return self.run_patrol(&plan, total).await;
        }
        let points = plan.execution_order();
        let segments = plan_segments(&points);
        info!(
            "航线执行计划: {} 个航点 → {} 段导航任务（本体同一时刻只允许一个导航任务）",
            points.len(),
            segments.len()
        );

        // 0) 取 RoamerX 控制权 + 切导航模式：
        //    - 4044/5000 都在 4000~5999 非白名单区间，没控制权网关直接不转发；
        //    - 停在通用（手动）模式下本体把导航能力判给手动控制，5000 回 13897。
        if let Err(e) = super::teleop::enter_navigation_mode(&self.client).await {
            warn!("切导航模式失败（继续尝试下发，但本体可能拒任务）: {e}");
        }

        // 1) 避障等级：任务启动后必须重设，导航每次回到 STANDBY 会恢复为 40
        self.apply_avoid_level(plan.avoid_level).await;

        // 2) 按段执行：一段 = 一条 5000，段内多个 goals
        let mut offset = 0_usize;
        for (seg_index, range) in segments.iter().enumerate() {
            if self.is_cancelled() {
                return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
            }
            self.wait_while_paused().await;
            if self.is_cancelled() {
                return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
            }

            let seg_points = &points[range.clone()];
            let len = seg_points.len();
            // 只有首段可能重新载图定位；后续段（含「返回起点」）一律复用已加载的定位
            let locate = if seg_index == 0 {
                self.locate_for(&plan, true).await
            } else {
                Locate::Reuse
            };
            info!(
                "执行航段 {}/{}: 第 {}-{} 点（{} 个航点，need_locate={}）",
                seg_index + 1,
                segments.len(),
                range.start + 1,
                range.end,
                len,
                locate.need_locate()
            );
            match self
                .run_segment(&plan, seg_points, locate, offset, total)
                .await
            {
                Ok(()) => {
                    // 段尾那个点如果有停留/拍照，才在这里做作业
                    if let Some(last) = seg_points.last() {
                        self.perform_actions(last).await;
                    }
                    *self.completed.lock().await = offset + len;
                    offset += len;
                }
                Err(e) => {
                    if self.is_cancelled() {
                        return Ok(self.outcome(false, "任务已取消".to_string(), total).await);
                    }
                    // 段失败：先把本体上那条任务真正收尾（5002 + 等 idle），
                    // 否则下一段的 5000 会继续被回 13897。
                    self.abort_task().await;
                    // 排障用的诊断尾巴只在这里拼，**不要**拼进 `run_segment` 返回的
                    // `Err`：`is_locate_error` 只该看本体原话，而尾巴里有
                    // `need_locate=…` 这个针，拼进去会让**任何**航段失败都被判成
                    // 「定位错误」——可跳过的航点不再跳过，整条航线直接终止。
                    let detail = format!(
                        "第 {}/{total} 点起 {len} 个航点 need_locate={} vx={:.2} \
                         vz={YAW_LIMIT:.2} map={}",
                        range.start + 1,
                        locate.need_locate(),
                        speed_vx(&plan),
                        plan.resolve_map_path(super::mapping::DEFAULT_MAP_ROOT)
                    );
                    let failed = seg_points.last();
                    let failed_name = failed.map(|p| p.name.clone()).unwrap_or_default();
                    // 定位/地图类错误不可跳过：后面每段都会秒失败，
                    // 跳过只会把航线静默跑成 0/N。
                    if failed.map(Waypoint::skippable).unwrap_or(false) && !is_locate_error(&e) {
                        warn!("航段 {detail} 失败，跳过（段尾 {failed_name}）: {e}");
                        *self.skipped.lock().await += len;
                        offset += len;
                        continue;
                    }
                    return Ok(self
                        .outcome(
                            false,
                            format!("航点 {failed_name} 执行失败: {e}（{detail}）"),
                            total,
                        )
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
                // 5014 的 `map_id` 是本体目录名，不是业务地图 ID；用业务 ID 会被
                // 回 13898 map not found，导致「航线跑完了但没回充」。
                if let Err(e) = self
                    .client
                    .call(
                        T_RETURN_TO_CHARGE,
                        json!({ "map_id": plan.robot_map_dir() }),
                    )
                    .await
                {
                    warn!("返回充电点失败: {e}");
                }
            }
            EndTask::ReturnToStart => {
                // 闭环模式 `execution_order()` 已经把首点补在末尾，不再额外跑一次
                if plan.loop_mode != "闭环"
                    && let Some(first) = points.first()
                {
                    info!("巡检结束，返回起点");
                    // 复用定位：此刻地图肯定还加载在本体上，不需要再载一次
                    let tail = std::slice::from_ref(first);
                    if let Err(e) = self.run_segment(&plan, tail, Locate::Reuse, offset, total).await {
                        warn!("返回起点失败: {e}");
                        self.abort_task().await;
                    }
                }
            }
            EndTask::None => {
                info!("巡检结束，原地待命");
            }
        }

        *self.nav_task_id.lock().await = None;
        Ok(self.outcome(true, "巡检完成".to_string(), total).await)
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

        // 0) 控制权 + 导航模式：4044/5000 都在 4000~5999 非白名单区间，没控制权网关不转发
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
                let result = terminal_result(task);
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

    async fn wait_while_paused(&self) {
        while self.paused.load(Ordering::SeqCst) && !self.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// 下发一段导航任务（`5000 kind=2` + 该段多个 goals）并跟到终态。
    ///
    /// 段内只申请一次本体导航能力，所以不会再出现「上一个点还没结束就被拒」的 13897。
    /// 失败时统一走 [`Self::abort_task`]（5002 + 等 idle）再决定重试 / 上报，
    /// 避免本体上留着任务把下一段也挡住。
    async fn run_segment(
        &self,
        plan: &FlightPlan,
        points: &[Waypoint],
        locate: Locate,
        offset: usize,
        total: usize,
    ) -> Result<(), String> {
        let need_locate = locate.need_locate();
        let retries = points.iter().map(Waypoint::retry_limit).max().unwrap_or(0);
        let mut attempt = 0_u32;
        let mut busy_waits = 0_u32;
        loop {
            // 建任务前先确认本体没有别的任务占着能力：残留任务 / 上一段刚结束都算
            self.wait_body_idle(IDLE_WAIT).await;
            let data = navigation_task_payload(plan, points, need_locate);
            // 现场排障最有用的一行：把真实下发的载荷整份打出来（`RUST_LOG=...=debug`）
            debug!(
                "5000 载荷（第 {}-{} 点，{} 个航点，need_locate={need_locate}）: {data}",
                offset + 1,
                offset + points.len(),
                points.len()
            );

            let error = match self
                .create_segment_task(&data, points.len(), offset, total)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) => e,
            };
            if self.is_cancelled() {
                return Err("任务已取消".to_string());
            }

            // 失败：先把本体上那条任务收尾，让导航能力回到 idle
            self.abort_task().await;

            if is_capability_busy(&error) {
                // 13897：本体上还有别的任务占着能力，退避等待后重试，不消耗航段重试次数
                busy_waits += 1;
                if busy_waits > BUSY_RETRIES {
                    return Err(format!(
                        "{error}；本体上仍有未结束的任务（可能在远程接管/暂停态），\
                         请先在本体 APP 取消该任务或重启本体算法服务"
                    ));
                }
                warn!(
                    "本体能力忙（13897），{}s 后重试 {busy_waits}/{BUSY_RETRIES}",
                    BUSY_BACKOFF.as_secs()
                );
                tokio::time::sleep(BUSY_BACKOFF).await;
                continue;
            }
            // 定位/地图类错误重试没有意义（每次都会秒失败）
            if attempt < retries && !is_locate_error(&error) {
                attempt += 1;
                warn!(
                    "航段 第 {}-{} 点 第 {attempt} 次重试（共 {retries} 次）: {error}",
                    offset + 1,
                    offset + points.len()
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            // 只回本体原话：诊断信息（点位 / need_locate / 速度 / 地图）由 `run()` 拼，
            // 免得 `is_locate_error` 把尾巴里的 `need_locate=` 当成定位错误。
            return Err(error);
        }
    }

    /// 下发一次 `5000` 并跟到终态（不含重试逻辑，重试在 [`Self::run_segment`]）。
    async fn create_segment_task(
        &self,
        data: &Value,
        point_count: usize,
        offset: usize,
        total: usize,
    ) -> Result<(), String> {
        // 先订阅再下发：任务被本体秒拒（参数非法 / 能力忙）时，终态可能在上面的
        // `call` 返回后、订阅之前就推过来，漏掉就只能干等到超时。
        let mut rx = self.client.subscribe();
        let reply = self.client.call(T_TASK_CREATE, data.clone()).await?;
        debug!(
            "5001 应答（第 {}-{} 点）: {reply}",
            offset + 1,
            offset + point_count
        );
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
            "航段任务已受理: {nav_task_id} → 第 {}-{} 点（{point_count} 个航点）",
            offset + 1,
            offset + point_count
        );

        // 等待该任务终态
        let per_point = 180_u64;
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(per_point * point_count.max(1) as u64);
        // 同一 boot_id 内按 snapshot_seq 丢弃重复 / 更旧快照（文档 §2.6.10 第 2 条）：
        // 这条等待路径不经过 TaskMonitor，自己带一个单帧丢帧器。
        let mut gate = super::status::SeqGate::default();
        loop {
            if self.is_cancelled() {
                return Err("任务已取消".to_string());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(format!(
                    "第 {}-{} 点导航超时",
                    offset + 1,
                    offset + point_count
                ));
            }
            let env = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(env)) => env,
                Ok(Err(_)) => return Err("推送通道关闭".to_string()),
                Err(_) => {
                    return Err(format!(
                        "第 {}-{} 点导航超时",
                        offset + 1,
                        offset + point_count
                    ));
                }
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
                // 算法节点重启：本体上的导航任务已经不存在，等下去只能等到超时。
                return Err("算法节点重启（boot_id 变化），本体任务已失效".to_string());
            }
            let Some(task) = frame.task_for(&nav_task_id) else {
                continue;
            };
            // 到点进度：5009 `detail.last_arrived_goal.index` 是段内 1-based 序号
            if let Some(index) = task.detail_index.filter(|index| *index > 0) {
                let done = (offset + index as usize).min(total);
                let mut completed = self.completed.lock().await;
                if done > *completed {
                    *completed = done;
                }
            }
            if task.is_terminal() {
                let result = terminal_result(task);
                // 成功 → 清掉任务 ID；失败保留，让上层 abort_task 统一收尾
                if result.is_ok() {
                    *self.nav_task_id.lock().await = None;
                }
                return result;
            }
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
            road_network: false,
            fence_path: None,
        }
    }

    #[test]
    fn locate_only_reloads_on_first_point() {
        let map = "/ota/alg_data/map/history_map/M";
        // 没有预检：首点载图定位，其余点复用
        assert_eq!(locate_decision(true, None, map), Locate::Reload);
        assert_eq!(locate_decision(false, None, map), Locate::Reuse);
        // 预检刚落完这张图：首点也不重载
        assert_eq!(
            locate_decision(true, Some((map, Duration::ZERO)), map),
            Locate::Reuse
        );
        // 后续点：不管有没有预检记录都复用
        assert_eq!(
            locate_decision(false, Some((map, Duration::ZERO)), map),
            Locate::Reuse
        );
    }

    #[test]
    fn locate_ignores_stale_or_other_map_pre_locate() {
        let map = "/ota/alg_data/map/history_map/M";
        // 预检记录过期（预检到首个 5000 之间夹了太多步骤）：不再可信
        assert_eq!(
            locate_decision(
                true,
                Some((map, PRE_LOCATE_REUSE_WINDOW + Duration::from_secs(1))),
                map
            ),
            Locate::Reload
        );
        // 预检载的是另一张图（中途被换图）：不再可信
        assert_eq!(
            locate_decision(
                true,
                Some(("/ota/alg_data/map/history_map/OTHER", Duration::ZERO)),
                map
            ),
            Locate::Reload
        );
        // 结束动作「返回起点」传 is_first=false → 永远复用，不会多载一次图
        assert_eq!(
            locate_decision(
                false,
                Some(("/ota/alg_data/map/history_map/OTHER", Duration::ZERO)),
                map
            ),
            Locate::Reuse
        );
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
    fn locate_error_is_fatal_not_skippable() {
        assert!(Locate::Reload.need_locate());
        assert!(!Locate::Reuse.need_locate());

        // 定位/地图类错误：不能当成「可跳过的航点失败」
        assert!(is_locate_error(
            "5000 响应失败: code=13898 map not found or invalid"
        ));
        assert!(is_locate_error("起步守卫：位未就绪（定位未就绪）"));
        assert!(is_locate_error("SLAM 定位失败：定位丢失（status=122）"));
        // 本体导航状态机报的失败（``[step AwaitArrival] … robot reported Failed``）
        // 不是定位问题：曾因为诊断尾巴里带 `need_locate=` 被误判成定位错误，
        // 可跳过的航点直接把整条航线终止（skipped=0）。
        assert!(!is_locate_error(
            "导航失败: [step AwaitArrival] navigation aborted: robot reported Failed"
        ));
        // 其它错误照旧按航点策略处理
        assert!(!is_locate_error(
            "5001 响应失败: code=13897 resource conflict (capability busy)"
        ));
        assert!(!is_locate_error("航点 X 导航超时"));
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

    /// 5000 载荷必须与协议 §2.5.1.4 对齐：字段名、枚举值、不能为 0 的角速度上限。
    ///
    /// 这三个现场踩过的坑都锁在这里：`vz: 0`（禁止转向 → 规划失败）、
    /// 空串 `fence_path`（→ 导航指令非法）、`goal_type` 与 `map_type` 不一致。
    /// 分段规则：纯导航点合并成一段（= 一条 5000），只有需要停点作业的点才切段。
    ///
    /// 现场教训：逐点下发时本体同一时刻只允许一个导航任务，第 2 个点起全部
    /// 13897 capability busy。合并成一段后只申请一次能力，和官方 App 一致。
    #[test]
    fn segments_merge_pure_navigation_points() {
        let p = plan("往返");
        // 3 个纯导航点 → 1 段
        assert_eq!(plan_segments(&p.waypoints), vec![0..3]);

        // 中间点要停留 → 在它之后切段
        let mut dwell_plan = plan("往返");
        dwell_plan.waypoints[1].dwell = 10;
        assert_eq!(plan_segments(&dwell_plan.waypoints), vec![0..2, 2..3]);

        // 末点有作业 → 仍然只切在末尾（整条航线还是 1 段）
        let mut photo_plan = plan("往返");
        photo_plan.waypoints[2].actions.push(WaypointAction {
            action_type: "可见光拍照".to_string(),
            ..Default::default()
        });
        assert_eq!(plan_segments(&photo_plan.waypoints), vec![0..3]);

        // 每个点都要作业 → 每点一段（退化成原来的逐点模式，但段间会 5002 收尾）
        let mut all_work = plan("往返");
        for w in &mut all_work.waypoints {
            w.dwell = 5;
        }
        assert_eq!(plan_segments(&all_work.waypoints), vec![0..1, 1..2, 2..3]);

        // 空航线
        assert!(plan_segments(&[]).is_empty());
    }

    /// 一段 5000 的 `params.goals` 必须按顺序带全部航点（协议 §2.5.1.4）。
    #[test]
    fn navigation_payload_carries_all_goals_of_a_segment() {
        let p = plan("往返");
        let data = navigation_task_payload(&p, &p.waypoints, false);
        let goals = data["params"]["goals"].as_array().cloned().unwrap_or_default();
        assert_eq!(goals.len(), 3, "整段航点必须一次下发: {data}");
        assert_eq!(goals[0]["position"]["x"], 0.0);
        assert_eq!(goals[1]["position"]["x"], 1.0);
        assert_eq!(goals[2]["position"]["x"], 2.0);
        assert!(
            data["name"].as_str().unwrap_or_default().contains("3 个航点"),
            "任务名要能看出这是一整段: {data}"
        );
    }

    #[test]
    fn navigation_payload_matches_protocol() {
        let p = plan("往返");
        let data = navigation_task_payload(&p, &p.waypoints[..1], false);
        assert_eq!(data["kind"], 2, "kind=2 是单点/多点导航");
        assert_eq!(data["schedule"], 0);
        assert!(
            data["execute_at"].is_string(),
            "execute_at 是毫秒时间戳字符串"
        );

        let params = &data["params"];
        assert_eq!(params["map_type"], 1.0);
        assert_eq!(params["goal_type"], 1.0, "goal_type 必须与 map_type 一致");
        assert_eq!(params["need_locate"], false);
        assert_eq!(params["goals"].as_array().map(Vec::len), Some(1));
        assert!(
            params.get("fence_path").is_none(),
            "没配围栏时不应下发空串 fence_path"
        );
        assert_eq!(params["map_type"], 1.0);
        assert_eq!(params["goal_tolerance"]["x"], 0.25);
        assert_eq!(params["goal_tolerance"]["theta"], 0.2);
        let vz = params["speed_limit"]["vz"].as_f64().unwrap_or_default();
        assert!(vz > 0.0, "speed_limit.vz 是角速度上限，0 会禁止转向: {vz}");
        assert!(params["speed_limit"]["vx"].as_f64().unwrap_or_default() > 0.0);
    }

    #[test]
    fn navigation_payload_carries_road_network_and_fence() {
        let mut p = plan("往返");
        p.map_type = 3;
        p.fence_path = Some("/ota/alg_data/map/history_map/fence.geojson".to_string());
        let data = navigation_task_payload(&p, &p.waypoints[..1], false);
        let params = &data["params"];
        assert_eq!(params["map_type"], 3.0);
        assert_eq!(params["goal_type"], 3.0, "路网导航 goal_type 必须同步为 3");
        assert_eq!(
            params["fence_path"],
            "/ota/alg_data/map/history_map/fence.geojson"
        );

        // 空串/空白不能下发，否则固件会回“导航指令非法”
        p.fence_path = Some("   ".to_string());
        let data = navigation_task_payload(&p, &p.waypoints[..1], false);
        assert!(data["params"].get("fence_path").is_none());
    }

    #[test]
    fn navigation_payload_normalizes_invalid_map_type() {
        // SDK 的 0=kGrid 和 3D 的 2 都不能直接进导航协议；统一回退 2D 栅格。
        let mut p = plan("往返");
        for invalid in [0_u32, 2_u32] {
            p.map_type = invalid;
            let data = navigation_task_payload(&p, &p.waypoints[..1], false);
            assert_eq!(data["params"]["map_type"], 1.0, "invalid={invalid}");
            assert_eq!(data["params"]["goal_type"], 1.0, "invalid={invalid}");
        }
    }
}
