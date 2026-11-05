use crate::business::dog::task::domain::{
    CreateTaskRequest, DogTask, TaskPageQuery, TaskVO, UpdateTaskRequest,
};
use crate::business::dog::task::repository::SeaOrmDogTaskRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::generate_id;

pub struct DogTaskService {
    repo: SeaOrmDogTaskRepository,
}

impl DogTaskService {
    pub fn new(repo: SeaOrmDogTaskRepository) -> Self {
        Self { repo }
    }

    pub async fn create(
        &self,
        req: CreateTaskRequest,
        scope: &DataScope,
    ) -> Result<DogTask, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64, scope: &DataScope) -> Result<DogTask, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Task with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: TaskPageQuery,
        scope: &DataScope,
    ) -> Result<PageResponse<TaskVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(TaskVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self, scope: &DataScope) -> Result<Vec<DogTask>, AppError> {
        self.repo
            .find_all(scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateTaskRequest,
        scope: &DataScope,
    ) -> Result<DogTask, AppError> {
        self.repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Task with id {} not found", id)))
    }

    pub async fn update_status(
        &self,
        id: i64,
        status: &str,
        scope: &DataScope,
    ) -> Result<DogTask, AppError> {
        self.repo
            .update_status(id, status, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Task with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!("Task with id {} not found", id)));
        }
        Ok(())
    }
}
