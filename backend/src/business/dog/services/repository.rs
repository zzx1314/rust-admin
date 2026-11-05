use crate::business::dog::services::domain::{CreateServicesRequest, UpdateServicesRequest};
use crate::business::dog::services::entity::ActiveModel as ServicesActiveModel;
use crate::business::dog::services::entity::Column as ServicesColumn;
use crate::business::dog::services::entity::Entity as ServicesEntity;
use crate::business::dog::services::entity::Model as DogServices;
use crate::common::base::{BaseRepository, order_desc};
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogServicesRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogServicesRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl BaseRepository for SeaOrmDogServicesRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmDogServicesRepository {
    pub async fn create(
        &self,
        req: &CreateServicesRequest,
        id: i64,
    ) -> Result<DogServices, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        ServicesEntity::insert(active_model)
            .exec(&*self.conn)
            .await?;
        ServicesEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<DogServices>, sea_orm::DbErr> {
        ServicesEntity::find()
            .filter(ServicesColumn::Id.eq(id))
            .filter(ServicesColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateServicesRequest,
    ) -> Result<Option<DogServices>, sea_orm::DbErr> {
        let entity = ServicesEntity::find()
            .filter(ServicesColumn::Id.eq(id))
            .filter(ServicesColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: ServicesActiveModel = entity.into();
        if let Some(t) = &req.r#type {
            active_model.r#type = ActiveValue::set(Some(t.clone()));
        }
        if let Some(params) = &req.params {
            active_model.params = ActiveValue::set(Some(params.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        ServicesEntity::update(active_model)
            .exec(&*self.conn)
            .await?;
        self.find_by_id(id).await
    }

    pub async fn find_all(&self) -> Result<Vec<DogServices>, sea_orm::DbErr> {
        ServicesEntity::find()
            .filter(ServicesColumn::IsDeleted.eq(0))
            .order_by(ServicesColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let services = self.find_by_id(id).await?;
        if let Some(mut services) = services {
            services.is_deleted = 1;
            services.update_time = Some(chrono::Utc::now());
            let mut active_model: ServicesActiveModel = services.into();
            active_model.is_deleted = ActiveValue::Set(1);
            ServicesEntity::update(active_model)
                .exec(&*self.conn)
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
