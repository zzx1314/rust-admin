use crate::business::dog::device::domain::{
    CreateDeviceRequest, DevicePageQuery, UpdateDeviceRequest,
};
use crate::business::dog::device::entity::ActiveModel as DeviceActiveModel;
use crate::business::dog::device::entity::Column as DeviceColumn;
use crate::business::dog::device::entity::Entity as DeviceEntity;
use crate::business::dog::device::entity::Model as Device;
use crate::business::dog::user_device::entity::Column as UserDeviceColumn;
use crate::business::dog::user_device::entity::Entity as UserDeviceEntity;
use crate::common::base::{BaseRepository, order_desc};
use crate::common::data_scope::DataScope;
use crate::common::util::parse_beijing_datetime;
use sea_orm::sea_query::{Expr, SelectStatement};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, QueryTrait, Select,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogDeviceRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogDeviceRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }

    fn bound_device_ids(owner: i64) -> SelectStatement {
        UserDeviceEntity::find()
            .select_only()
            .column(UserDeviceColumn::DeviceId)
            .filter(UserDeviceColumn::UserId.eq(owner))
            .filter(UserDeviceColumn::IsDeleted.eq(0))
            .into_query()
    }

    fn apply_scope(query: Select<DeviceEntity>, scope: &DataScope) -> Select<DeviceEntity> {
        match scope.owner_filter() {
            Some(owner) => {
                query.filter(Expr::col(DeviceColumn::Id).in_subquery(Self::bound_device_ids(owner)))
            }
            None => query,
        }
    }

    pub async fn create(
        &self,
        req: &CreateDeviceRequest,
        id: i64,
    ) -> Result<Device, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        DeviceEntity::insert(active_model).exec(&*self.conn).await?;
        self.find_by_id_unscoped(id).await.map(|r| r.unwrap())
    }

    /// Unscoped lookup used by callers that do not act on behalf of a user.
    pub async fn find_by_id_unscoped(&self, id: i64) -> Result<Option<Device>, sea_orm::DbErr> {
        DeviceEntity::find()
            .filter(DeviceColumn::Id.eq(id))
            .filter(DeviceColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    /// Resolves a device by its hardware/business id (the value the gateway
    /// registers with), independent of the row level data scope.
    pub async fn find_by_device_code(
        &self,
        device_code: &str,
    ) -> Result<Option<Device>, sea_orm::DbErr> {
        DeviceEntity::find()
            .filter(DeviceColumn::DeviceId.eq(device_code))
            .filter(DeviceColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_by_id(
        &self,
        id: i64,
        scope: &DataScope,
    ) -> Result<Option<Device>, sea_orm::DbErr> {
        let query = DeviceEntity::find()
            .filter(DeviceColumn::Id.eq(id))
            .filter(DeviceColumn::IsDeleted.eq(0));
        Self::apply_scope(query, scope).one(&*self.conn).await
    }

    pub async fn exists_for_scope(
        &self,
        id: i64,
        scope: &DataScope,
    ) -> Result<bool, sea_orm::DbErr> {
        Ok(self.find_by_id(id, scope).await?.is_some())
    }

    pub async fn find_by_ids(&self, ids: &[i64]) -> Result<Vec<Device>, sea_orm::DbErr> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        DeviceEntity::find()
            .filter(DeviceColumn::Id.is_in(ids.iter().copied()))
            .filter(DeviceColumn::IsDeleted.eq(0))
            .all(&*self.conn)
            .await
    }

    pub async fn find_all_with_page(
        &self,
        query: &DevicePageQuery,
        scope: &DataScope,
    ) -> Result<(Vec<Device>, i64), sea_orm::DbErr> {
        let base_query = DeviceEntity::find().filter(DeviceColumn::IsDeleted.eq(0));
        let mut cond = Condition::all();
        if let Some(ref device_id) = query.device_id {
            cond = cond.add(DeviceColumn::DeviceId.contains(device_id.as_str()));
        }
        if let Some(ref device_type) = query.device_type {
            cond = cond.add(DeviceColumn::DeviceType.contains(device_type.as_str()));
        }
        if let Some(ref status) = query.status {
            cond = cond.add(DeviceColumn::Status.eq(status.as_str()));
        }
        if let Some(ref begin) = query.begin_time
            && let Some(utc_dt) = parse_beijing_datetime(begin, false)
        {
            cond = cond.add(DeviceColumn::CreateTime.gte(utc_dt));
        }
        if let Some(ref end) = query.end_time
            && let Some(utc_dt) = parse_beijing_datetime(end, true)
        {
            cond = cond.add(DeviceColumn::CreateTime.lte(utc_dt));
        }

        let total = Self::apply_scope(base_query.clone().filter(cond.clone()), scope)
            .count(&*self.conn)
            .await?;
        let current = query.current.max(1);
        let size = query.size.max(1);
        let offset = (current - 1) * size;

        let records = Self::apply_scope(base_query.filter(cond), scope)
            .order_by(DeviceColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateDeviceRequest,
        scope: &DataScope,
    ) -> Result<Option<Device>, sea_orm::DbErr> {
        let entity = self.find_by_id(id, scope).await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: DeviceActiveModel = entity.into();
        if let Some(device_type) = &req.device_type {
            active_model.device_type = ActiveValue::set(Some(device_type.clone()));
        }
        if let Some(status) = &req.status {
            active_model.status = ActiveValue::set(status.clone());
        }
        if let Some(ip) = &req.device_ip {
            active_model.device_ip = ActiveValue::set(Some(ip.clone()));
        }
        if let Some(model) = &req.model {
            active_model.model = ActiveValue::set(Some(model.clone()));
        }
        if let Some(speed) = &req.speed {
            active_model.speed = ActiveValue::set(Some(speed.clone()));
        }
        if let Some(height) = &req.height {
            active_model.height = ActiveValue::set(Some(height.clone()));
        }
        if let Some(battery) = &req.battery {
            active_model.battery = ActiveValue::set(Some(battery.clone()));
        }
        if let Some(course) = &req.course {
            active_model.course = ActiveValue::set(Some(course.clone()));
        }
        if let Some(location) = &req.location {
            active_model.location = ActiveValue::set(Some(location.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        DeviceEntity::update(active_model)
            .filter(DeviceColumn::Id.eq(id))
            .exec(&*self.conn)
            .await?;
        self.find_by_id(id, scope).await
    }

    pub async fn update_status(
        &self,
        id: i64,
        status: &str,
        scope: &DataScope,
    ) -> Result<Option<Device>, sea_orm::DbErr> {
        let device = self.find_by_id(id, scope).await?;
        if let Some(mut device) = device {
            device.status = status.to_string();
            device.update_time = Some(chrono::Utc::now());
            let mut active_model: DeviceActiveModel = device.into();
            active_model.status = ActiveValue::Set(status.to_string());
            DeviceEntity::update(active_model)
                .filter(DeviceColumn::Id.eq(id))
                .exec(&*self.conn)
                .await?;
            self.find_by_id(id, scope).await
        } else {
            Ok(None)
        }
    }

    pub async fn update_comm_time(&self, id: i64) -> Result<Option<Device>, sea_orm::DbErr> {
        let device = self.find_by_id_unscoped(id).await?;
        if let Some(mut device) = device {
            device.comm_time = Some(chrono::Utc::now());
            device.update_time = Some(chrono::Utc::now());
            let mut active_model: DeviceActiveModel = device.into();
            active_model.comm_time = ActiveValue::Set(Some(chrono::Utc::now()));
            DeviceEntity::update(active_model)
                .filter(DeviceColumn::Id.eq(id))
                .exec(&*self.conn)
                .await?;
            self.find_by_id_unscoped(id).await
        } else {
            Ok(None)
        }
    }

    pub async fn find_all(&self, scope: &DataScope) -> Result<Vec<Device>, sea_orm::DbErr> {
        Self::apply_scope(
            DeviceEntity::find().filter(DeviceColumn::IsDeleted.eq(0)),
            scope,
        )
        .order_by(DeviceColumn::CreateTime, order_desc())
        .all(&*self.conn)
        .await
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<bool, sea_orm::DbErr> {
        let device = self.find_by_id(id, scope).await?;
        if let Some(mut device) = device {
            device.is_deleted = 1;
            device.update_time = Some(chrono::Utc::now());
            let mut active_model: DeviceActiveModel = device.into();
            active_model.is_deleted = ActiveValue::Set(1);
            DeviceEntity::update(active_model)
                .filter(DeviceColumn::Id.eq(id))
                .exec(&*self.conn)
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn count(&self, scope: &DataScope) -> Result<i64, sea_orm::DbErr> {
        Self::apply_scope(
            DeviceEntity::find().filter(DeviceColumn::IsDeleted.eq(0)),
            scope,
        )
        .count(&*self.conn)
        .await
        .map(|c| c as i64)
    }
}

impl BaseRepository for SeaOrmDogDeviceRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}
