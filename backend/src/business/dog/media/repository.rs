use crate::business::dog::media::domain::{CreateMediaRequest, MediaPageQuery, UpdateMediaRequest};
use crate::business::dog::media::entity::ActiveModel as MediaActiveModel;
use crate::business::dog::media::entity::Column as MediaColumn;
use crate::business::dog::media::entity::Entity as MediaEntity;
use crate::business::dog::media::entity::Model as Media;
use crate::common::base::{BaseRepository, order_desc};
use crate::common::util::parse_beijing_datetime;
use chrono::{DateTime, Utc};
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

    /// 按原图 SHA-256 查重：设备重复回传同一张图时复用已有记录，不新增一行。
    pub async fn find_by_sha256(&self, sha256: &str) -> Result<Option<Media>, sea_orm::DbErr> {
        MediaEntity::find()
            .filter(MediaColumn::Sha256.eq(sha256))
            .filter(MediaColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    /// 同 [`Self::find_by_sha256`]，但**包含软删除**的行。
    ///
    /// `sha256` 上是普通唯一索引（迁移 32），已删除的记录仍然占着这个值：
    /// 同一张图再次回传时必须恢复它，否则插入会撞唯一索引。
    pub async fn find_by_sha256_any(&self, sha256: &str) -> Result<Option<Media>, sea_orm::DbErr> {
        // 历史库可能残留同一 sha 的多行（迁移 33 前的软删除重传）：优先未删除的那条，
        // 再按 id 取最新；`limit(1)` 顺便避免 `one()` 在多行时直接报错。
        MediaEntity::find()
            .filter(MediaColumn::Sha256.eq(sha256))
            .order_by_asc(MediaColumn::IsDeleted)
            .order_by_desc(MediaColumn::Id)
            .limit(1)
            .one(&*self.conn)
            .await
    }

    /// 恢复一条被软删除的媒体记录（同一张图重新回传时用）。
    pub async fn revive(
        &self,
        id: i64,
        req: &CreateMediaRequest,
    ) -> Result<Media, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        MediaEntity::update(MediaActiveModel {
            id: ActiveValue::set(id),
            media_name: ActiveValue::set(req.media_name.clone()),
            media_type: ActiveValue::set(req.media_type.clone()),
            media_path: ActiveValue::set(req.media_path.clone()),
            media_size: ActiveValue::set(req.media_size),
            remarks: ActiveValue::set(req.remarks.clone()),
            device_code: ActiveValue::set(req.device_code.clone()),
            park_code: ActiveValue::set(req.park_code.clone()),
            plan_id: ActiveValue::set(req.plan_id.clone()),
            point_index: ActiveValue::set(req.point_index),
            captured_at: ActiveValue::set(req.captured_at.clone()),
            source: ActiveValue::set(req.source.clone()),
            sha256: ActiveValue::set(req.sha256.clone()),
            create_time: ActiveValue::not_set(),
            update_time: ActiveValue::set(Some(now)),
            is_deleted: ActiveValue::set(0),
        })
        .exec(&*self.conn)
        .await?;
        MediaEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|row| row.expect("刚更新过的行必然存在"))
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
        if let Some(ref device) = query.device_code
            && !device.trim().is_empty()
        {
            cond = cond.add(MediaColumn::DeviceCode.eq(device.trim()));
        }
        if let Some(ref plan) = query.plan_id
            && !plan.trim().is_empty()
        {
            cond = cond.add(MediaColumn::PlanId.eq(plan.trim()));
        }
        if let Some(ref source) = query.source
            && !source.trim().is_empty()
        {
            cond = cond.add(MediaColumn::Source.eq(source.trim()));
        }

        if let Some(ref begin) = query.begin_time
            && let Some(utc_dt) = parse_beijing_datetime(begin, false)
        {
            cond = cond.add(MediaColumn::CreateTime.gte(utc_dt));
        }
        if let Some(ref end) = query.end_time
            && let Some(utc_dt) = parse_beijing_datetime(end, true)
        {
            cond = cond.add(MediaColumn::CreateTime.lte(utc_dt));
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

    pub async fn count(&self) -> Result<i64, sea_orm::DbErr> {
        MediaEntity::find()
            .filter(MediaColumn::IsDeleted.eq(0))
            .count(&*self.conn)
            .await
            .map(|c| c as i64)
    }

    /// Creation timestamps at or after `since`, used by dashboard statistics.
    pub async fn find_create_times_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<DateTime<Utc>>, sea_orm::DbErr> {
        MediaEntity::find()
            .filter(MediaColumn::IsDeleted.eq(0))
            .filter(MediaColumn::CreateTime.gte(since))
            .select_only()
            .column(MediaColumn::CreateTime)
            .into_tuple::<DateTime<Utc>>()
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
