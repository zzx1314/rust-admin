use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::traits::{RoleRepository, UserRepository};
use crate::common::util::{decrypt_password, md5_encrypt};
use crate::user::domain::{CreateUserRequest, UpdateUserRequest, User, UserPageQuery, UserVO};
use std::sync::Arc;

#[derive(Debug, serde::Deserialize)]
pub struct PasswordUpdateRequest {
    pub old_password: Option<String>,
    pub password: String,
    pub user_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct PasswordUpdateResponse {
    pub success: bool,
    pub msg: String,
}

pub struct UserService {
    user_repo: Arc<dyn UserRepository>,
    role_repo: Arc<dyn RoleRepository>,
}

impl UserService {
    pub fn new(user_repo: Arc<dyn UserRepository>, role_repo: Arc<dyn RoleRepository>) -> Self {
        Self { user_repo, role_repo }
    }

    pub async fn create_user(&self, req: CreateUserRequest) -> Result<User, AppError> {
        let id = self.generate_id().await;
        let mut req = req;
        if let Some(ref password) = req.password {
            let decrypted = decrypt_password(password)
                .map_err(|e| AppError::BadRequest(format!("Password decryption failed: {}", e)))?;
            req.password = Some(md5_encrypt(&decrypted));
        }
        self.user_repo
            .create(&req, &id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_user(&self, id: &str) -> Result<User, AppError> {
        self.user_repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("User with id {} not found", id)))
    }

    pub async fn get_all_users(&self) -> Result<Vec<User>, AppError> {
        self.user_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_users_page(
        &self,
        query: UserPageQuery,
    ) -> Result<PageResponse<UserVO>, AppError> {
        let (records, total) = self
            .user_repo
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

    pub async fn update_user(&self, id: &str, req: UpdateUserRequest) -> Result<User, AppError> {
        self.user_repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("User with id {} not found", id)))
    }

    pub async fn delete_user(&self, id: &str) -> Result<(), AppError> {
        let deleted = self
            .user_repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if !deleted {
            return Err(AppError::NotFound(format!("User with id {} not found", id)));
        }
        Ok(())
    }

    pub async fn find_by_username(&self, username: &str) -> Result<User, AppError> {
        self.user_repo
            .find_by_username(username)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::Unauthorized("Invalid username or password".to_string()))
    }

    pub async fn update_password(&self, user_id: &str, old_password: Option<&str>, new_password: &str) -> Result<PasswordUpdateResponse, AppError> {
        let user = self.user_repo.find_by_id(user_id).await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

        if let (Some(old_pwd), Some(password_hash)) = (old_password, &user.password)
            && !crate::common::util::md5_verify(old_pwd, password_hash)
        {
            return Err(AppError::BadRequest("Old password is incorrect".to_string()));
        }

        let decrypted = decrypt_password(new_password)
            .map_err(|e| AppError::BadRequest(format!("Password decryption failed: {}", e)))?;
        let hashed = md5_encrypt(&decrypted);

        let req = UpdateUserRequest {
            username: None,
            phone: None,
            email: None,
            real_name: None,
            password: Some(hashed),
            org_id: None,
            remarks: None,
            card: None,
            is_show: None,
            enable: None,
            sex: None,
        };

        self.user_repo.update(user_id, &req).await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PasswordUpdateResponse {
            success: true,
            msg: "Password updated successfully".to_string(),
        })
    }

    pub async fn get_users_by_role(&self, role_id: &str) -> Result<Vec<User>, AppError> {
        self.role_repo.find_users_by_role_id(role_id).await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn reset_password(&self, user_id: &str, password: &str) -> Result<(), AppError> {
        let hashed = md5_encrypt(password);
        let req = UpdateUserRequest {
            username: None,
            phone: None,
            email: None,
            real_name: None,
            password: Some(hashed),
            org_id: None,
            remarks: None,
            card: None,
            is_show: None,
            enable: None,
            sex: None,
        };
        self.user_repo.update(user_id, &req).await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(())
    }

    pub async fn toggle_enable(&self, user_id: &str, enable: i32) -> Result<(), AppError> {
        let req = UpdateUserRequest {
            username: None,
            phone: None,
            email: None,
            real_name: None,
            password: None,
            org_id: None,
            remarks: None,
            card: None,
            is_show: None,
            enable: Some(enable),
            sex: None,
        };
        self.user_repo.update(user_id, &req).await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(())
    }

    async fn generate_id(&self) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .to_string()
    }
}
