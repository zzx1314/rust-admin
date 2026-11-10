use serde::{Deserialize, Serialize};
use sqlx::FromRow;

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
    pub signal: i64,
    pub temperature: f64,
    pub runtime_minutes: i64,
    pub odometer: f64,
    pub position: Option<String>,
    pub current_task_name: Option<String>,
    pub last_heartbeat: Option<String>,
    pub firmware: Option<String>,
    pub device_ip: Option<String>,
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
    pub created_at: String,
    pub updated_at: String,
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
    pub signal: Option<i64>,
    pub temperature: Option<f64>,
    pub runtime_minutes: Option<i64>,
    pub odometer: Option<f64>,
    pub position: Option<String>,
    pub current_task_name: Option<String>,
    pub last_heartbeat: Option<String>,
    pub firmware: Option<String>,
    pub device_ip: Option<String>,
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
    pub route_id: Option<i64>,
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
    pub park_id: i64,
    #[serde(default)]
    pub bind: bool,
}

#[derive(Debug, FromRow, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BackendMap {
    pub id: i64,
    pub map_name: String,
    pub park_id: Option<i64>,
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MapQuery {
    pub park_id: Option<i64>,
    pub status: Option<String>,
    pub source: Option<String>,
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
