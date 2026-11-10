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
    pub secret: Option<String>,
}

/// 服务端 → 客户端：注册确认
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterAckPayload {
    pub success: bool,
    pub message: String,
    pub device_id: String,
    pub device_name: Option<String>,
    pub server_ts: String,
}

/// 客户端 → 服务端：心跳
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatPayload {
    pub battery: Option<i64>,
    pub signal: Option<i64>,
    pub temperature: Option<f64>,
    pub position: Option<String>,
    pub odometer: Option<f64>,
    pub runtime_minutes: Option<i64>,
    pub current_task: Option<String>,
    pub firmware: Option<String>,
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
    pub task_id: Option<i64>,
    pub task_code: String,
    pub status: String,
    pub progress: i64,
    pub point_index: Option<i64>,
    pub message: Option<String>,
}

/// 客户端 → 服务端：设备事件
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceEventPayload {
    pub event_type: String,
    pub message: String,
    pub data: Option<Value>,
}

/// 服务端 → 客户端：任务下发
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
#[derive(Debug, Clone)]
pub struct DeviceSession {
    pub device_id: String,
    pub device_name: Option<String>,
    pub connected_at: String,
    pub last_heartbeat: String,
}
