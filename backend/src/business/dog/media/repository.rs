use crate::business::dog::media::domain::{CreateMediaRequest, MediaPageQuery, UpdateMediaRequest};
use crate::business::dog::media::entity::ActiveModel as MediaActiveModel;
use crate::business::dog::media::entity::Column as MediaColumn;
use crate::business::dog::media::entity::Entity as MediaEntity;
use crate::business::dog::media::entity::Model as Media;
use crate::common::base::{BaseRepository, order_desc};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogMediaRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogMediaRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl BaseRepository for SeaOrmDogMediaRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmDogMediaRepository {
    pub async fn create(&self, req: &CreateMediaRequest, id: i64) -> Result<Media, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        MediaEntity::insert(active_model).exec(&*self.conn).await?;
        MediaEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<Media>, sea_orm::DbErr> {
        MediaEntity::find()
            .filter(MediaColumn::Id.eq(id))
            .filter(MediaColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_all_with_page(
        &self,
        query: &MediaPageQuery,
    ) -> Result<(Vec<Media>, i64), sea_orm::DbErr> {
        let base_query = MediaEntity::find().filter(MediaColumn::IsDeleted.eq(0));
        let mut cond = Condition::all();
        if let Some(ref name) = query.media_name {
            cond = cond.add(MediaColumn::MediaName.contains(name.as_str()));
        }
        if let Some(ref mtype) = query.media_type {
            cond = cond.add(MediaColumn::MediaType.eq(mtype.as_str()));
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
            .order_by(MediaColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateMediaRequest,
    ) -> Result<Option<Media>, sea_orm::DbErr> {
        let entity = MediaEntity::find()
            .filter(MediaColumn::Id.eq(id))
            .filter(MediaColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: MediaActiveModel = entity.into();
        if let Some(name) = &req.media_name {
            active_model.media_name = ActiveValue::set(name.clone());
        }
        if let Some(mtype) = &req.media_type {
            active_model.media_type = ActiveValue::set(Some(mtype.clone()));
        }
        if let Some(path) = &req.media_path {
            active_model.media_path = ActiveValue::set(Some(path.clone()));
        }
        if let Some(size) = req.media_size {
            active_model.media_size = ActiveValue::set(Some(size));
        }
        if let Some(remarks) = &req.remarks {
            active_model.remarks = ActiveValue::set(Some(remarks.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        MediaEntity::update(active_model).exec(&*self.conn).await?;
        self.find_by_id(id).await
    }

    pub async fn find_all(&self) -> Result<Vec<Media>, sea_orm::DbErr> {
        MediaEntity::find()
            .filter(MediaColumn::IsDeleted.eq(0))
            .order_by(MediaColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let media = self.find_by_id(id).await?;
        if let Some(mut media) = media {
            media.is_deleted = 1;
            media.update_time = Some(chrono::Utc::now());
            let mut active_model: MediaActiveModel = media.into();
            active_model.is_deleted = ActiveValue::Set(1);
            MediaEntity::update(active_model).exec(&*self.conn).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
