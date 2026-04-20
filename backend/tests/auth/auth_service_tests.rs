use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::auth::service::AuthService;
use x_rust::common::error::AppError;
use x_rust::common::traits::{
    DynFuture, RoleRepository, SeaOrmOptResult, SeaOrmResult, TokenStore, UserRepository,
};
use x_rust::common::util::md5_encrypt;
use x_rust::role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use x_rust::user::domain::{CreateUserRequest, User, UserPageQuery, UserVO};

// ==================== Fake User Repository ====================

struct FakeUserRepository {
    users: Arc<Mutex<HashMap<i64, User>>>,
}

impl FakeUserRepository {
    fn new() -> Self {
        Self {
            users: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[allow(dead_code)]
    fn insert_user(&self, user: User) {
        self.users.lock().unwrap().insert(user.id, user);
    }
}

impl UserRepository for FakeUserRepository {
    fn create(&self, req: &CreateUserRequest, id: &i64) -> DynFuture<SeaOrmResult<User>> {
        let users = self.users.clone();
        let req = req.clone();
        let id = *id;
        Box::pin(async move {
            let user = User {
                id,
                username: req.username.clone(),
                phone: req.phone.clone(),
                email: req.email.clone(),
                real_name: req.real_name.clone(),
                password: req.password.clone(),
                org_id: req.org_id,
                lock_time: None,
                last_login_time: None,
                try_count: Some(0),
                lock_flag: Some(1),
                create_time: Utc::now(),
                update_time: Utc::now(),
                is_deleted: 0,
                remarks: req.remarks.clone(),
                pass_update_time: None,
                card: req.card.clone(),
                is_show: Some(1),
                enable: Some(1),
                first_login: Some(1),
                sex: req.sex.clone(),
            };
            users.lock().unwrap().insert(id, user.clone());
            Ok(user)
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<User>> {
        let users = self.users.clone();
        let id = *id;
        Box::pin(async move { Ok(users.lock().unwrap().get(&id).cloned()) })
    }

    fn find_by_email(&self, email: &str) -> DynFuture<SeaOrmOptResult<User>> {
        let users = self.users.clone();
        let email = email.to_string();
        Box::pin(async move {
            Ok(users
                .lock()
                .unwrap()
                .values()
                .find(|u| u.email.as_deref() == Some(&email))
                .cloned())
        })
    }

    fn find_by_username(&self, username: &str) -> DynFuture<SeaOrmOptResult<User>> {
        let users = self.users.clone();
        let username = username.to_string();
        Box::pin(async move {
            Ok(users
                .lock()
                .unwrap()
                .values()
                .find(|u| u.username == username)
                .cloned())
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<User>>> {
        let users = self.users.clone();
        Box::pin(async move { Ok(users.lock().unwrap().values().cloned().collect()) })
    }

    fn update(
        &self,
        id: &i64,
        req: &x_rust::user::domain::UpdateUserRequest,
    ) -> DynFuture<SeaOrmOptResult<User>> {
        let users = self.users.clone();
        let id = *id;
        let username = req.username.clone();
        let phone = req.phone.clone();
        let email = req.email.clone();
        let real_name = req.real_name.clone();
        let org_id = req.org_id;
        let remarks = req.remarks.clone();
        let card = req.card.clone();
        let is_show = req.is_show;
        let enable = req.enable;
        let sex = req.sex.clone();
        Box::pin(async move {
            let mut users_lock = users.lock().unwrap();
            if let Some(user) = users_lock.get_mut(&id) {
                if let Some(v) = username {
                    user.username = v;
                }
                if let Some(v) = phone {
                    user.phone = Some(v);
                }
                if let Some(v) = email {
                    user.email = Some(v);
                }
                if let Some(v) = real_name {
                    user.real_name = Some(v);
                }
                user.org_id = org_id;
                if let Some(v) = remarks {
                    user.remarks = Some(v);
                }
                if let Some(v) = card {
                    user.card = Some(v);
                }
                if let Some(v) = is_show {
                    user.is_show = Some(v);
                }
                if let Some(v) = enable {
                    user.enable = Some(v);
                }
                if let Some(v) = sex {
                    user.sex = Some(v);
                }
                user.update_time = Utc::now();
                Ok(Some(user.clone()))
            } else {
                Ok(None)
            }
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let users = self.users.clone();
        let id = *id;
        Box::pin(async move { Ok(users.lock().unwrap().remove(&id).is_some()) })
    }

    fn find_all_with_page(
        &self,
        _query: &UserPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<UserVO>, i64)>> {
        Box::pin(async move { Ok((vec![], 0)) })
    }
}

// ==================== Fake Token Store ====================

struct FakeTokenStore {
    store: Arc<Mutex<HashMap<String, String>>>,
}

impl FakeTokenStore {
    fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl TokenStore for FakeTokenStore {
    fn set_token(
        &self,
        user_id: &str,
        token: &str,
        _ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        let token = token.to_string();
        Box::pin(async move {
            store.lock().unwrap().insert(key, token);
            Ok(())
        })
    }

    fn get_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        Box::pin(async move { Ok(store.lock().unwrap().get(&key).cloned()) })
    }

    fn delete_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        Box::pin(async move {
            store.lock().unwrap().remove(&key);
            Ok(())
        })
    }
}

struct FakeRoleRepository;

impl RoleRepository for FakeRoleRepository {
    fn create(&self, _req: &CreateRoleRequest, _id: &i64) -> DynFuture<SeaOrmResult<Role>> {
        Box::pin(async move { unimplemented!() })
    }
    fn find_by_id(&self, _id: &i64) -> DynFuture<SeaOrmOptResult<Role>> {
        Box::pin(async move { Ok(None) })
    }
    fn find_by_code(&self, _code: &str) -> DynFuture<SeaOrmOptResult<Role>> {
        Box::pin(async move { Ok(None) })
    }
    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Role>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
    fn find_all_with_page(
        &self,
        _query: &RolePageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<Role>, i64)>> {
        Box::pin(async move { Ok((Vec::new(), 0)) })
    }
    fn update(&self, _id: &i64, _req: &UpdateRoleRequest) -> DynFuture<SeaOrmOptResult<Role>> {
        Box::pin(async move { Ok(None) })
    }
    fn delete(&self, _id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        Box::pin(async move { Ok(false) })
    }
    fn assign_role_to_user(&self, _user_id: &i64, _role_id: &i64) -> DynFuture<SeaOrmResult<()>> {
        Box::pin(async move { Ok(()) })
    }
    fn remove_role_from_user(
        &self,
        _user_id: &i64,
        _role_id: &i64,
    ) -> DynFuture<SeaOrmResult<bool>> {
        Box::pin(async move { Ok(false) })
    }
    fn find_roles_by_user_id(&self, _user_id: &i64) -> DynFuture<SeaOrmResult<Vec<Role>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
    fn find_users_by_role_id(&self, _role_id: &i64) -> DynFuture<SeaOrmResult<Vec<User>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
    fn set_menus(&self, _role_id: &i64, _menu_ids: &[i64]) -> DynFuture<SeaOrmResult<()>> {
        Box::pin(async move { Ok(()) })
    }
}

// ==================== Helper ====================

fn create_auth_service(
    user_repo: Arc<dyn UserRepository>,
    token_store: Arc<dyn TokenStore>,
) -> AuthService {
    let role_repo = Arc::new(FakeRoleRepository);
    AuthService::new(user_repo, token_store, role_repo, "test-secret")
}

// ==================== Auth Service Tests ====================

#[tokio::test]
async fn test_login_success() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store.clone());

    let password_hash = md5_encrypt("password123");
    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: Some("Test User".to_string()),
        password: Some(password_hash),
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1i64).await.unwrap();

    let result = service.login("testuser", "password123").await.unwrap();
    assert!(!result.access_token.is_empty());
    assert!(!result.refresh_token.is_empty());
    assert_eq!(result.token_type, "Bearer");
    assert_eq!(result.user.id, 1);
    assert_eq!(result.user.username, "testuser");

    let stored = token_store.get_token("1").await.unwrap();
    assert_eq!(stored, Some(result.access_token));
}

#[tokio::test]
async fn test_login_wrong_username() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo, token_store);

    let result = service.login("nonexistent", "password123").await;
    assert!(matches!(result, Err(AppError::Unauthorized(_))));
}

#[tokio::test]
async fn test_login_wrong_password() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store);

    let password_hash = md5_encrypt("password123");
    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: None,
        password: Some(password_hash),
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1i64).await.unwrap();

    let result = service.login("testuser", "wrongpassword").await;
    assert!(matches!(result, Err(AppError::Unauthorized(_))));
}

#[tokio::test]
async fn test_login_no_password_hash() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store);

    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: None,
        password: None,
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1).await.unwrap();

    let result = service.login("testuser", "password123").await;
    assert!(matches!(result, Err(AppError::Unauthorized(_))));
}

#[tokio::test]
async fn test_logout_success() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store.clone());

    let password_hash = md5_encrypt("password123");
    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: None,
        password: Some(password_hash),
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1i64).await.unwrap();

    let login_result = service.login("testuser", "password123").await.unwrap();
    assert!(!login_result.access_token.is_empty());

    let stored = token_store.get_token("1").await.unwrap();
    assert!(stored.is_some());

    service.logout(1).await.unwrap();

    let stored = token_store.get_token("1").await.unwrap();
    assert!(stored.is_none());
}

#[tokio::test]
async fn test_validate_token_success() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store.clone());

    let password_hash = md5_encrypt("password123");
    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: None,
        password: Some(password_hash),
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1i64).await.unwrap();

    let login_result = service.login("testuser", "password123").await.unwrap();
    let token = login_result.access_token;

    let user_id = service.validate_token(&token).await.unwrap();
    assert_eq!(user_id, 1);
}

#[tokio::test]
async fn test_validate_token_wrong_token() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store.clone());

    let password_hash = md5_encrypt("password123");
    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: None,
        password: Some(password_hash),
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1i64).await.unwrap();

    let login_result = service.login("testuser", "password123").await.unwrap();

    token_store
        .set_token("1", "some-other-token", 86400)
        .await
        .unwrap();

    let result = service.validate_token(&login_result.access_token).await;
    assert!(matches!(result, Err(AppError::Unauthorized(_))));
}

#[tokio::test]
async fn test_validate_token_no_stored_token() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store.clone());

    let password_hash = md5_encrypt("password123");
    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: None,
        password: Some(password_hash),
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1i64).await.unwrap();

    let login_result = service.login("testuser", "password123").await.unwrap();
    let token = login_result.access_token;

    token_store.delete_token("1").await.unwrap();

    let result = service.validate_token(&token).await;
    assert!(matches!(result, Err(AppError::Unauthorized(_))));
}

#[tokio::test]
async fn test_refresh_token_success() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store.clone());

    let password_hash = md5_encrypt("password123");
    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: None,
        password: Some(password_hash),
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1i64).await.unwrap();

    let login_result = service.login("testuser", "password123").await.unwrap();

    let refresh_result = service
        .refresh_token(&login_result.refresh_token)
        .await
        .unwrap();

    assert!(!refresh_result.access_token.is_empty());
    assert!(!refresh_result.refresh_token.is_empty());
    assert_eq!(refresh_result.token_type, "Bearer");
    assert_eq!(refresh_result.user.id, 1);

    let stored = token_store.get_token("1").await.unwrap();
    assert_eq!(stored, Some(refresh_result.access_token));
}

#[tokio::test]
async fn test_refresh_token_invalid() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo, token_store);

    let result = service.refresh_token("invalid-token").await;
    assert!(matches!(result, Err(AppError::Unauthorized(_))));
}

#[tokio::test]
async fn test_refresh_token_user_not_found() {
    let user_repo = Arc::new(FakeUserRepository::new());
    let token_store = Arc::new(FakeTokenStore::new());
    let service = create_auth_service(user_repo.clone(), token_store.clone());

    let password_hash = md5_encrypt("password123");
    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: None,
        password: Some(password_hash),
        org_id: 0,
        remarks: None,
        card: None,
        sex: None,
    };
    user_repo.create(&req, &1i64).await.unwrap();

    let login_result = service.login("testuser", "password123").await.unwrap();

    user_repo.delete(&1).await.unwrap();

    let result = service.refresh_token(&login_result.refresh_token).await;
    assert!(matches!(result, Err(AppError::Unauthorized(_))));
}
