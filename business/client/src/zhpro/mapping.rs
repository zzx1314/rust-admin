//! 地图构建与地图管理。
//!
//! 覆盖：
//! - 5000 `kind=1` 建图任务（`params.map_type` 0 室外 / 2 室内）
//! - 5009 `mapping.preparing|running|saving|success|error` 阶段跟踪
//! - 4035 建图位姿与里程遥测
//! - 4001 建图保存结果（无论成功失败都会推送一次）
//! - 4012/4013 地图列表、4014/4015 目录改名、4016/4017 删除地图
//! - 4018/4019 设置回充点、4036/4037 删除回充点
//! - 4030/4031 载图定位（导航与巡检的前置动作）
//!
//! 地图管理的增删改查与载图定位属于运维能力，不一定被主流程调用，
//! 因此本模块整体允许 dead_code，保留完整接口以便按文档逐条对接。

#![allow(dead_code)]

use super::client::ZhProClient;
use super::protocol::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// 建图遥测（4035）
#[derive(Debug, Clone, Default)]
pub struct MappingTelemetry {
    pub pose: Option<Pose>,
    pub mileage: f64,
    pub remain_mileage: f64,
    pub received_at_ms: i64,
}

/// 建图结果（4001）
///
/// 4001 是建图收尾的唯一权威信号，无论成败都推送一次。成功时载荷里带有
/// 新地图的落盘路径，这是后续「地图回传中台」的唯一来源，因此必须完整保留。
#[derive(Debug, Clone)]
pub struct MappingSaved {
    pub success: bool,
    pub code: i64,
    pub message: String,
    /// 新地图目录的绝对路径（如 `/ota/alg_data/map/history_map/map_20261110_2312`）
    pub map_path: Option<String>,
    /// 目录名，等价于机器人侧的地图 ID
    pub map_name: Option<String>,
    /// 原始载荷，便于排障与补字段
    pub raw: serde_json::Value,
}

impl MappingSaved {
    /// 地图目录名：优先取显式字段，否则从路径末段推导
    pub fn dir_name(&self) -> Option<String> {
        self.map_name
            .clone()
            .or_else(|| {
                self.map_path
                    .as_deref()
                    .and_then(|path| path.trim_end_matches('/').rsplit('/').next())
                    .map(str::to_string)
            })
            .filter(|name| !name.is_empty())
    }
}

/// 从 4001 载荷中挑出地图路径（文档不同小版本字段名略有差异，按优先级探测）
fn extract_map_path(data: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 5] = [
        "map_path",
        "map_dir",
        "map_path_name",
        "mapPath",
        "dir_path",
    ];
    for key in KEYS {
        if let Some(value) = data.get(key).and_then(|v| v.as_str())
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_string());
        }
    }
    // 有些版本把结果包在 result / data 里
    for wrapper in ["result", "data"] {
        if let Some(inner) = data.get(wrapper)
            && let Some(found) = extract_map_path(inner)
        {
            return Some(found);
        }
    }
    None
}

/// 从 4001 载荷中挑出地图名
fn extract_map_name(data: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 3] = ["map_name", "map_id", "name"];
    for key in KEYS {
        if let Some(value) = data.get(key).and_then(|v| v.as_str())
            && !value.trim().is_empty()
        {
            return Some(value.trim().to_string());
        }
    }
    for wrapper in ["result", "data"] {
        if let Some(inner) = data.get(wrapper)
            && let Some(found) = extract_map_name(inner)
        {
            return Some(found);
        }
    }
    None
}

/// 机器人上的一张地图目录（4013 的 map_path_list 元素）
#[derive(Debug, Clone)]
pub struct RobotMap {
    /// 目录真实绝对路径 `<map_dir_prefix>/history_map/<目录名>`
    pub path: String,
    /// 目录名（地图 ID，4018/5014 用）
    pub map_id: String,
    /// 是否存在 map.geojson，决定是否支持路网导航
    pub route_exist: bool,
    /// 是否已设置可用回充点
    pub charge_point_exist: bool,
}

impl RobotMap {
    fn from_value(value: &serde_json::Value) -> Option<Self> {
        let path = value.get("path").and_then(|v| v.as_str())?.to_string();
        // 目录名即地图 ID：`<prefix>/history_map/<目录名>`
        let map_id = path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if map_id.is_empty() {
            return None;
        }
        Some(Self {
            path,
            map_id,
            route_exist: value
                .get("map_route_exist")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            charge_point_exist: value
                .get("charge_point_exist")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }

    /// 是否同时具备路网与回充点，可直接跑巡检
    pub fn patrol_ready(&self) -> bool {
        self.route_exist && self.charge_point_exist
    }
}

/// 建图会话：下发 5000 kind=1 并跟踪到 4001 保存结果
pub struct MappingSession {
    client: Arc<ZhProClient>,
    task_id: Mutex<Option<String>>,
    telemetry: Mutex<MappingTelemetry>,
    saved: Mutex<Option<MappingSaved>>,
    started_at_ms: AtomicI64,
}

impl MappingSession {
    pub fn new(client: Arc<ZhProClient>) -> Self {
        Self {
            client,
            task_id: Mutex::new(None),
            telemetry: Mutex::new(MappingTelemetry::default()),
            saved: Mutex::new(None),
            started_at_ms: AtomicI64::new(0),
        }
    }

    /// 下发建图任务，返回 5001 返回的 task_id
    pub async fn start(&self, name: &str, kind: MappingType) -> Result<String, String> {
        let data = serde_json::json!({
            "kind": TaskKind::Mapping.code(),
            "name": name,
            "schedule": 0,
            "execute_at": now_ms().to_string(),
            "priority": 0,
            "record": false,
            "params": { "map_type": kind.code() }
        });
        let reply = self.client.call(T_TASK_CREATE, data).await?;
        let task_id = reply
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if task_id.is_empty() {
            return Err("建图任务创建成功但未返回 task_id".to_string());
        }
        *self.task_id.lock().await = Some(task_id.clone());
        self.started_at_ms.store(now_ms(), Ordering::Relaxed);
        info!("建图任务已受理: task_id={task_id} 类型={:?}", kind);
        Ok(task_id)
    }

    pub async fn task_id(&self) -> Option<String> {
        self.task_id.lock().await.clone()
    }

    /// 处理 4035 建图遥测
    pub async fn on_telemetry(&self, data: &serde_json::Value) {
        let mut telemetry = self.telemetry.lock().await;
        if let Some(slam) = data.get("slam_odom_pose") {
            telemetry.pose = Some(Pose {
                position: Vector3 {
                    x: f64_at(slam, "position", "x"),
                    y: f64_at(slam, "position", "y"),
                    z: f64_at(slam, "position", "z"),
                },
                orientation: Quaternion {
                    x: f64_at(slam, "orientation", "x"),
                    y: f64_at(slam, "orientation", "y"),
                    z: f64_at(slam, "orientation", "z"),
                    w: {
                        let w = f64_at(slam, "orientation", "w");
                        if w == 0.0 { 1.0 } else { w }
                    },
                },
            });
        }
        telemetry.mileage = data
            .get("mileage")
            .and_then(|v| v.as_f64())
            .unwrap_or(telemetry.mileage);
        telemetry.remain_mileage = data
            .get("remain_mileage")
            .and_then(|v| v.as_f64())
            .unwrap_or(telemetry.remain_mileage);
        telemetry.received_at_ms = now_ms();
    }

    pub async fn telemetry(&self) -> MappingTelemetry {
        self.telemetry.lock().await.clone()
    }

    /// 处理 4001 保存结果（一次建图会话只记录第一次）
    pub async fn on_map_saved(&self, data: &serde_json::Value) -> Option<MappingSaved> {
        let mut slot = self.saved.lock().await;
        if slot.is_some() {
            return None;
        }
        let success = data.get("code").and_then(|v| v.as_i64()) == Some(0);
        let saved = MappingSaved {
            success,
            code: data.get("code").and_then(|v| v.as_i64()).unwrap_or(-1),
            message: data
                .get("msg")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            map_path: extract_map_path(data),
            map_name: extract_map_name(data),
            raw: data.clone(),
        };
        if success {
            match (&saved.map_path, &saved.map_name) {
                (Some(path), _) => info!("地图保存成功: {} → {path}", saved.message),
                (None, Some(name)) => {
                    warn!("地图保存成功但未返回路径，仅有名称 {name}，将按 map_root 推导");
                }
                (None, None) => {
                    warn!("地图保存成功但载荷中没有地图路径/名称，无法自动回传中台");
                }
            }
        } else {
            warn!("地图保存失败: code={} {}", saved.code, saved.message);
        }
        *slot = Some(saved.clone());
        Some(saved)
    }

    pub async fn saved_result(&self) -> Option<MappingSaved> {
        self.saved.lock().await.clone()
    }

    pub async fn reset(&self) {
        *self.task_id.lock().await = None;
        *self.saved.lock().await = None;
        *self.telemetry.lock().await = MappingTelemetry::default();
        self.started_at_ms.store(0, Ordering::Relaxed);
    }

    /// 停止建图：取消任务后等 4001 收尾（最多 `timeout`）
    pub async fn stop(&self, timeout: Duration) -> Option<MappingSaved> {
        let task_id = self.task_id().await?;
        if let Err(e) = self
            .client
            .call(T_TASK_CANCEL, serde_json::json!({ "task_id": task_id }))
            .await
        {
            warn!("取消建图任务失败: {e}");
        }
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            if let Some(saved) = self.saved_result().await {
                return Some(saved);
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        warn!("等待 4001 地图保存结果超时");
        None
    }
}

fn f64_at(value: &serde_json::Value, outer: &str, inner: &str) -> f64 {
    value
        .get(outer)
        .and_then(|v| v.get(inner))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}

/// 地图管理 API
pub struct MapManager {
    client: Arc<ZhProClient>,
}

impl MapManager {
    pub fn new(client: Arc<ZhProClient>) -> Self {
        Self { client }
    }

    /// 4012/4013 获取地图列表
    pub async fn list(&self) -> Result<Vec<RobotMap>, String> {
        let data = self
            .client
            .call(T_MAP_LIST, serde_json::Value::Null)
            .await?;
        let prefix = data
            .get("map_dir_prefix")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let maps: Vec<RobotMap> = data
            .get("map_path_list")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().filter_map(RobotMap::from_value).collect())
            .unwrap_or_default();
        info!("机器人现有地图 {} 张（根目录 {prefix}）", maps.len());
        Ok(maps)
    }

    /// 按地图 ID（目录名）查一张地图
    pub async fn find(&self, map_id: &str) -> Result<Option<RobotMap>, String> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .find(|map| map.map_id == map_id))
    }

    /// 按候选目录名/路径解析出本体上真实存在的地图。
    ///
    /// 业务侧的 `mapId`（`ROS-MAP-…@v1.0`）**不是**本体目录名：目录名由建图会话决定，
    /// 平台只能从设备回传的 `source_ref` 里拿到真实目录。这里按「服务端给的目录名 →
    /// 服务端给的路径 → 业务 ID 合成名」的顺序在 4013 列表里匹配，
    /// 全部对不上就报错——调用方必须拒绝下发，否则 5000 会指向一个空目录，
    /// 本体只会回 `13898 map not found or invalid (missing map.yaml)`。
    pub async fn resolve(&self, candidates: &[&str]) -> Result<RobotMap, String> {
        let maps = self.list().await?;
        let mut tried: Vec<String> = Vec::new();
        for candidate in candidates {
            let Some(name) = candidate_dir_name(candidate) else {
                continue;
            };
            if tried.contains(&name) {
                continue;
            }
            if let Some(map) = maps.iter().find(|map| map.map_id == name) {
                return Ok(map.clone());
            }
            tried.push(name);
        }

        let mut available: Vec<String> = maps.iter().map(|map| map.map_id.clone()).collect();
        available.sort();
        let shown = if available.is_empty() {
            "（本体上一张地图都没有）".to_string()
        } else if available.len() > 8 {
            format!("{}, …共 {} 张", available[..8].join(", "), available.len())
        } else {
            available.join(", ")
        };
        Err(format!(
            "本体上没有该地图目录（尝试过：{}）；本体现有目录：{}。\
             请先在平台「设备管理 → 同步地图」把该地图回传（或确认地图基线里登记的本体目录名），再重试",
            tried.join(" / "),
            shown
        ))
    }

    /// 4014/4015 修改地图目录名
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        self.client
            .call(T_MAP_RENAME, serde_json::json!({ "from": from, "to": to }))
            .await
            .map(|_| ())
    }

    /// 4016/4017 删除地图（`map_id` 为目录名数组）
    pub async fn delete(&self, map_ids: &[String]) -> Result<(), String> {
        if map_ids.is_empty() {
            return Ok(());
        }
        self.client
            .call(T_MAP_DELETE, serde_json::json!({ "map_id": map_ids }))
            .await
            .map(|_| ())
    }

    /// 4018/4019 设置返航回充点位（幂等覆盖）
    pub async fn set_charge_point(&self, map_id: &str, pose: &Pose) -> Result<(), String> {
        if pose.orientation.is_zero() {
            return Err("回充点姿态四元数不能为零".to_string());
        }
        self.client
            .call(
                T_SET_CHARGE_POINT,
                serde_json::json!({ "map_id": map_id, "pose": pose }),
            )
            .await
            .map(|_| ())
    }

    /// 4036/4037 删除返航回充点位（幂等）
    pub async fn delete_charge_point(&self, map_id: &str) -> Result<(), String> {
        self.client
            .call(
                T_DELETE_CHARGE_POINT,
                serde_json::json!({ "map_id": map_id }),
            )
            .await
            .map(|_| ())
    }
}

/// 默认地图根目录（L2 为 `/userdata/alg_data/map`，M1 为 `/ota/alg_data/map`）：
/// 4013 未返回 `map_dir_prefix` 时的兜底值。
/// 默认地图根目录统一定义在 `config` 模块，避免与命令行默认值漂移
pub use robodog_device_client::config::DEFAULT_MAP_ROOT;

/// 地图名（目录名）与业务侧地图版本的映射约定
///
/// 智航侧只认目录名，业务侧使用 `MAP-ZJ-01@v2.1` 形式的版本号。
/// 这里把两边做一层可逆映射：去掉 `@`/空格等不宜做目录名的字符，保留下划线。
pub fn to_map_dir_name(business_map_id: &str) -> String {
    business_map_id
        .chars()
        .map(|c| match c {
            '@' | '/' | '\\' | ' ' => '_',
            other => other,
        })
        .collect()
}

/// 载图定位前置检查：从 5009 心跳里读到的定位状态码给出可读描述。
///
/// 定位状态枚举（§2.6.7）：0 Idle / 1 Standby / 100 Locating / 120 MapLoading /
/// 121 Relocating / 122 Lost / 203 Error / 210 Fail / 255 Unknown。
pub fn describe_loc_status(status: i64) -> &'static str {
    match status {
        0 => "空闲（未载图）",
        1 => "就绪（可加载地图）",
        100 => "定位正常",
        120 => "加载地图中",
        121 => "重定位中",
        122 => "定位丢失",
        203 => "定位异常",
        210 => "定位失败",
        _ => "未知（尚未收到定位状态）",
    }
}

/// 4038/4039 获取机器人时间，用于校准本地与本体时钟差（故障排查与日志对账）
pub async fn robot_time_offset_ms(client: &ZhProClient) -> Result<i64, String> {
    let sent_at = now_ms();
    let data = client.call(T_ROBOT_TIME, serde_json::json!({})).await?;
    let robot_ms = data
        .get("timestamp_ms")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or_else(|| "4039 未返回 timestamp_ms".to_string())?;
    Ok(robot_ms - sent_at)
}

/// `4031` 回执等待上限
///
/// 现场固件的 `4031` 经常比这更慢（甚至只靠 `5009 loc_state` 表达进度），所以这个
/// 窗口只用来区分「被明确拒绝」和「还在处理」，**不能拿来判失败**。
pub const LOCATE_ACK_WAIT: Duration = Duration::from_secs(3);

/// 4030/4031 载图并定位。
///
/// `initial_pose` 为 None 时等价于下发零四元数，由 SLAM 读取上一份位姿文件做完整重定位。
///
/// 返回 `Ok(None)` 表示本体没在 [`LOCATE_ACK_WAIT`] 内回 `4031`（继续等 5009 即可），
/// `Ok(Some(data))` 表示 `4031` 已明确受理。
pub async fn load_map(
    client: &ZhProClient,
    map_path: &str,
    initial_pose: Option<Pose>,
) -> Result<Option<serde_json::Value>, String> {
    let pose = initial_pose.unwrap_or_default();
    client
        .call_ack_optional(
            T_LOCATE,
            serde_json::json!({
                "op": 1,
                "map_path": map_path,
                "initial_pose": pose
            }),
            LOCATE_ACK_WAIT,
        )
        .await
}

/// 候选目录名/路径 → 目录名（4013 `path` 的最后一段）
///
/// 服务端可能给 `mapDir`（目录名）也可能给 `mapPath`（绝对路径），两者都归一成目录名。
pub fn candidate_dir_name(candidate: &str) -> Option<String> {
    let name = candidate
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// 地图目录：`<map_root>/history_map/<map_dir>`
///
/// 与 `route::FlightPlan::resolve_map_path` 的推导一致，供「重新定位」这类
/// 只有 mapId、没有路线载荷的场景复用。
pub fn default_map_path(map_root: &str, map_id: &str) -> String {
    format!(
        "{}/history_map/{}",
        map_root.trim_end_matches('/'),
        to_map_dir_name(map_id)
    )
}

/// 定位已收敛：可以直接起步
pub fn loc_status_ready(status: i64) -> bool {
    status == 100
}

/// SLAM 已明确放弃本次定位（继续等待没有意义，需要人工介入）
pub fn loc_status_fatal(status: i64) -> bool {
    matches!(status, 203 | 210)
}

/// 读最近一帧 5009 的定位状态，最多等 `wait`
///
/// 用于起步前预检：先确认是否已经定位好，避免无谓地重载地图。
pub async fn latest_loc_status(client: &ZhProClient, wait: Duration) -> Option<i64> {
    let mut rx = client.subscribe();
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let Ok(Ok(env)) = tokio::time::timeout(remaining, rx.recv()).await else {
            return None;
        };
        if env.head.msg_type == T_TASK_STATUS
            && let Some(frame) = super::status::StatusFrame::parse(&env.data)
        {
            return Some(frame.loc_status);
        }
    }
}

/// 等定位收敛到 `100 Locating`。
///
/// - 超时 → 返回超时错误（附最后一次状态的可读描述）
/// - SLAM 明确失败（`203 Error` / `210 Fail`）→ 立即返回，不再干等
pub async fn wait_localized(client: &ZhProClient, timeout: Duration) -> Result<i64, String> {
    let mut rx = client.subscribe();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = 255_i64;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "等待定位收敛超时（{}s），当前状态：{}（status={last}）",
                timeout.as_secs(),
                describe_loc_status(last)
            ));
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(env)) => {
                if env.head.msg_type != T_TASK_STATUS {
                    continue;
                }
                let Some(frame) = super::status::StatusFrame::parse(&env.data) else {
                    continue;
                };
                last = frame.loc_status;
                if loc_status_ready(last) {
                    return Ok(last);
                }
                if loc_status_fatal(last) {
                    return Err(format!(
                        "SLAM 定位失败：{}（status={last}）",
                        describe_loc_status(last)
                    ));
                }
            }
            Ok(Err(_)) => return Err("推送通道关闭，无法确认定位状态".to_string()),
            // 本帧间隔内没有新状态：回循环顶部检查 deadline
            Err(_) => continue,
        }
    }
}

/// 载图 + 定位（4030），并等定位收敛。
///
/// `initial_pose` 为 `None` 时下发零四元数，由 SLAM 读取上一份位姿文件做完整重定位，
/// 适合「设备还在原位、只是没载图」；设备被搬动过时由平台显式给初始位姿。
///
/// 两个约束（都踩过）：
/// 1. `4030` 在 4000~5999 非白名单区间内，**必须先拿 RoamerX 控制权**，否则 gateway
///    直接不转发（以前表现为「等待 4031 响应超时」，本体那边却什么都没做）；
/// 2. `4031` 只是受理回执，真正的判据是 `5009 loc_state` 收敛到 100，因此回执慢/丢
///    不得判失败。
pub async fn relocalize(
    client: &ZhProClient,
    map_path: &str,
    initial_pose: Option<Pose>,
    timeout: Duration,
) -> Result<i64, String> {
    client.ensure_control().await?;
    match load_map(client, map_path, initial_pose).await? {
        Some(_) => info!("4030 已受理（4031 回执），等待定位收敛 map={map_path}"),
        None => warn!(
            "4030 未在 {}s 内收到 4031 回执，继续等 5009 定位状态 map={map_path}",
            LOCATE_ACK_WAIT.as_secs()
        ),
    }
    wait_localized(client, timeout).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_map_path_matches_flight_plan() {
        assert_eq!(
            default_map_path("/ota/alg_data/map", "MAP-ZJ-01@v2.1"),
            "/ota/alg_data/map/history_map/MAP-ZJ-01_v2.1"
        );
        // 尾部斜杠不应产生双斜杠
        assert_eq!(
            default_map_path("/ota/alg_data/map/", "A@1"),
            "/ota/alg_data/map/history_map/A_1"
        );
    }

    #[test]
    fn candidate_dir_name_normalizes_dirs_and_paths() {
        // 服务端可能给目录名（source_ref basename）也可能给绝对路径，都要归一成目录名
        assert_eq!(
            candidate_dir_name("2026_11_10_16_15_06").as_deref(),
            Some("2026_11_10_16_15_06")
        );
        assert_eq!(
            candidate_dir_name("/ota/alg_data/map/history_map/2026_11_10_16_15_06/").as_deref(),
            Some("2026_11_10_16_15_06")
        );
        assert_eq!(candidate_dir_name("  "), None);
    }

    #[test]
    fn loc_status_gate_classification() {
        assert!(loc_status_ready(100));
        assert!(!loc_status_ready(121));
        assert!(loc_status_fatal(203));
        assert!(loc_status_fatal(210));
        // 122 定位丢失还有救，不应当终止等待
        assert!(!loc_status_fatal(122));
        assert_eq!(describe_loc_status(122), "定位丢失");
    }

    #[test]
    fn map_dir_name_is_safe() {
        assert_eq!(to_map_dir_name("MAP-ZJ-01@v2.1"), "MAP-ZJ-01_v2.1");
        assert_eq!(to_map_dir_name("A/B C"), "A_B_C");
    }

    #[test]
    fn parses_map_list_item() {
        let item = serde_json::json!({
            "path": "/ota/alg_data/map/history_map/2026_06_12_03_10_49",
            "map_route_exist": true,
            "charge_point_exist": false
        });
        let map = RobotMap::from_value(&item).unwrap();
        assert_eq!(map.map_id, "2026_06_12_03_10_49");
        assert!(map.route_exist);
        assert!(!map.charge_point_exist);
    }
}
