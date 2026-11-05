use std::path::{Path, PathBuf};
use std::sync::Arc;

use sea_orm::DatabaseConnection;
use serde_json::Value;

use crate::business::dog::command::domain::DogCommand;
use crate::business::dog::command::repository::SeaOrmDogCommandRepository;
use crate::business::dog::device::entity::Model as Device;
use crate::business::dog::gateway::domain::{
    CommandResultPayload, ConnectionInfo, DeviceRegistration, DispatchResult, HeartbeatPayload,
    MAX_UPLOAD_BYTES, server_command_message,
};
use crate::business::dog::gateway::registry::DeviceRegistry;
use crate::business::dog::gateway::repository::DogGatewayRepository;
use crate::business::dog::media::domain::{CreateMediaRequest, DogMedia};
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::common::error::AppError;

/// Device gateway: device authentication, registration, heartbeats, task
/// dispatch and file uploads.
pub struct DogGateway {
    registry: DeviceRegistry,
    repo: DogGatewayRepository,
    command_repo: SeaOrmDogCommandRepository,
    media_repo: SeaOrmDogMediaRepository,
    upload_root: PathBuf,
}

impl DogGateway {
    pub fn new(conn: Arc<DatabaseConnection>, upload_root: impl Into<PathBuf>) -> Self {
        Self {
            registry: DeviceRegistry::new(),
            repo: DogGatewayRepository::new(conn.clone()),
            command_repo: SeaOrmDogCommandRepository::new(conn.clone()),
            media_repo: SeaOrmDogMediaRepository::new(conn),
            upload_root: upload_root.into(),
        }
    }

    pub fn registry(&self) -> &DeviceRegistry {
        &self.registry
    }

    pub fn upload_root(&self) -> &Path {
        &self.upload_root
    }

    // ---------------------------------------------------------------- tokens

    /// Issue a device access token (`GET /api/dogGateway/auth/{deviceId}`).
    pub async fn issue_token(&self, device_id: &str) -> String {
        self.registry.issue_token(device_id).await
    }

    /// Validate the token used in the WebSocket handshake.
    pub async fn authorize(&self, device_id: &str, token: &str) -> bool {
        self.registry.authorize(device_id, token).await
    }

    // -------------------------------------------------------------- devices

    pub async fn device(&self, device_id: &str) -> Result<Option<Device>, AppError> {
        self.repo
            .find_device(device_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    /// `POST /api/dogGateway/register`.
    pub async fn register_device(
        &self,
        registration: DeviceRegistration,
    ) -> Result<Device, AppError> {
        if registration.device_id.trim().is_empty() {
            return Err(AppError::BadRequest(
                "deviceId must not be empty".to_string(),
            ));
        }
        let device = self
            .repo
            .upsert_registration(&registration)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        tracing::info!(
            device_id = %device.device_id,
            device_type = ?device.device_type,
            "device registered"
        );
        Ok(device)
    }

    /// Persist device telemetry received in a heartbeat.
    pub async fn handle_heartbeat(
        &self,
        device_id: &str,
        heartbeat: &HeartbeatPayload,
    ) -> Result<Option<Device>, AppError> {
        let device = self
            .repo
            .apply_heartbeat(device_id, heartbeat)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if device.is_none() {
            tracing::warn!(
                device_id = device_id,
                "heartbeat from unregistered device, register it first"
            );
        }
        Ok(device)
    }

    /// Mark the device offline when its WebSocket connection is gone.
    pub async fn mark_offline(&self, device_id: &str) {
        if let Err(error) = self.repo.mark_offline(device_id).await {
            tracing::warn!(device_id = device_id, %error, "failed to mark device offline");
        }
    }

    // -------------------------------------------------------------- results

    /// Persist a command execution result reported by a device.
    pub async fn handle_command_result(
        &self,
        device_id: &str,
        payload: &CommandResultPayload,
        raw: &Value,
    ) -> Result<Option<DogCommand>, AppError> {
        let Some(command_id) = payload.command_id() else {
            tracing::warn!(
                device_id = device_id,
                payload = %raw,
                "command result without command id"
            );
            return Ok(None);
        };

        let status = payload
            .normalized_status()
            .unwrap_or_else(|| "success".to_string());
        let return_value = payload
            .return_text()
            .or_else(|| Some(serde_json::to_string(raw).unwrap_or_default()));

        let updated = self
            .command_repo
            .update_status(command_id, &status, return_value)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        match &updated {
            Some(command) => tracing::info!(
                device_id = device_id,
                command_id = command_id,
                status = %command.status,
                "command result stored"
            ),
            None => tracing::warn!(
                device_id = device_id,
                command_id = command_id,
                "command result for unknown command"
            ),
        }

        Ok(updated)
    }

    // -------------------------------------------------------------- uploads

    /// Persist an uploaded file stream and record it as a `dog_media` row.
    pub async fn save_upload(
        &self,
        device_id: &str,
        stream_id: &str,
        file_name: Option<&str>,
        media_type: Option<&str>,
        data: &[u8],
    ) -> Result<Option<DogMedia>, AppError> {
        if data.is_empty() {
            tracing::warn!(
                device_id = device_id,
                stream_id = stream_id,
                "empty upload stream ignored"
            );
            return Ok(None);
        }
        if data.len() > MAX_UPLOAD_BYTES {
            return Err(AppError::BadRequest(format!(
                "upload stream {stream_id} exceeds {MAX_UPLOAD_BYTES} bytes"
            )));
        }

        let safe_name = sanitize_file_name(file_name.unwrap_or("upload.bin"));
        let directory = self.upload_root.join(sanitize_component(device_id));
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|error| {
                AppError::BadRequest(format!("failed to create upload dir: {error}"))
            })?;

        let stored_name = format!("{}_{}", sanitize_component(stream_id), safe_name);
        let path = directory.join(&stored_name);
        tokio::fs::write(&path, data)
            .await
            .map_err(|error| AppError::BadRequest(format!("failed to write upload: {error}")))?;

        let request = CreateMediaRequest {
            media_name: safe_name,
            media_type: Some(media_type.unwrap_or("device_upload").to_string()),
            media_path: Some(path.to_string_lossy().to_string()),
            media_size: Some(data.len() as i64),
            remarks: Some(format!(
                "uploaded by device {device_id}, stream {stream_id}"
            )),
        };

        let media = self
            .media_repo
            .create(&request, generate_id())
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        tracing::info!(
            device_id = device_id,
            path = %path.display(),
            size = data.len(),
            "device upload stored"
        );

        Ok(Some(media))
    }

    // ------------------------------------------------------------- dispatch

    /// Push a stored command to the device that owns it.
    ///
    /// Returns the refreshed command when the device is online, `None` when it
    /// is not reached (the command keeps its current status).
    pub async fn dispatch_command(
        &self,
        command: &DogCommand,
    ) -> Result<Option<DogCommand>, AppError> {
        let Some(device_id) = command
            .device_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };

        let payload = server_command_message(
            device_id,
            command.id,
            &command.command_name,
            command.command_type.as_deref(),
            command.task_id,
            &command.status,
            command.command_params.as_deref(),
        );

        if !self.registry.send(device_id, &payload.to_string()).await {
            tracing::info!(
                device_id = device_id,
                command_id = command.id,
                "device offline, command not pushed"
            );
            return Ok(None);
        }

        let next_status = if matches!(command.status.as_str(), "" | "pending") {
            "sent"
        } else {
            command.status.as_str()
        };

        let updated = self
            .command_repo
            .update_status(command.id, next_status, command.return_value.clone())
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        tracing::info!(
            device_id = device_id,
            command_id = command.id,
            "command pushed to device"
        );

        Ok(updated.or_else(|| Some(command.clone())))
    }

    /// Push an arbitrary JSON payload to a device (manual dispatch endpoint).
    pub async fn dispatch_raw(&self, device_id: &str, payload: &Value) -> DispatchResult {
        let delivered = self.registry.send(device_id, &payload.to_string()).await;
        DispatchResult {
            delivered,
            device_id: device_id.to_string(),
        }
    }

    pub async fn connections(&self) -> Vec<ConnectionInfo> {
        self.registry.connections().await
    }

    pub async fn is_online(&self, device_id: &str) -> bool {
        self.registry.is_online(device_id).await
    }
}

/// Strip directories and unusual characters from a device provided name.
fn sanitize_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let cleaned: String = base
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        "upload.bin".to_string()
    } else {
        cleaned
    }
}

/// Sanitize a path component (device id / stream id).
fn sanitize_component(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

fn generate_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
