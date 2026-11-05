use crate::business::dog::model::domain::{
    CreateModelRequest, DogModel, ModelPageQuery, ModelVO, UpdateModelRequest,
};
use crate::business::dog::model::repository::SeaOrmDogModelRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;

pub struct DogModelService {
    repo: SeaOrmDogModelRepository,
}

impl DogModelService {
    pub fn new(repo: SeaOrmDogModelRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateModelRequest) -> Result<DogModel, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogModel, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Model with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: ModelPageQuery,
    ) -> Result<PageResponse<ModelVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(ModelVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self) -> Result<Vec<DogModel>, AppError> {
        self.repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(&self, id: i64, req: UpdateModelRequest) -> Result<DogModel, AppError> {
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Model with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Model with id {} not found",
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
