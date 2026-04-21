use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::traits::SysDictRepository;
use crate::system::sys_dict::domain::{
    CreateSysDictRequest, SysDict, SysDictPageQuery, SysDictVO, UpdateSysDictRequest,
};
use std::sync::Arc;

pub struct SysDictService {
    dict_repo: Arc<dyn SysDictRepository>,
}

impl SysDictService {
    pub fn new(dict_repo: Arc<dyn SysDictRepository>) -> Self {
        Self { dict_repo }
    }

    pub async fn create_dict(&self, req: CreateSysDictRequest) -> Result<SysDict, AppError> {
        let id = self.generate_id().await;
        self.dict_repo
            .create(&req, &id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_dict(&self, id: &i64) -> Result<SysDict, AppError> {
        self.dict_repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("SysDict with id {} not found", id)))
    }

    pub async fn get_all_dicts(&self) -> Result<Vec<SysDict>, AppError> {
        self.dict_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_dicts_page(
        &self,
        query: SysDictPageQuery,
    ) -> Result<PageResponse<SysDictVO>, AppError> {
        let (records, total) = self
            .dict_repo
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

    pub async fn update_dict(
        &self,
        id: &i64,
        req: UpdateSysDictRequest,
    ) -> Result<SysDict, AppError> {
        self.dict_repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("SysDict with id {} not found", id)))
    }

    pub async fn delete_dict(&self, id: &i64) -> Result<(), AppError> {
        let deleted = self
            .dict_repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if !deleted {
            return Err(AppError::NotFound(format!(
                "SysDict with id {} not found",
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
