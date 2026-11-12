use serde::{Deserialize, Serialize};
use serde_json::Value;

/// WebSocket 消息信封
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WsMessage {
    /// 消息类型标识
    pub msg_type: String,
    /// 时间戳
    pub ts: String,
    /// 业务载荷
    pub payload: Value,
}

/// 客户端 → 服务端：设备注册
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterPayload {
    pub device_id: String,
    /// 可选设备密钥（后续可扩展）
    #[allow(dead_code)] // 协议预留字段，当前服务端不校验
    pub secret: Option<String>,
    /// 设备型号：客户端取本体 1201 的 `type`（如 `ZSL-2-Ultra`），取不到则不报
    pub model: Option<String>,
    /// 固件版本：客户端本地配置 / 本体上报，用于中台台账展示
    pub firmware: Option<String>,
}

/// 服务端 → 客户端：注册确认
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterAckPayload {
    pub success: bool,
    pub message: String,
    pub device_id: String,
    pub device_name: Option<String>,
    /// 设备绑定的园区；未绑定时不下发，客户端据此跳过地图回传
    #[serde(skip_serializing_if = "Option::is_none")]
    pub park_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub park_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub park_name: Option<String>,
    pub server_ts: String,
}

/// 客户端 → 服务端：心跳
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatPayload {
    pub battery: Option<i64>,
    pub temperature: Option<f64>,
    pub position: Option<String>,
    pub odometer: Option<f64>,
    pub runtime_minutes: Option<i64>,
    pub current_task: Option<String>,
    pub firmware: Option<String>,
    /// 设备型号（本体 1201 `type`）：首次报到后变化才触发中台台账重同步
    pub model: Option<String>,
    /// 本体链路是否在线（false = 客户端处于模拟模式 / 本体掉线）
    pub body_online: Option<bool>,
    /// 客户端工作模式：robot / sim
    pub mode: Option<String>,
    /// 充电状态：本体 1315 上报（充电桩接入 / 电池电源状态为充电或充满）
    pub charging: Option<bool>,
    /// 本体工作模式：1315 `mode`（协议里的 MachineStatus），如 `remote`
    pub robot_mode: Option<String>,
    /// 定位状态（智航 5009 `loc_state.status`）：100 已收敛 / 122 定位丢失 / 210 定位失败
    pub loc_status: Option<i64>,
    /// 机器人当前生效地图（业务侧 mapId，如 `ROS-MAP-3@v1.0`）
    pub map_id: Option<String>,
    /// 1315 `ctrl_source`：0 未知 / 1 APP / 2 SDK / 3 其他（含硬件遥控器）
    pub ctrl_source: Option<i64>,
    /// 1315 `motion_status`：stand_up / lie_down / walk …（卧倒时走不动）
    pub motion_status: Option<String>,
    /// 1315 `estop.software`：软件急停是否按下
    pub estop_software: Option<bool>,
    /// 1207 `modules[]` 的 `sdk_node`：本体算法节点是否就绪
    pub sdk_node_available: Option<bool>,
}

/// 服务端 → 客户端：心跳确认
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatAckPayload {
    pub server_ts: String,
    /// 待执行命令队列（可选）
    pub pending_commands: Vec<PendingCommand>,
}

/// 客户端 → 服务端：任务进度上报
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressPayload {
    #[allow(dead_code)] // 协议预留字段，任务匹配以 task_code 为准
    pub task_id: Option<i64>,
    pub task_code: String,
    pub status: String,
    pub progress: i64,
    pub point_index: Option<i64>,
    pub message: Option<String>,
    /// 智航 Pro 本体侧任务标识（客户端从 5009 快照中取得）
    #[serde(default)]
    pub robot_task_id: Option<String>,
    /// 智航 Pro 执行阶段，如 `navigation.path_running`
    #[serde(default)]
    pub stage: Option<String>,
    /// 航点总数，便于前端展示 `已完成/总数`
    #[serde(default)]
    pub total_points: Option<i64>,
}

/// 客户端 → 服务端：设备事件
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEventPayload {
    pub event_type: String,
    pub message: String,
    #[allow(dead_code)] // 协议预留字段，用于后续事件详情展示
    pub data: Option<Value>,
}

/// 服务端 → 客户端：任务下发
///
/// `route_data` 是前端三栏编辑器产出的 `RouteData`（`points/legs/returnLeg`）。
/// `map` 携带客户端驱动智航本体时需要的载图信息；缺省时客户端会按任务所属园区
/// 的当前基线回退。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskDispatchPayload {
    pub task_id: i64,
    pub task_code: String,
    pub task_name: String,
    pub task_type: Option<String>,
    pub execute_mode: Option<String>,
    pub route: Option<Value>,
    pub route_data: Option<Value>,
    pub plan_start: Option<String>,
    pub plan_end: Option<String>,
    pub dispatch_policy: Option<String>,
    pub description: Option<String>,
    /// 智航本体载图信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map: Option<DispatchMap>,
    /// 巡航速度上限（m/s），取自路线 `default_speed`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_limit: Option<f64>,
    /// 避障等级：10 关闭 / 20 仅急停 / 30 感知 / 40 全避障
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avoid_level: Option<i64>,
    /// 结束动作：`return_to_start` | `return_to_charge` | `none`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_task: Option<String>,
}

/// 下发载荷中的地图绑定
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DispatchMap {
    /// 业务侧地图版本 ID，如 `ROS-MAP-3@v1.0`
    pub map_id: String,
    /// 本体地图目录名/路径（相对本体 map_root，客户端会用 4013 列表核对）
    pub map_path: String,
    /// 本体地图目录名（4013 `path` 的最后一段）
    pub map_dir: String,
    /// 1 = 2D 栅格, 3 = Route 路网
    pub map_type: u32,
    pub park_id: i64,
    pub map_name: String,
    pub version: String,
}

/// 服务端 → 客户端：任务取消/暂停
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskControlPayload {
    pub task_id: i64,
    pub task_code: String,
    pub action: String, // "pause" | "resume" | "cancel"
    pub reason: Option<String>,
}

/// 服务端 → 客户端：远程指令
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommandPayload {
    pub command: String,
    pub params: Option<Value>,
    pub issued_at: String,
}

/// 待执行命令（心跳响应中捎带）
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingCommand {
    pub cmd_type: String, // "task_dispatch" | "task_control" | "remote_command"
    pub payload: Value,
}

/// 连接中的设备会话信息
///
/// 目前连接池只保存广播发送端，本结构用于后续展示会话详情。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DeviceSession {
    pub device_id: String,
    pub device_name: Option<String>,
    pub connected_at: String,
    pub last_heartbeat: String,
}
