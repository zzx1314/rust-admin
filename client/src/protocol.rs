use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Frame type used by the client for heartbeats.
pub const MSG_HEARTBEAT: &str = "heartbeat";
/// Frame type the server uses to dispatch a command/task.
pub const MSG_SERVER_COMMAND: &str = "server-command";
/// Frame type used by the client to report a command result.
pub const MSG_REPORT: &str = "report";
pub const MSG_FILE_START: &str = "file_start";
pub const MSG_FILE_END: &str = "file_end";

/// Standard API envelope used by the server (`code = 10200` means success).
#[derive(Debug, Clone, Deserialize)]
pub struct ApiEnvelope<T> {
    #[serde(default)]
    pub code: u16,
    #[serde(default)]
    pub msg: String,
    pub data: Option<T>,
}

impl<T> ApiEnvelope<T> {
    /// Return `data` or turn the envelope into a [`ClientError::Server`].
    pub fn into_data(self) -> Result<T, crate::error::ClientError> {
        match self.data {
            Some(data) => Ok(data),
            None => Err(crate::error::ClientError::Server {
                code: self.code,
                msg: self.msg,
            }),
        }
    }
}

/// `data` of `GET /api/dogGateway/auth/{deviceId}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTokenData {
    pub device_id: String,
    pub token: String,
    #[serde(default)]
    pub expires_in: i64,
}

/// Device record returned by the register endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSummary {
    pub id: i64,
    pub device_id: String,
    #[serde(default)]
    pub device_type: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub device_ip: Option<String>,
    #[serde(default)]
    pub comm_time: Option<String>,
}

/// Body of `POST /api/dogGateway/register`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegistration {
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_ip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

/// Telemetry reported in a heartbeat (speed / height / battery / course / position).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Telemetry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

/// Heartbeat frame sent to the server.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub device_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drones: Option<Telemetry>,
}

impl Heartbeat {
    /// Heartbeat with status `success` and no telemetry.
    pub fn online(device_id: impl Into<String>) -> Self {
        Self {
            kind: MSG_HEARTBEAT,
            device_id: device_id.into(),
            status: "success".to_string(),
            drones: None,
        }
    }

    /// Attach telemetry (builder style).
    pub fn with_telemetry(mut self, telemetry: Telemetry) -> Self {
        self.drones = Some(telemetry);
        self
    }
}

/// Command (task) dispatched by the server.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCommand {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub mission_id: Option<String>,
    #[serde(default)]
    pub command_id: Option<i64>,
    #[serde(default)]
    pub command_name: Option<String>,
    #[serde(default)]
    pub command_type: Option<String>,
    #[serde(default)]
    pub task_id: Option<i64>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub timestamp: Option<String>,
}

impl ServerCommand {
    /// Command id, accepting both `commandId` and the `missionId` fallback.
    pub fn id(&self) -> Option<i64> {
        self.command_id.or_else(|| {
            self.mission_id
                .as_deref()
                .and_then(|value| value.trim().parse().ok())
        })
    }

    /// `params` rendered as a compact JSON string.
    pub fn params_text(&self) -> String {
        serde_json::to_string(&self.params).unwrap_or_else(|_| self.params.to_string())
    }
}

/// Command execution result reported back to the server.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub command_id: i64,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_value: Option<String>,
}

impl CommandResult {
    pub fn success(command_id: i64, return_value: impl Into<String>) -> Self {
        Self {
            kind: MSG_SERVER_COMMAND,
            command_id,
            status: "success".to_string(),
            params: None,
            return_value: Some(return_value.into()),
        }
    }

    pub fn failure(command_id: i64, return_value: impl Into<String>) -> Self {
        Self {
            kind: MSG_SERVER_COMMAND,
            command_id,
            status: "error".to_string(),
            params: None,
            return_value: Some(return_value.into()),
        }
    }

    pub fn with_params(mut self, params: Value) -> Self {
        self.params = Some(params);
        self
    }
}

/// Acknowledgement returned by the server after a file upload.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAck {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub stream_id: Option<String>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub path: Option<String>,
}

impl FileAck {
    pub fn is_success(&self) -> bool {
        self.status == "success"
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileStartMessage {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub stream_id: String,
    pub file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileEndMessage {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ids: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_serializes_with_type_and_telemetry() {
        let heartbeat = Heartbeat::online("dog-1").with_telemetry(Telemetry {
            battery: Some("90".to_string()),
            ..Telemetry::default()
        });
        let value: Value = serde_json::to_value(&heartbeat).unwrap();

        assert_eq!(value["type"], "heartbeat");
        assert_eq!(value["deviceId"], "dog-1");
        assert_eq!(value["status"], "success");
        assert_eq!(value["drones"]["battery"], "90");
        // unset telemetry fields are omitted
        assert!(value["drones"].get("speed").is_none());
    }

    #[test]
    fn command_result_serializes_as_camel_case_report() {
        let result = CommandResult::failure(42, "boom");
        let value: Value = serde_json::to_value(&result).unwrap();

        assert_eq!(value["type"], "server-command");
        assert_eq!(value["commandId"], 42);
        assert_eq!(value["status"], "error");
        assert_eq!(value["returnValue"], "boom");
    }

    #[test]
    fn server_command_accepts_mission_id_fallback() {
        let command: ServerCommand = serde_json::from_str(
            r#"{"type":"server-command","missionId":"1700000000001","params":{"head":"SCAN"}}"#,
        )
        .unwrap();

        assert_eq!(command.id(), Some(1_700_000_000_001));
        assert_eq!(command.params["head"], "SCAN");
    }

    #[test]
    fn registration_omits_empty_optional_fields() {
        let registration = DeviceRegistration {
            device_id: "dog-1".to_string(),
            ..Default::default()
        };
        let value: Value = serde_json::to_value(&registration).unwrap();

        assert_eq!(value.as_object().unwrap().len(), 1);
        assert_eq!(value["deviceId"], "dog-1");
    }

    #[test]
    fn envelope_without_data_is_an_error() {
        let envelope: ApiEnvelope<DeviceSummary> =
            serde_json::from_str(r#"{"code":10400,"msg":"bad token"}"#).unwrap();
        assert!(envelope.into_data().is_err());
    }
}
