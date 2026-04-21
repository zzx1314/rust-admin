use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::traits::SysDictItemRepository;
use crate::sys_dict_item::domain::{CreateSysDictItemRequest, SysDictItem, SysDictItemPageQuery, SysDictItemVO, UpdateSysDictItemRequest};
use std::sync::Arc;

pub struct SysDictItemService {
    dict_item_repo: Arc<dyn SysDictItemRepository>,
}

impl SysDictItemService {
    pub fn new(dict_item_repo: Arc<dyn SysDictItemRepository>) -> Self {
        Self { dict_item_repo }
    }

    pub async fn create_dict_item(&self, req: CreateSysDictItemRequest) -> Result<SysDictItem, AppError> {
        let id = self.generate_id().await;
        self.dict_item_repo
            .create(&req, &id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_dict_item(&self, id: &i64) -> Result<SysDictItem, AppError> {
        self.dict_item_repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("SysDictItem with id {} not found", id)))
    }

    pub async fn get_all_dict_items(&self) -> Result<Vec<SysDictItem>, AppError> {
        self.dict_item_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_dict_items_by_dict_id(&self, dict_id: &i64) -> Result<Vec<SysDictItem>, AppError> {
        self.dict_item_repo
            .find_by_dict_id(dict_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_dict_items_by_type(&self, r#type: &str) -> Result<Vec<SysDictItem>, AppError> {
        self.dict_item_repo
            .find_by_type(r#type)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_dict_items_page(
        &self,
        query: SysDictItemPageQuery,
    ) -> Result<PageResponse<SysDictItemVO>, AppError> {
        let (records, total) = self
            .dict_item_repo
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

    pub async fn update_dict_item(&self, id: &i64, req: UpdateSysDictItemRequest) -> Result<SysDictItem, AppError> {
        self.dict_item_repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("SysDictItem with id {} not found", id)))
    }

    pub async fn delete_dict_item(&self, id: &i64) -> Result<(), AppError> {
        let deleted = self
            .dict_item_repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if !deleted {
            return Err(AppError::NotFound(format!("SysDictItem with id {} not found", id)));
        }
        Ok(())
    }

    pub async fn get_safe_policy(&self) -> Result<std::collections::HashMap<String, String>, AppError> {
        let items = self
            .dict_item_repo
            .find_by_type("sys_security_policy")
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let mut result = std::collections::HashMap::new();
        for item in items {
            let value = item.value.unwrap_or_default();
            if item.r#type == "sysOvertime" {
                let parsed: i64 = value.parse().unwrap_or(0);
                result.insert(item.r#type, (parsed + 1).to_string());
            } else {
                result.insert(item.r#type, value);
            }
        }
        Ok(result)
    }

    async fn generate_id(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}