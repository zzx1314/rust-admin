use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Token lifetime for a device access token, mirroring the original gateway
/// (10 minutes, refreshed every time the token is used).
pub const DEVICE_TOKEN_TTL_SECS: i64 = 600;

/// Maximum payload size a device may buffer for a single file stream (256 MiB).
pub const MAX_UPLOAD_BYTES: usize = 256 * 1024 * 1024;

/// Text frame types understood by the gateway.
pub const MSG_HEARTBEAT: &str = "heartbeat";
pub const MSG_SERVER_COMMAND: &str = "server-command";
pub const MSG_REPORT: &str = "report";
pub const MSG_TOROS_REPORT: &str = "torosReport";
pub const MSG_FILE_START: &str = "file_start";
pub const MSG_FILE_END: &str = "file_end";
pub const MSG_PING: &str = "ping";

/// Response of `GET /api/dogGateway/auth/{deviceId}`.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTokenResponse {
    pub device_id: String,
    pub token: String,
    pub expires_in: i64,
}

/// Body of `POST /api/dogGateway/register`.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegistration {
    pub device_id: String,
    #[serde(default)]
    pub device_ip: Option<String>,
    #[serde(default)]
    pub device_type: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub os_version: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
    /// How the device derived its id on the hardware (board serial, MAC, ...).
    #[serde(default)]
    pub device_id_source: Option<String>,
}

/// Device telemetry carried by a heartbeat.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct HeartbeatTelemetry {
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub height: Option<String>,
    #[serde(default)]
    pub battery: Option<String>,
    #[serde(default)]
    pub course: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

/// Heartbeat frame sent by a device.
///
/// Compatible with the original client which sends telemetry either nested
/// under `drones` or directly on the message.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct HeartbeatPayload {
    pub device_id: Option<String>,
    pub status: Option<String>,
    pub speed: Option<String>,
    pub height: Option<String>,
    pub battery: Option<String>,
    pub course: Option<String>,
    pub location: Option<String>,
    pub drones: Option<HeartbeatTelemetry>,
}

impl HeartbeatPayload {
    /// Flatten nested `drones` telemetry into the top-level fields.
    pub fn telemetry(&self) -> HeartbeatTelemetry {
        let nested = self.drones.clone().unwrap_or_default();
        HeartbeatTelemetry {
            speed: self.speed.clone().or(nested.speed),
            height: self.height.clone().or(nested.height),
            battery: self.battery.clone().or(nested.battery),
            course: self.course.clone().or(nested.course),
            location: self.location.clone().or(nested.location),
        }
    }
}

/// Command execution result reported by a device.
///
/// `server-command` frames carry `commandId`, `report` frames carry `id` and
/// `torosReport` frames carry `missionId`, so all three are accepted.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CommandResultPayload {
    pub command_id: Option<Value>,
    pub id: Option<Value>,
    pub mission_id: Option<Value>,
    pub status: Option<String>,
    pub return_value: Option<String>,
    pub params: Option<Value>,
    pub msg: Option<Value>,
}

impl CommandResultPayload {
    /// Extract the numeric command id from whichever field the device used.
    pub fn command_id(&self) -> Option<i64> {
        [
            self.command_id.as_ref(),
            self.id.as_ref(),
            self.mission_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        .find_map(value_to_i64)
    }

    /// Normalised status: `success` / `error` / the raw string.
    pub fn normalized_status(&self) -> Option<String> {
        self.status
            .as_deref()
            .map(str::trim)
            .filter(|status| !status.is_empty())
            .map(|status| match status {
                "SUCCESS" | "ok" | "OK" => "success".to_string(),
                "ERROR" | "failed" | "FAILED" => "error".to_string(),
                other => other.to_string(),
            })
    }

    /// Human readable return value, falling back to the raw payload.
    pub fn return_text(&self) -> Option<String> {
        if let Some(value) = &self.return_value
            && !value.is_empty()
        {
            return Some(value.clone());
        }
        if let Some(params) = &self.params {
            return Some(compact_json(params));
        }
        self.msg.as_ref().map(compact_json)
    }
}

/// `file_start` frame.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FileStartPayload {
    pub stream_id: String,
    pub file_name: Option<String>,
    pub media_type: Option<String>,
}

/// `file_end` frame.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FileEndPayload {
    pub stream_id: String,
    pub target_ids: Option<Value>,
}

/// Command pushed from the server to a device.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServerCommandMessage {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub device_id: String,
    pub mission_id: String,
    pub command_id: i64,
    pub command_name: String,
    pub command_type: Option<String>,
    pub task_id: Option<i64>,
    pub status: String,
    pub params: Value,
    pub timestamp: String,
}

/// Manual dispatch request (`POST /api/dogGateway/dispatch`).
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DispatchRequest {
    pub device_id: String,
    pub payload: Value,
}

/// Assignment request (`POST /api/dogGateway/assign`).
///
/// Device registration stays user agnostic; the owner is attached through
/// this endpoint using the hardware id the device reported when registering.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AssignDeviceRequest {
    pub device_id: String,
    /// Target user. Administrators may assign anybody; regular users may only
    /// pass their own id (omitted means "myself").
    #[serde(default)]
    pub user_id: Option<i64>,
}

/// Query parameters of `GET/DELETE /api/dogGateway/assign/{deviceId}`.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssignUserQuery {
    #[serde(default)]
    pub user_id: Option<i64>,
}

/// One online device connection.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub device_id: String,
    pub connected_at: String,
    pub remote_addr: Option<String>,
}

/// Result of a forced disconnect (`POST /api/dogGateway/disconnect/{deviceId}`).
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectResult {
    pub device_id: String,
    pub disconnected: bool,
}

/// Result of a dispatch attempt.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DispatchResult {
    pub delivered: bool,
    pub device_id: String,
}

/// Accepts `12`, `"12"` and `12.0` as a command id.
fn value_to_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// Build the payload pushed to a device for a stored command.
pub fn server_command_message(
    device_id: &str,
    command_id: i64,
    command_name: &str,
    command_type: Option<&str>,
    task_id: Option<i64>,
    status: &str,
    params: Option<&str>,
) -> Value {
    let params = params
        .map(|raw| {
            serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
        })
        .unwrap_or(Value::Null);

    let message = ServerCommandMessage {
        kind: MSG_SERVER_COMMAND,
        device_id: device_id.to_string(),
        mission_id: command_id.to_string(),
        command_id,
        command_name: command_name.to_string(),
        command_type: command_type.map(str::to_string),
        task_id,
        status: status.to_string(),
        params,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    json!(message)
}

/// Heartbeat acknowledgement sent back to the device.
pub fn heartbeat_ack(device_id: &str) -> Value {
    json!({
        "type": MSG_REPORT,
        "deviceId": device_id,
        "status": "online",
        "msg": "success",
    })
}

/// File upload acknowledgement.
pub fn file_ack(stream_id: &str, saved: bool, path: Option<&str>) -> Value {
    json!({
        "type": "file_ack",
        "streamId": stream_id,
        "status": if saved { "success" } else { "error" },
        "path": path,
    })
}
