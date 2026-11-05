use crate::business::dog::command::domain::{
    CommandPageQuery, CommandVO, CreateCommandRequest, DogCommand, UpdateCommandRequest,
};
use crate::business::dog::command::repository::SeaOrmDogCommandRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;

pub struct DogCommandService {
    repo: SeaOrmDogCommandRepository,
}

impl DogCommandService {
    pub fn new(repo: SeaOrmDogCommandRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateCommandRequest) -> Result<DogCommand, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogCommand, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Command with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: CommandPageQuery,
    ) -> Result<PageResponse<CommandVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(CommandVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self) -> Result<Vec<DogCommand>, AppError> {
        self.repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(&self, id: i64, req: UpdateCommandRequest) -> Result<DogCommand, AppError> {
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Command with id {} not found", id)))
    }

    pub async fn update_status(
        &self,
        id: i64,
        status: &str,
        return_value: Option<String>,
    ) -> Result<DogCommand, AppError> {
        self.repo
            .update_status(id, status, return_value)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Command with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Command with id {} not found",
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
