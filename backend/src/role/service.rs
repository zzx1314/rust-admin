use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::traits::RoleRepository;
use crate::role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use crate::user::domain::User;
use std::sync::Arc;

pub struct RoleService {
    role_repo: Arc<dyn RoleRepository>,
}

impl RoleService {
    pub fn new(role_repo: Arc<dyn RoleRepository>) -> Self {
        Self { role_repo }
    }

    pub async fn create_role(&self, req: CreateRoleRequest) -> Result<Role, AppError> {
        let id = self.generate_id().await;
        self.role_repo
            .create(&req, &id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_role(&self, id: &i64) -> Result<Role, AppError> {
        self.role_repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Role with id {} not found", id)))
    }

    pub async fn get_all_roles(&self) -> Result<Vec<Role>, AppError> {
        self.role_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_roles_page(&self, req: RolePageQuery) -> Result<PageResponse<Role>, AppError> {
        let (records, total) = self
            .role_repo
            .find_all_with_page(&req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(PageResponse::new(records, total, req.page(), req.size()))
    }

    pub async fn update_role(&self, id: &i64, req: UpdateRoleRequest) -> Result<Role, AppError> {
        self.role_repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Role with id {} not found", id)))
    }

    pub async fn delete_role(&self, id: &i64) -> Result<(), AppError> {
        let deleted = self
            .role_repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if !deleted {
            return Err(AppError::NotFound(format!("Role with id {} not found", id)));
        }
        Ok(())
    }

    pub async fn assign_role(&self, user_id: &i64, role_id: &i64) -> Result<(), AppError> {
        self.get_role(role_id).await?;
        self.role_repo
            .assign_role_to_user(user_id, role_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn remove_role(&self, user_id: &i64, role_id: &i64) -> Result<(), AppError> {
        let removed = self
            .role_repo
            .remove_role_from_user(user_id, role_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if !removed {
            return Err(AppError::NotFound(format!(
                "Role assignment not found for user {} and role {}",
                user_id, role_id
            )));
        }
        Ok(())
    }

    pub async fn get_roles_for_user(&self, user_id: &i64) -> Result<Vec<Role>, AppError> {
        self.role_repo
            .find_roles_by_user_id(user_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_users_for_role(&self, role_id: &i64) -> Result<Vec<User>, AppError> {
        self.role_repo
            .find_users_by_role_id(role_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    async fn generate_id(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}