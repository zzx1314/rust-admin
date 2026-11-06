use crate::business::dog::model::domain::{CreateModelRequest, ModelPageQuery, UpdateModelRequest};
use crate::business::dog::model::entity::ActiveModel as ModelActiveModel;
use crate::business::dog::model::entity::Column as ModelColumn;
use crate::business::dog::model::entity::Entity as ModelEntity;
use crate::business::dog::model::entity::Model as DogModel;
use crate::common::base::{BaseRepository, order_desc};
use crate::common::util::parse_beijing_datetime;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogModelRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogModelRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl BaseRepository for SeaOrmDogModelRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmDogModelRepository {
    pub async fn create(
        &self,
        req: &CreateModelRequest,
        id: i64,
    ) -> Result<DogModel, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        ModelEntity::insert(active_model).exec(&*self.conn).await?;
        ModelEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<DogModel>, sea_orm::DbErr> {
        ModelEntity::find()
            .filter(ModelColumn::Id.eq(id))
            .filter(ModelColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_all_with_page(
        &self,
        query: &ModelPageQuery,
    ) -> Result<(Vec<DogModel>, i64), sea_orm::DbErr> {
        let base_query = ModelEntity::find().filter(ModelColumn::IsDeleted.eq(0));
        let mut cond = Condition::all();
        if let Some(ref name) = query.model_name {
            cond = cond.add(ModelColumn::ModelName.contains(name.as_str()));
        }
        if let Some(ref mtype) = query.model_type {
            cond = cond.add(ModelColumn::ModelType.eq(mtype.as_str()));
        }

        if let Some(ref begin) = query.begin_time
            && let Some(utc_dt) = parse_beijing_datetime(begin, false)
        {
            cond = cond.add(ModelColumn::CreateTime.gte(utc_dt));
        }
        if let Some(ref end) = query.end_time
            && let Some(utc_dt) = parse_beijing_datetime(end, true)
        {
            cond = cond.add(ModelColumn::CreateTime.lte(utc_dt));
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
            .order_by(ModelColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateModelRequest,
    ) -> Result<Option<DogModel>, sea_orm::DbErr> {
        let entity = ModelEntity::find()
            .filter(ModelColumn::Id.eq(id))
            .filter(ModelColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: ModelActiveModel = entity.into();
        if let Some(name) = &req.model_name {
            active_model.model_name = ActiveValue::set(name.clone());
        }
        if let Some(mtype) = &req.model_type {
            active_model.model_type = ActiveValue::set(Some(mtype.clone()));
        }
        if let Some(fname) = &req.file_name {
            active_model.file_name = ActiveValue::set(Some(fname.clone()));
        }
        if let Some(fpath) = &req.file_path {
            active_model.file_path = ActiveValue::set(Some(fpath.clone()));
        }
        if let Some(fformat) = &req.file_format {
            active_model.file_format = ActiveValue::set(Some(fformat.clone()));
        }
        if let Some(fsize) = req.file_size {
            active_model.file_size = ActiveValue::set(Some(fsize));
        }
        if let Some(remarks) = &req.remarks {
            active_model.remarks = ActiveValue::set(Some(remarks.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        ModelEntity::update(active_model).exec(&*self.conn).await?;
        self.find_by_id(id).await
    }

    pub async fn find_all(&self) -> Result<Vec<DogModel>, sea_orm::DbErr> {
        ModelEntity::find()
            .filter(ModelColumn::IsDeleted.eq(0))
            .order_by(ModelColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn count(&self) -> Result<i64, sea_orm::DbErr> {
        ModelEntity::find()
            .filter(ModelColumn::IsDeleted.eq(0))
            .count(&*self.conn)
            .await
            .map(|c| c as i64)
    }

    /// Creation timestamps at or after `since`, used by dashboard statistics.
    pub async fn find_create_times_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<DateTime<Utc>>, sea_orm::DbErr> {
        ModelEntity::find()
            .filter(ModelColumn::IsDeleted.eq(0))
            .filter(ModelColumn::CreateTime.gte(since))
            .select_only()
            .column(ModelColumn::CreateTime)
            .into_tuple::<DateTime<Utc>>()
            .all(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let model = self.find_by_id(id).await?;
        if let Some(mut model) = model {
            model.is_deleted = 1;
            model.update_time = Some(chrono::Utc::now());
            let mut active_model: ModelActiveModel = model.into();
            active_model.is_deleted = ActiveValue::Set(1);
            ModelEntity::update(active_model).exec(&*self.conn).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
