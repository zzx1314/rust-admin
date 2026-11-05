use crate::business::dog::media::domain::{
    CreateMediaRequest, DogMedia, MediaPageQuery, MediaVO, UpdateMediaRequest,
};
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;

pub struct DogMediaService {
    repo: SeaOrmDogMediaRepository,
}

impl DogMediaService {
    pub fn new(repo: SeaOrmDogMediaRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateMediaRequest) -> Result<DogMedia, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogMedia, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Media with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: MediaPageQuery,
    ) -> Result<PageResponse<MediaVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(MediaVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self) -> Result<Vec<DogMedia>, AppError> {
        self.repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(&self, id: i64, req: UpdateMediaRequest) -> Result<DogMedia, AppError> {
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Media with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Media with id {} not found",
                id
            )));
        }
        Ok(())
    }
}

fn generate_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
