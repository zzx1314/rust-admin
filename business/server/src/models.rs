use serde::{Deserialize, Deserializer, Serialize};
use sqlx::FromRow;

/// 区分「字段缺失」(None) 与「显式传 null」(Some(None))。
/// 更新接口用它识别「把路线清空为临时路径」这类操作。
pub fn double_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Park {
    pub id: i64,
    pub park_code: String,
    pub park_name: String,
    pub park_description: Option<String>,
    pub park_address: Option<String>,
    pub area: f64,
    pub owner_name: Option<String>,
    pub park_type: Option<String>,
    pub status: String,
    pub color: String,
    pub created_at: String,
    pub updated_at: String,
    pub map_version_id: Option<String>,
    pub map_status: String,
    pub zone_count: i64,
    pub device_count: i64,
    pub task_count: i64,
    pub done_count: i64,
    pub created_by: Option<i64>,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapVersion {
    pub id: String,
    pub park_id: i64,
    pub park_name: Option<String>,
    pub map_name: String,
    pub version: String,
    pub frame_id: Option<String>,
    pub crs: Option<String>,
    pub origin: Option<String>,
    pub resolution: Option<String>,
    pub floors: Option<String>,
    pub road_count: i64,
    pub no_go_count: i64,
    pub status: String,
    pub map_hash: Option<String>,
    pub map_width: Option<i64>,
    pub map_height: Option<i64>,
    pub note: Option<String>,
    pub published_at: Option<String>,
    pub created_at: String,
    pub source: String,
    pub source_id: Option<String>,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_size: Option<i64>,
    /// 中台地图归属用户（机器狗建图时的 owner）
    pub owner_user_id: Option<i64>,
    pub owner_username: Option<String>,
    /// 机器人本体地图路径，用于自动 / 手动同步去重
    pub source_ref: Option<String>,
    pub route_count: i64,
    pub task_count: i64,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapRefRoute {
    pub id: i64,
    pub route_code: String,
    pub route_name: String,
    pub status: String,
    pub version: String,
    pub point_count: i64,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapRefTask {
    pub id: i64,
    pub task_code: String,
    pub task_name: String,
    pub task_status: String,
    pub route_id: Option<i64>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapRefs {
    pub map: MapVersion,
    pub routes: Vec<MapRefRoute>,
    pub tasks: Vec<MapRefTask>,
    pub device_count: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MapRaster {
    pub map_id: String,
    pub image: String,
    pub resolution: Option<String>,
    pub origin: Option<String>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Zone {
    pub id: i64,
    pub park_id: i64,
    pub park_name: Option<String>,
    pub zone_name: String,
    pub zone_type: Option<String>,
    pub frequency: Option<String>,
    pub status: String,
    pub device_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub created_by: Option<i64>,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: i64,
    pub device_id: String,
    pub model: Option<String>,
    pub park_id: Option<i64>,
    pub park_name: Option<String>,
    pub zone_id: Option<i64>,
    pub zone_name: Option<String>,
    pub status: String,
    pub battery: i64,
    pub temperature: f64,
    pub runtime_minutes: i64,
    pub odometer: f64,
    pub position: Option<String>,
    pub current_task_name: Option<String>,
    pub last_heartbeat: Option<String>,
    pub firmware: Option<String>,
    pub device_ip: Option<String>,
    /// 实时视频地址（RTSP / HLS / FLV / WebRTC），由本体自带服务提供
    pub video_url: Option<String>,
    /// 视频协议类型：none / hls / flv / webrtc / rtsp / mp4
    pub video_type: String,
    /// 当前远程接管人；为空表示无人接管，遥控指令一律拒绝
    pub teleop_holder: Option<String>,
    /// 接管时间，用于超时自动释放
    pub teleop_at: Option<String>,
    /// 本体链路是否在线：0 表示客户端处于模拟模式 / 本体掉线，电量等数值不可信
    pub body_online: bool,
    /// 客户端工作模式：robot（接本体）/ sim（纯模拟）/ NULL（未上报）
    pub run_mode: Option<String>,
    /// 充电状态：本体 1315 上报（充电桩接入 / 电池电源状态），不是按电量猜的
    pub charging: bool,
    /// 本体工作模式：1315 `mode`（协议里的 MachineStatus），如 `remote`
    pub robot_mode: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: i64,
    pub task_code: String,
    pub task_name: String,
    pub task_type: Option<String>,
    pub execute_mode: Option<String>,
    pub level: Option<String>,
    pub device_id: Option<i64>,
    pub device_code: Option<String>,
    pub park_id: Option<i64>,
    pub park_name: Option<String>,
    pub zone_id: Option<i64>,
    pub zone_name: Option<String>,
    pub route_id: Option<i64>,
    pub route_code: Option<String>,
    pub route_name: Option<String>,
    pub plan_start: Option<String>,
    pub plan_end: Option<String>,
    pub task_status: String,
    pub progress: i64,
    pub dispatch_policy: Option<String>,
    pub description: Option<String>,
    /// 智航 Pro 本体侧任务标识
    pub robot_task_id: Option<String>,
    /// 智航 Pro 执行阶段（mapping.* / navigation.* / patrol.* / recharge.* …）
    pub robot_stage: Option<String>,
    pub robot_updated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub created_by: Option<i64>,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: i64,
    pub route_code: String,
    pub route_name: String,
    pub park_id: Option<i64>,
    pub park_name: Option<String>,
    pub zone_id: Option<i64>,
    pub zone_name: Option<String>,
    pub model: Option<String>,
    pub status: String,
    pub version: String,
    pub loop_mode: String,
    pub default_speed: f64,
    pub route_description: Option<String>,
    pub route_data: Option<String>,
    pub point_count: i64,
    pub distance_m: f64,
    pub duration_s: i64,
    pub battery_pct: i64,
    pub published_at: Option<String>,
    pub map_version_id: Option<String>,
    pub map_status: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub used_task_count: i64,
    pub created_by: Option<i64>,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub id: i64,
    pub alert_code: String,
    pub level: String,
    pub content: String,
    pub device_id: Option<i64>,
    pub device_code: Option<String>,
    pub park_id: Option<i64>,
    pub park_name: Option<String>,
    pub status: String,
    pub occurred_at: String,
    pub handled_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub id: i64,
    pub group_name: String,
    pub setting_key: String,
    pub setting_value: Option<String>,
    pub value_type: String,
    pub description: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub id: i64,
    pub event_type: String,
    pub title: String,
    pub description: Option<String>,
    pub device_code: Option<String>,
    pub task_code: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfo {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    /// 关联的中台账号 id；本地服务账号为空
    pub zentai_user_id: Option<i64>,
    pub zentai_username: Option<String>,
    pub is_admin: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub token: String,
    pub user: UserInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParkRequest {
    pub park_code: Option<String>,
    pub park_name: String,
    pub park_description: Option<String>,
    pub park_address: Option<String>,
    pub area: Option<f64>,
    pub owner_name: Option<String>,
    pub park_type: Option<String>,
    pub status: Option<String>,
    pub color: Option<String>,
    pub map_version_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneRequest {
    pub park_id: i64,
    pub zone_name: String,
    pub zone_type: Option<String>,
    pub frequency: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRequest {
    pub device_id: String,
    pub model: Option<String>,
    pub park_id: Option<i64>,
    pub zone_id: Option<i64>,
    pub status: Option<String>,
    pub battery: Option<i64>,
    pub temperature: Option<f64>,
    pub runtime_minutes: Option<i64>,
    pub odometer: Option<f64>,
    pub position: Option<String>,
    pub current_task_name: Option<String>,
    pub last_heartbeat: Option<String>,
    pub firmware: Option<String>,
    pub device_ip: Option<String>,
    pub video_url: Option<String>,
    pub video_type: Option<String>,
}

/// 手动把设备绑定到园区（可同时指定园区下的区域）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindParkRequest {
    pub park_id: i64,
    pub zone_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRequest {
    pub task_code: Option<String>,
    pub task_name: String,
    pub task_type: Option<String>,
    pub execute_mode: Option<String>,
    pub level: Option<String>,
    pub device_id: Option<i64>,
    pub park_id: Option<i64>,
    pub zone_id: Option<i64>,
    /// 更新时显式传 null 表示解除路线关联（临时路径）
    #[serde(default, deserialize_with = "double_option")]
    pub route_id: Option<Option<i64>>,
    pub plan_start: Option<String>,
    pub plan_end: Option<String>,
    pub task_status: Option<String>,
    pub progress: Option<i64>,
    pub dispatch_policy: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusRequest {
    pub task_status: String,
    pub progress: Option<i64>,
    /// 暂停 / 取消的原因，会透传给客户端
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRequest {
    pub route_code: Option<String>,
    pub route_name: String,
    pub park_id: Option<i64>,
    pub zone_id: Option<i64>,
    pub model: Option<String>,
    pub status: Option<String>,
    pub version: Option<String>,
    pub loop_mode: Option<String>,
    pub default_speed: Option<f64>,
    pub route_description: Option<String>,
    pub route_data: Option<serde_json::Value>,
    pub map_version_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapVersionRequest {
    pub note: Option<String>,
    pub map_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportBackendMapRequest {
    pub backend_map_id: i64,
    /// 目标园区；缺省时按中台地图的 parkCode 反查业务园区
    pub park_id: Option<i64>,
    #[serde(default)]
    pub bind: bool,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BackendMap {
    pub id: i64,
    pub map_name: String,
    /// 归属园区编码（业务库 `parks.park_code`，中台只存不校验）
    pub park_code: Option<String>,
    /// 归属园区名称（中台冗列存储）
    pub park_name: Option<String>,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_format: Option<String>,
    pub file_size: Option<i64>,
    pub remarks: Option<String>,
    pub map_version: Option<String>,
    pub frame_id: Option<String>,
    pub crs: Option<String>,
    pub map_origin: Option<String>,
    pub resolution: Option<String>,
    pub floors: Option<String>,
    pub road_count: Option<i64>,
    pub nogo_count: Option<i64>,
    pub map_status: Option<String>,
    pub map_hash: Option<String>,
    pub publish_time: Option<String>,
    pub map_width: Option<i64>,
    pub map_height: Option<i64>,
    pub create_time: Option<String>,
    /// 中台 `dog_map.user_id`，用于按登录用户过滤地图
    pub user_id: Option<i64>,
    pub owner_username: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapQuery {
    pub park_id: Option<i64>,
    pub status: Option<String>,
    pub source: Option<String>,
}

/// 中台地图库筛选：按业务园区编码 / 归属用户过滤
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendMapQuery {
    pub park_code: Option<String>,
    /// 管理员可按中台用户 id 过滤
    pub owner_user_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertRequest {
    pub alert_code: Option<String>,
    pub level: String,
    pub content: String,
    pub device_id: Option<i64>,
    pub park_id: Option<i64>,
    pub status: Option<String>,
    pub occurred_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingItem {
    pub setting_key: String,
    pub setting_value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRequest {
    pub command: String,
}

/// 遥控摇杆下发
///
/// `forward` / `strafe` / `turn` 取值 -1.0 ~ 1.0，全 0 表示停止。
/// `speed` 为 0.1 ~ 1.0 的档位系数，缺省按 1.0。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeleopRequest {
    #[serde(default)]
    pub forward: f64,
    #[serde(default)]
    pub strafe: f64,
    #[serde(default)]
    pub turn: f64,
    pub speed: Option<f64>,
}

impl TeleopRequest {
    pub fn speed_ratio(&self) -> f64 {
        self.speed.unwrap_or(1.0).clamp(0.1, 1.0)
    }

    /// 三轴全 0 即为停车指令
    pub fn is_stop(&self) -> bool {
        self.forward == 0.0 && self.strafe == 0.0 && self.turn == 0.0
    }

    pub fn clamped(&self) -> Self {
        Self {
            forward: self.forward.clamp(-1.0, 1.0),
            strafe: self.strafe.clamp(-1.0, 1.0),
            turn: self.turn.clamp(-1.0, 1.0),
            speed: self.speed,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceQuery {
    pub q: Option<String>,
    pub park_id: Option<i64>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskQuery {
    pub task_status: Option<String>,
    pub device_id: Option<i64>,
    pub park_id: Option<i64>,
    pub route_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlertQuery {
    pub status: Option<String>,
    pub level: Option<String>,
    pub park_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteQuery {
    pub q: Option<String>,
    pub park_id: Option<i64>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ZoneQuery {
    pub park_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParkQuery {
    pub q: Option<String>,
    pub status: Option<String>,
}
