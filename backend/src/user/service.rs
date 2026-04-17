use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::traits::UserRepository;
use crate::common::util::{decrypt_password, md5_encrypt};
use crate::user::domain::{CreateUserRequest, UpdateUserRequest, User, UserPageQuery, UserVO};
use std::sync::Arc;

pub struct UserService {
    user_repo: Arc<dyn UserRepository>,
}

impl UserService {
    pub fn new(user_repo: Arc<dyn UserRepository>) -> Self {
        Self { user_repo }
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

    async fn generate_id(&self) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .to_string()
    }
}
