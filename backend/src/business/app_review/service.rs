use crate::business::app_review::domain::{
    CreateReviewRequest, Review, ReviewPageQuery, ReviewStatus, ReviewVO,
};
use crate::business::app_review::repository::SeaOrmAppReviewRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::business::harbor::service::HarborService;
use std::sync::Arc;

pub struct AppReviewService {
    repo: SeaOrmAppReviewRepository,
    harbor_service: Arc<HarborService>,
}

impl AppReviewService {
    pub fn new(repo: SeaOrmAppReviewRepository, harbor_service: Arc<HarborService>) -> Self {
        Self { repo, harbor_service }
    }

    pub async fn create_review(
        &self,
        req: CreateReviewRequest,
        created_by: Option<i64>,
    ) -> Result<Review, AppError> {
        let existing = self
            .repo
            .find_pending_by_artifact(&req.src_project, &req.repository_name, &req.tag)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if existing.is_some() {
            return Err(AppError::Conflict(
                "A pending review already exists for this artifact".to_string(),
            ));
        }

        let id = self.generate_id();
        self.repo
            .create(&req, id, created_by)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    /// Create a new review for an artifact pushed from Harbor.
    ///
    /// If an approved review already exists for the exact same artifact digest,
    /// returns `Ok(None)` so the caller can skip creating a duplicate review.
    /// If the digest differs (or no digest is provided), a new pending review is
    /// created normally.
    pub async fn create_review_with_dedup(
        &self,
        req: CreateReviewRequest,
        created_by: Option<i64>,
    ) -> Result<Option<Review>, AppError> {
        if let Some(digest) = req.digest.as_deref().filter(|d| !d.is_empty()) {
            let existing = self
                .repo
                .find_approved_by_artifact_digest(
                    &req.src_project,
                    &req.repository_name,
                    &req.tag,
                    digest,
                )
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;

            if existing.is_some() {
                return Ok(None);
            }
        }

        self.create_review(req, created_by).await.map(Some)
    }

    pub async fn get_review(&self, id: i64) -> Result<Review, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Review with id {} not found", id)))
    }

    pub async fn list_reviews(
        &self,
        query: ReviewPageQuery,
    ) -> Result<PageResponse<ReviewVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(ReviewVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn approve_review(
        &self,
        id: i64,
        reviewer_id: i64,
        comment: Option<String>,
    ) -> Result<Review, AppError> {
        let review = self.get_review(id).await?;
        if review.status != ReviewStatus::Pending.as_str() {
            return Err(AppError::BadRequest("Only pending reviews can be approved".to_string()));
        }

        self.harbor_service
            .replicate_artifact(&review.src_project, &review.dest_project, &review.repository_name, &review.tag)
            .await?;

        let updated = self
            .repo
            .update_status(id, ReviewStatus::Approved.as_str(), reviewer_id, comment)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Review with id {} not found", id)))?;

        Ok(updated)
    }

    pub async fn reject_review(
        &self,
        id: i64,
        reviewer_id: i64,
        comment: Option<String>,
    ) -> Result<Review, AppError> {
        let review = self.get_review(id).await?;
        if review.status != ReviewStatus::Pending.as_str() {
            return Err(AppError::BadRequest("Only pending reviews can be rejected".to_string()));
        }

        let updated = self
            .repo
            .update_status(id, ReviewStatus::Rejected.as_str(), reviewer_id, comment)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Review with id {} not found", id)))?;

        Ok(updated)
    }

    pub async fn delete_review(&self, id: i64) -> Result<(), AppError> {
        let deleted = self.repo.delete(id).await.map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!("Review with id {} not found", id)));
        }
        Ok(())
    }

    fn generate_id(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}
