use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::traits::SysLogRepository;
use crate::system::sys_log::domain::{
    CreateSysLogRequest, SysLog, SysLogPageQuery, SysLogVO, UpdateSysLogRequest,
};
use std::sync::Arc;

pub struct SysLogService {
    log_repo: Arc<dyn SysLogRepository>,
}

impl SysLogService {
    pub fn new(log_repo: Arc<dyn SysLogRepository>) -> Self {
        Self { log_repo }
    }

    pub async fn create_log(&self, req: CreateSysLogRequest) -> Result<SysLog, AppError> {
        let id = self.generate_id().await;
        self.log_repo
            .create(&req, &id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_log(&self, id: &i64) -> Result<SysLog, AppError> {
        self.log_repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("SysLog with id {} not found", id)))
    }

    pub async fn get_all_logs(&self) -> Result<Vec<SysLog>, AppError> {
        self.log_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_logs_page(
        &self,
        query: SysLogPageQuery,
    ) -> Result<PageResponse<SysLogVO>, AppError> {
        let (records, total) = self
            .log_repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(PageResponse::new(
            records,
            total,
            query.page(),
            query.size(),
        ))
    }

    pub async fn update_log(
        &self,
        id: &i64,
        req: UpdateSysLogRequest,
    ) -> Result<SysLog, AppError> {
        self.log_repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("SysLog with id {} not found", id)))
    }

    pub async fn delete_log(&self, id: &i64) -> Result<(), AppError> {
        let deleted = self
            .log_repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if !deleted {
            return Err(AppError::NotFound(format!(
                "SysLog with id {} not found",
                id
            )));
        }
        Ok(())
    }

    async fn generate_id(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}