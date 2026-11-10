use crate::business::dog::config::domain::{
    ConfigPageQuery, ConfigVO, CreateConfigRequest, DogConfig, UpdateConfigRequest,
};
use crate::business::dog::config::repository::SeaOrmDogConfigRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;

pub struct DogConfigService {
    repo: SeaOrmDogConfigRepository,
}

impl DogConfigService {
    pub fn new(repo: SeaOrmDogConfigRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateConfigRequest) -> Result<DogConfig, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogConfig, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Config with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: ConfigPageQuery,
    ) -> Result<PageResponse<ConfigVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseError)?;

        Ok(PageResponse::new(
            records.into_iter().map(ConfigVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self) -> Result<Vec<DogConfig>, AppError> {
        self.repo.find_all().await.map_err(AppError::DatabaseError)
    }

    pub async fn update(&self, id: i64, req: UpdateConfigRequest) -> Result<DogConfig, AppError> {
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Config with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseError)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Config with id {} not found",
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
