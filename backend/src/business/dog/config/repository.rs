use crate::business::dog::config::domain::{
    ConfigPageQuery, CreateConfigRequest, UpdateConfigRequest,
};
use crate::business::dog::config::entity::ActiveModel as ConfigActiveModel;
use crate::business::dog::config::entity::Column as ConfigColumn;
use crate::business::dog::config::entity::Entity as ConfigEntity;
use crate::business::dog::config::entity::Model as DogConfig;
use crate::common::base::{BaseRepository, order_desc};
use crate::common::util::parse_beijing_datetime;
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogConfigRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogConfigRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl BaseRepository for SeaOrmDogConfigRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmDogConfigRepository {
    pub async fn create(
        &self,
        req: &CreateConfigRequest,
        id: i64,
    ) -> Result<DogConfig, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        ConfigEntity::insert(active_model).exec(&*self.conn).await?;
        ConfigEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<DogConfig>, sea_orm::DbErr> {
        ConfigEntity::find()
            .filter(ConfigColumn::Id.eq(id))
            .filter(ConfigColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_all_with_page(
        &self,
        query: &ConfigPageQuery,
    ) -> Result<(Vec<DogConfig>, i64), sea_orm::DbErr> {
        let base_query = ConfigEntity::find().filter(ConfigColumn::IsDeleted.eq(0));
        let mut cond = Condition::all();
        if let Some(ref name) = query.config_name {
            cond = cond.add(ConfigColumn::ConfigName.contains(name.as_str()));
        }

        if let Some(ref begin) = query.begin_time
            && let Some(utc_dt) = parse_beijing_datetime(begin, false)
        {
            cond = cond.add(ConfigColumn::CreateTime.gte(utc_dt));
        }
        if let Some(ref end) = query.end_time
            && let Some(utc_dt) = parse_beijing_datetime(end, true)
        {
            cond = cond.add(ConfigColumn::CreateTime.lte(utc_dt));
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
            .order_by(ConfigColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateConfigRequest,
    ) -> Result<Option<DogConfig>, sea_orm::DbErr> {
        let entity = ConfigEntity::find()
            .filter(ConfigColumn::Id.eq(id))
            .filter(ConfigColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: ConfigActiveModel = entity.into();
        if let Some(name) = &req.config_name {
            active_model.config_name = ActiveValue::set(name.clone());
        }
        if let Some(value) = &req.config_value {
            active_model.config_value = ActiveValue::set(Some(value.clone()));
        }
        if let Some(desc) = &req.description {
            active_model.description = ActiveValue::set(Some(desc.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        ConfigEntity::update(active_model).exec(&*self.conn).await?;
        self.find_by_id(id).await
    }

    pub async fn find_all(&self) -> Result<Vec<DogConfig>, sea_orm::DbErr> {
        ConfigEntity::find()
            .filter(ConfigColumn::IsDeleted.eq(0))
            .order_by(ConfigColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let config = self.find_by_id(id).await?;
        if let Some(mut config) = config {
            config.is_deleted = 1;
            config.update_time = Some(chrono::Utc::now());
            let mut active_model: ConfigActiveModel = config.into();
            active_model.is_deleted = ActiveValue::Set(1);
            ConfigEntity::update(active_model).exec(&*self.conn).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
