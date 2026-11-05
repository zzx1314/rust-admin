use std::sync::Arc;

use chrono::{DateTime, Utc};
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::business::dog::device::entity::ActiveModel as DeviceActiveModel;
use crate::business::dog::device::entity::Column as DeviceColumn;
use crate::business::dog::device::entity::Entity as DeviceEntity;
use crate::business::dog::device::entity::Model as Device;
use crate::business::dog::gateway::domain::{DeviceRegistration, HeartbeatPayload};

/// Device persistence used by the device gateway (registration, heartbeat and
/// connection lifecycle). Kept separate from `SeaOrmDogDeviceRepository`
/// because these operations are keyed by the business `device_id` instead of
/// the primary key.
#[derive(Clone)]
pub struct DogGatewayRepository {
    conn: Arc<DatabaseConnection>,
}

impl DogGatewayRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }

    pub async fn find_device(&self, device_id: &str) -> Result<Option<Device>, sea_orm::DbErr> {
        DeviceEntity::find()
            .filter(DeviceColumn::DeviceId.eq(device_id))
            .filter(DeviceColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    /// Create the device on first registration, otherwise refresh the
    /// reported metadata. The device stays `offline` until a WebSocket
    /// connection sends its first heartbeat.
    pub async fn upsert_registration(
        &self,
        registration: &DeviceRegistration,
    ) -> Result<Device, sea_orm::DbErr> {
        let now = Utc::now();
        match self.find_device(&registration.device_id).await? {
            Some(existing) => {
                let id = existing.id;
                let mut active_model: DeviceActiveModel = existing.into();
                if let Some(device_ip) = &registration.device_ip {
                    active_model.device_ip = ActiveValue::set(Some(device_ip.clone()));
                }
                if let Some(device_type) = &registration.device_type {
                    active_model.device_type = ActiveValue::set(Some(device_type.clone()));
                }
                if let Some(model) = &registration.model {
                    active_model.model = ActiveValue::set(Some(model.clone()));
                }
                active_model.status = ActiveValue::set("offline".to_string());
                active_model.comm_time = ActiveValue::set(Some(now));
                active_model.update_time = ActiveValue::set(Some(now));
                DeviceEntity::update(active_model).exec(&*self.conn).await?;
                self.find_device(&registration.device_id)
                    .await?
                    .ok_or_else(|| sea_orm::DbErr::RecordNotFound(id.to_string()))
            }
            None => {
                let model = DeviceActiveModel {
                    id: ActiveValue::set(generate_id()),
                    device_id: ActiveValue::set(registration.device_id.clone()),
                    device_type: ActiveValue::set(registration.device_type.clone()),
                    status: ActiveValue::set("offline".to_string()),
                    device_ip: ActiveValue::set(registration.device_ip.clone()),
                    model: ActiveValue::set(registration.model.clone()),
                    speed: ActiveValue::set(None),
                    height: ActiveValue::set(None),
                    battery: ActiveValue::set(None),
                    course: ActiveValue::set(None),
                    location: ActiveValue::set(None),
                    comm_time: ActiveValue::set(Some(now)),
                    create_time: ActiveValue::set(now),
                    update_time: ActiveValue::set(Some(now)),
                    is_deleted: ActiveValue::set(0),
                };
                DeviceEntity::insert(model).exec(&*self.conn).await?;
                self.find_device(&registration.device_id)
                    .await?
                    .ok_or_else(|| sea_orm::DbErr::RecordNotFound(registration.device_id.clone()))
            }
        }
    }

    /// Mark the device online and persist the reported telemetry.
    pub async fn apply_heartbeat(
        &self,
        device_id: &str,
        heartbeat: &HeartbeatPayload,
    ) -> Result<Option<Device>, sea_orm::DbErr> {
        let Some(existing) = self.find_device(device_id).await? else {
            return Ok(None);
        };
        let now = Utc::now();
        let telemetry = heartbeat.telemetry();
        let mut active_model: DeviceActiveModel = existing.into();
        active_model.status = ActiveValue::set("online".to_string());
        active_model.comm_time = ActiveValue::set(Some(now));
        active_model.update_time = ActiveValue::set(Some(now));
        if let Some(speed) = telemetry.speed {
            active_model.speed = ActiveValue::set(Some(speed));
        }
        if let Some(height) = telemetry.height {
            active_model.height = ActiveValue::set(Some(height));
        }
        if let Some(battery) = telemetry.battery {
            active_model.battery = ActiveValue::set(Some(battery));
        }
        if let Some(course) = telemetry.course {
            active_model.course = ActiveValue::set(Some(course));
        }
        if let Some(location) = telemetry.location {
            active_model.location = ActiveValue::set(Some(location));
        }
        DeviceEntity::update(active_model).exec(&*self.conn).await?;
        self.find_device(device_id).await
    }

    /// Mark the device offline after its WebSocket connection dropped.
    pub async fn mark_offline(&self, device_id: &str) -> Result<Option<Device>, sea_orm::DbErr> {
        let Some(existing) = self.find_device(device_id).await? else {
            return Ok(None);
        };
        let now: DateTime<Utc> = Utc::now();
        let mut active_model: DeviceActiveModel = existing.into();
        active_model.status = ActiveValue::set("offline".to_string());
        active_model.comm_time = ActiveValue::set(Some(now));
        active_model.update_time = ActiveValue::set(Some(now));
        DeviceEntity::update(active_model).exec(&*self.conn).await?;
        self.find_device(device_id).await
    }
}

fn generate_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
