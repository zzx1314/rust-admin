use crate::business::dog::user_device::domain::{CreateUserDeviceRequest, UserDevicePageQuery};
use crate::business::dog::user_device::entity::ActiveModel as UserDeviceActiveModel;
use crate::business::dog::user_device::entity::Column as UserDeviceColumn;
use crate::business::dog::user_device::entity::Entity as UserDeviceEntity;
use crate::business::dog::user_device::entity::Model as UserDevice;
use crate::common::base::{BaseRepository, order_desc};
use crate::common::data_scope::DataScope;
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogUserDeviceRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogUserDeviceRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }

    pub async fn create(
        &self,
        req: &CreateUserDeviceRequest,
        user_id: i64,
        id: i64,
    ) -> Result<UserDevice, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, user_id, now);
        UserDeviceEntity::insert(active_model)
            .exec(&*self.conn)
            .await?;
        UserDeviceEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(
        &self,
        id: i64,
        scope: &DataScope,
    ) -> Result<Option<UserDevice>, sea_orm::DbErr> {
        let mut query = UserDeviceEntity::find()
            .filter(UserDeviceColumn::Id.eq(id))
            .filter(UserDeviceColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            query = query.filter(UserDeviceColumn::UserId.eq(owner));
        }
        query.one(&*self.conn).await
    }

    /// Finds a binding regardless of its soft-delete flag, used to restore a
    /// previously removed binding instead of violating the unique index.
    pub async fn find_any_by_user_and_device(
        &self,
        user_id: i64,
        device_id: i64,
    ) -> Result<Option<UserDevice>, sea_orm::DbErr> {
        UserDeviceEntity::find()
            .filter(UserDeviceColumn::UserId.eq(user_id))
            .filter(UserDeviceColumn::DeviceId.eq(device_id))
            .one(&*self.conn)
            .await
    }

    pub async fn restore(&self, id: i64) -> Result<(), sea_orm::DbErr> {
        let mut active_model = UserDeviceActiveModel {
            id: ActiveValue::set(id),
            ..Default::default()
        };
        active_model.is_deleted = ActiveValue::set(0);
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));
        UserDeviceEntity::update(active_model)
            .filter(UserDeviceColumn::Id.eq(id))
            .exec(&*self.conn)
            .await?;
        Ok(())
    }

    pub async fn count_active_by_device(&self, device_id: i64) -> Result<i64, sea_orm::DbErr> {
        UserDeviceEntity::find()
            .filter(UserDeviceColumn::DeviceId.eq(device_id))
            .filter(UserDeviceColumn::IsDeleted.eq(0))
            .count(&*self.conn)
            .await
            .map(|c| c as i64)
    }

    pub async fn find_all_with_page(
        &self,
        query: &UserDevicePageQuery,
        scope: &DataScope,
    ) -> Result<(Vec<UserDevice>, i64), sea_orm::DbErr> {
        let mut base_query = UserDeviceEntity::find().filter(UserDeviceColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            base_query = base_query.filter(UserDeviceColumn::UserId.eq(owner));
        }

        let mut cond = Condition::all();
        if let Some(user_id) = query.user_id {
            cond = cond.add(UserDeviceColumn::UserId.eq(user_id));
        }
        if let Some(device_id) = query.device_id {
            cond = cond.add(UserDeviceColumn::DeviceId.eq(device_id));
        }

        let total = base_query
            .clone()
            .filter(cond.clone())
            .count(&*self.conn)
            .await?;

        let current = query.current.max(1);
        let size = query.size.max(1);
        let offset = (current - 1) * size;

        let records = base_query
            .filter(cond)
            .order_by(UserDeviceColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn find_all(&self, scope: &DataScope) -> Result<Vec<UserDevice>, sea_orm::DbErr> {
        let mut query = UserDeviceEntity::find().filter(UserDeviceColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            query = query.filter(UserDeviceColumn::UserId.eq(owner));
        }
        query
            .order_by(UserDeviceColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn find_all_by_user(
        &self,
        user_id: i64,
        scope: &DataScope,
    ) -> Result<Vec<UserDevice>, sea_orm::DbErr> {
        // Non-administrators may only list their own bindings.
        if let Some(owner) = scope.owner_filter()
            && owner != user_id
        {
            return Ok(Vec::new());
        }
        UserDeviceEntity::find()
            .filter(UserDeviceColumn::UserId.eq(user_id))
            .filter(UserDeviceColumn::IsDeleted.eq(0))
            .order_by(UserDeviceColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn find_all_by_device(
        &self,
        device_id: i64,
        scope: &DataScope,
    ) -> Result<Vec<UserDevice>, sea_orm::DbErr> {
        let mut query = UserDeviceEntity::find()
            .filter(UserDeviceColumn::DeviceId.eq(device_id))
            .filter(UserDeviceColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            query = query.filter(UserDeviceColumn::UserId.eq(owner));
        }
        query
            .order_by(UserDeviceColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn find_active_by_user_and_device(
        &self,
        user_id: i64,
        device_id: i64,
    ) -> Result<Option<UserDevice>, sea_orm::DbErr> {
        UserDeviceEntity::find()
            .filter(UserDeviceColumn::UserId.eq(user_id))
            .filter(UserDeviceColumn::DeviceId.eq(device_id))
            .filter(UserDeviceColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<bool, sea_orm::DbErr> {
        let binding = self.find_by_id(id, scope).await?;
        let binding = match binding {
            Some(b) => b,
            None => return Ok(false),
        };

        let mut active_model: UserDeviceActiveModel = binding.into();
        active_model.is_deleted = ActiveValue::set(1);
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        let mut update = UserDeviceEntity::update(active_model).filter(UserDeviceColumn::Id.eq(id));
        if let Some(owner) = scope.owner_filter() {
            update = update.filter(UserDeviceColumn::UserId.eq(owner));
        }
        update.exec(&*self.conn).await?;
        Ok(true)
    }
}

impl BaseRepository for SeaOrmDogUserDeviceRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}
