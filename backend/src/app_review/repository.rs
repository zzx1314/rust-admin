use crate::app_review::domain::{CreateReviewRequest, ReviewPageQuery};
use crate::app_review::entity::ActiveModel as ReviewActiveModel;
use crate::app_review::entity::Column as ReviewColumn;
use crate::app_review::entity::Entity as ReviewEntity;
use crate::app_review::entity::Model as Review;
use crate::common::base::{order_desc, BaseRepository};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use std::sync::Arc;

pub struct SeaOrmAppReviewRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmAppReviewRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl BaseRepository for SeaOrmAppReviewRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmAppReviewRepository {
    pub async fn create(
        &self,
        req: &CreateReviewRequest,
        id: i64,
        created_by: Option<i64>,
    ) -> Result<Review, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, created_by, now);

        ReviewEntity::insert(active_model).exec(&*self.conn).await?;
        ReviewEntity::find_by_id(id).one(&*self.conn).await.map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<Review>, sea_orm::DbErr> {
        ReviewEntity::find()
            .filter(ReviewColumn::Id.eq(id))
            .filter(ReviewColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_pending_by_artifact(
        &self,
        src_project: &str,
        repository_name: &str,
        tag: &str,
    ) -> Result<Option<Review>, sea_orm::DbErr> {
        ReviewEntity::find()
            .filter(ReviewColumn::SrcProject.eq(src_project))
            .filter(ReviewColumn::RepositoryName.eq(repository_name))
            .filter(ReviewColumn::Tag.eq(tag))
            .filter(ReviewColumn::Status.eq("pending"))
            .filter(ReviewColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_all_with_page(
        &self,
        query: &ReviewPageQuery,
    ) -> Result<(Vec<Review>, i64), sea_orm::DbErr> {
        let mut base_query = ReviewEntity::find().filter(ReviewColumn::IsDeleted.eq(0));

        let mut cond = Condition::all();
        if let Some(src) = &query.src_project {
            cond = cond.add(ReviewColumn::SrcProject.eq(src.as_str()));
        }
        if let Some(repo) = &query.repository_name {
            cond = cond.add(ReviewColumn::RepositoryName.contains(repo.as_str()));
        }
        if let Some(status) = &query.status {
            cond = cond.add(ReviewColumn::Status.eq(status.as_str()));
        }
        base_query = base_query.filter(cond);

        let total = base_query.clone().count(&*self.conn).await?;

        let current = query.current.max(1);
        let size = query.size.max(1);
        let offset = (current - 1) * size;

        let records = base_query
            .order_by(ReviewColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn update_status(
        &self,
        id: i64,
        status: &str,
        reviewer_id: i64,
        comment: Option<String>,
    ) -> Result<Option<Review>, sea_orm::DbErr> {
        let review = self.find_by_id(id).await?;
        if review.is_none() {
            return Ok(None);
        }

        let mut active_model: ReviewActiveModel = review.unwrap().into();
        active_model.status = ActiveValue::set(status.to_string());
        active_model.reviewer_id = ActiveValue::set(Some(reviewer_id));
        active_model.reviewer_comment = ActiveValue::set(comment);
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        ReviewEntity::update(active_model)
            .filter(ReviewColumn::Id.eq(id))
            .filter(ReviewColumn::IsDeleted.eq(0))
            .exec(&*self.conn)
            .await?;

        self.find_by_id(id).await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let review = self.find_by_id(id).await?;
        if let Some(mut review) = review {
            review.is_deleted = 1;
            review.update_time = Some(chrono::Utc::now());
            let mut active_model: ReviewActiveModel = review.into();
            active_model.is_deleted = ActiveValue::Set(1);
            ReviewEntity::update(active_model).exec(&*self.conn).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
