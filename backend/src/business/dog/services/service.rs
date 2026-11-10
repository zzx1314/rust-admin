use crate::business::dog::services::domain::{
    CreateServicesRequest, DogServices, ServicesQuery, UpdateServicesRequest,
};
use crate::business::dog::services::repository::SeaOrmDogServicesRepository;
use crate::common::error::AppError;

pub struct DogServicesService {
    repo: SeaOrmDogServicesRepository,
}

impl DogServicesService {
    pub fn new(repo: SeaOrmDogServicesRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateServicesRequest) -> Result<DogServices, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogServices, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Service with id {} not found", id)))
    }

    pub async fn list_all(&self, query: &ServicesQuery) -> Result<Vec<DogServices>, AppError> {
        self.repo
            .find_all(query)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateServicesRequest,
    ) -> Result<DogServices, AppError> {
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Service with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseError)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Service with id {} not found",
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
