use std::collections::HashMap;

use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, mpsc};

use crate::business::dog::gateway::domain::{ConnectionInfo, DEVICE_TOKEN_TTL_SECS};

/// A pending device access token (`token -> deviceId`), mirroring the Redis
/// entry kept by the original Java gateway.
struct DeviceToken {
    device_id: String,
    expires_at: DateTime<Utc>,
}

/// A live device connection: every server side message is queued on `tx` and
/// consumed by the WebSocket writer task.
pub struct DeviceConnection {
    pub tx: mpsc::UnboundedSender<String>,
    pub connected_at: DateTime<Utc>,
    pub remote_addr: Option<String>,
}

/// In-memory registry of device tokens and live WebSocket connections.
///
/// The gateway is single-instance by design (a device is connected to exactly
/// one node); crossing nodes would require a shared bus such as Redis pub/sub.
#[derive(Default)]
pub struct DeviceRegistry {
    tokens: Mutex<HashMap<String, DeviceToken>>,
    connections: Mutex<HashMap<String, DeviceConnection>>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Issue a fresh token for a device, replacing any older token.
    pub async fn issue_token(&self, device_id: &str) -> String {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut tokens = self.tokens.lock().await;
        let now = Utc::now();
        tokens.retain(|_, entry| entry.expires_at > now);
        tokens.insert(
            token.clone(),
            DeviceToken {
                device_id: device_id.to_string(),
                expires_at: now + chrono::Duration::seconds(DEVICE_TOKEN_TTL_SECS),
            },
        );
        token
    }

    /// Validate a device token and refresh its TTL on success.
    pub async fn authorize(&self, device_id: &str, token: &str) -> bool {
        let mut tokens = self.tokens.lock().await;
        let now = Utc::now();
        match tokens.get_mut(token) {
            Some(entry) if entry.expires_at > now && entry.device_id == device_id => {
                entry.expires_at = now + chrono::Duration::seconds(DEVICE_TOKEN_TTL_SECS);
                true
            }
            Some(_) => false,
            None => false,
        }
    }

    /// Register a live connection, replacing any previous one for the device.
    pub async fn register(
        &self,
        device_id: &str,
        tx: mpsc::UnboundedSender<String>,
        remote_addr: Option<String>,
    ) {
        let mut connections = self.connections.lock().await;
        connections.insert(
            device_id.to_string(),
            DeviceConnection {
                tx,
                connected_at: Utc::now(),
                remote_addr,
            },
        );
    }

    /// Remove the live connection of a device.
    pub async fn unregister(&self, device_id: &str) {
        let mut connections = self.connections.lock().await;
        connections.remove(device_id);
    }

    /// Send a text payload to a device; returns `false` when it is offline.
    pub async fn send(&self, device_id: &str, payload: &str) -> bool {
        let connections = self.connections.lock().await;
        match connections.get(device_id) {
            Some(connection) => connection.tx.send(payload.to_string()).is_ok(),
            None => false,
        }
    }

    pub async fn is_online(&self, device_id: &str) -> bool {
        let connections = self.connections.lock().await;
        connections.contains_key(device_id)
    }

    pub async fn connections(&self) -> Vec<ConnectionInfo> {
        let connections = self.connections.lock().await;
        let mut list: Vec<ConnectionInfo> = connections
            .iter()
            .map(|(device_id, connection)| ConnectionInfo {
                device_id: device_id.clone(),
                connected_at: crate::common::util::format_datetime(connection.connected_at),
                remote_addr: connection.remote_addr.clone(),
            })
            .collect();
        list.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        list
    }
}
