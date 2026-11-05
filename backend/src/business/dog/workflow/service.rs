use crate::business::dog::workflow::domain::{
    CreateWorkflowRequest, DogWorkflow, UpdateWorkflowRequest,
};
use crate::business::dog::workflow::repository::SeaOrmDogWorkflowRepository;
use crate::common::error::AppError;

pub struct DogWorkflowService {
    repo: SeaOrmDogWorkflowRepository,
}

impl DogWorkflowService {
    pub fn new(repo: SeaOrmDogWorkflowRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateWorkflowRequest) -> Result<DogWorkflow, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogWorkflow, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Workflow with id {} not found", id)))
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> Result<DogWorkflow, AppError> {
        self.repo
            .find_by_uuid(uuid)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Workflow with uuid {} not found", uuid)))
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateWorkflowRequest,
    ) -> Result<DogWorkflow, AppError> {
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Workflow with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Workflow with id {} not found",
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
