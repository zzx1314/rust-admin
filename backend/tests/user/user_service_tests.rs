use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::common::error::AppError;
use x_rust::common::traits::{
    DynFuture, RoleRepository, SeaOrmOptResult, SeaOrmResult, UserRepository,
};
use x_rust::common::util::encrypt_password;
use x_rust::system::sys_role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use x_rust::system::sys_user::domain::{CreateUserRequest, UpdateUserRequest, User, UserPageQuery, UserVO};
use x_rust::system::sys_user::service::UserService;

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
}

impl UserRepository for FakeUserRepository {
    fn create(&self, req: &CreateUserRequest, id: &i64) -> DynFuture<SeaOrmResult<User>> {
        let users = self.users.clone();
        let req = req.clone();
        let id = *id;
        Box::pin(async move {
            let user = User {
                id: id,
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

    fn update(&self, id: &i64, req: &UpdateUserRequest) -> DynFuture<SeaOrmOptResult<User>> {
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
        query: &UserPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<UserVO>, i64)>> {
        let users = self.users.clone();
        let query = query.clone();
        Box::pin(async move {
            let mut vec: Vec<UserVO> = users
                .lock()
                .unwrap()
                .values()
                .filter(|u| u.is_deleted == 0)
                .filter(|u| {
                    if let Some(ref v) = query.username {
                        if !u.username.contains(v.as_str()) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.real_name {
                        if !u
                            .real_name
                            .as_ref()
                            .map_or(false, |n| n.contains(v.as_str()))
                        {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.phone {
                        if !u.phone.as_ref().map_or(false, |p| p.contains(v.as_str())) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.email {
                        if !u.email.as_ref().map_or(false, |e| e.contains(v.as_str())) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.org_id {
                        let query_org_id: i64 = v.parse().unwrap_or(0);
                        if u.org_id != query_org_id {
                            return false;
                        }
                    }
                    if let Some(v) = query.enable {
                        if u.enable != Some(v) {
                            return false;
                        }
                    }
                    true
                })
                .map(|u| UserVO {
                    id: u.id.clone(),
                    username: u.username.clone(),
                    phone: u.phone.clone(),
                    email: u.email.clone(),
                    real_name: u.real_name.clone(),
                    org_id: u.org_id,
                    org_name: None,
                    lock_time: u.lock_time,
                    last_login_time: u.last_login_time,
                    try_count: u.try_count,
                    lock_flag: u.lock_flag,
                    create_time: u.create_time.format("%Y-%m-%d %H:%M:%S").to_string(),
                    update_time: u.update_time,
                    remarks: u.remarks.clone(),
                    pass_update_time: u.pass_update_time,
                    card: u.card.clone(),
                    is_show: u.is_show,
                    enable: u.enable,
                    first_login: u.first_login,
                    sex: u.sex.clone(),
                    role_names: None,
                })
                .collect();
            vec.sort_by(|a, b| b.id.cmp(&a.id));
            let total = vec.len() as i64;
            let offset = (query.page() - 1) * query.size();
            let records: Vec<UserVO> = vec
                .iter()
                .skip(offset as usize)
                .take(query.size() as usize)
                .cloned()
                .collect();
            Ok((records, total))
        })
    }
}

// ==================== Fake Role Repository ====================

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

// ==================== User Service Tests ====================

#[tokio::test]
async fn test_create_user_success() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo, role_repo);

    let req = CreateUserRequest {
        username: "johndoe".to_string(),
        phone: None,
        email: Some("john@example.com".to_string()),
        real_name: Some("John Doe".to_string()),
        password: Some(encrypt_password("password123")),
        org_id: 1,
        remarks: None,
        card: None,
        sex: None,
    };

    let result = service.create_user(req).await;
    assert!(result.is_ok());
    let user = result.unwrap();
    assert_eq!(user.username, "johndoe");
    assert_eq!(user.real_name, Some("John Doe".to_string()));
}

#[tokio::test]
async fn test_get_user_success() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo.clone(), role_repo.clone());

    let req = CreateUserRequest {
        username: "testuser".to_string(),
        phone: None,
        email: Some("test@example.com".to_string()),
        real_name: Some("Test User".to_string()),
        password: None,
        org_id: 1,
        remarks: None,
        card: None,
        sex: None,
    };
    let _created = repo.create(&req, &1i64).await.unwrap();

    let result = service.get_user(&1i64).await.unwrap();
    assert_eq!(result.username, "testuser");
}

#[tokio::test]
async fn test_get_user_not_found() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo, role_repo);

    let result = service.get_user(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_all_users() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo.clone(), role_repo.clone());

    let req1 = CreateUserRequest {
        username: "user1".to_string(),
        phone: None,
        email: Some("user1@example.com".to_string()),
        real_name: None,
        password: None,
        org_id: 1,
        remarks: None,
        card: None,
        sex: None,
    };
    repo.create(&req1, &1i64).await.unwrap();

    let req2 = CreateUserRequest {
        username: "user2".to_string(),
        phone: None,
        email: Some("user2@example.com".to_string()),
        real_name: None,
        password: None,
        org_id: 1,
        remarks: None,
        card: None,
        sex: None,
    };
    repo.create(&req2, &2i64).await.unwrap();

    let result = service.get_all_users().await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_update_user_success() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo.clone(), role_repo.clone());

    let req = CreateUserRequest {
        username: "original".to_string(),
        phone: None,
        email: Some("original@example.com".to_string()),
        real_name: Some("Original Name".to_string()),
        password: None,
        org_id: 1,
        remarks: None,
        card: None,
        sex: None,
    };
    let _created = repo.create(&req, &1i64).await.unwrap();

    let update_req = UpdateUserRequest {
        username: Some("updated".to_string()),
        phone: None,
        email: None,
        real_name: Some("Updated Name".to_string()),
        password: None,
        org_id: 1,
        remarks: None,
        card: None,
        is_show: None,
        enable: None,
        sex: None,
    };
    let result = service.update_user(&1i64, update_req).await.unwrap();
    assert_eq!(result.username, "updated");
    assert_eq!(result.real_name, Some("Updated Name".to_string()));
}

#[tokio::test]
async fn test_update_user_not_found() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo, role_repo);

    let req = UpdateUserRequest {
        username: Some("updated".to_string()),
        phone: None,
        email: None,
        real_name: None,
        password: None,
        org_id: 1,
        remarks: None,
        card: None,
        is_show: None,
        enable: None,
        sex: None,
    };

    let result = service.update_user(&999i64, req).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_user_success() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo.clone(), role_repo.clone());

    let req = CreateUserRequest {
        username: "deleteme".to_string(),
        phone: None,
        email: Some("delete@example.com".to_string()),
        real_name: None,
        password: None,
        org_id: 1,
        remarks: None,
        card: None,
        sex: None,
    };
    let _created = repo.create(&req, &1i64).await.unwrap();

    let result = service.delete_user(&1i64).await;
    assert!(result.is_ok());

    let find_result = service.get_user(&1i64).await;
    assert!(matches!(find_result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_user_not_found() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo, role_repo);

    let result = service.delete_user(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_users_page_default() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo.clone(), role_repo.clone());

    for i in 1..=15 {
        let req = CreateUserRequest {
            username: format!("user{}", i),
            phone: None,
            email: Some(format!("user{}@example.com", i)),
            real_name: None,
            password: None,
            org_id: 1,
            remarks: None,
            card: None,
            sex: None,
        };
        repo.create(&req, &(i as i64)).await.unwrap();
    }

    let result = service
        .get_users_page(UserPageQuery {
            current: 1,
            size: 10,
            username: None,
            real_name: None,
            phone: None,
            email: None,
            org_id: None,
            enable: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 10);
    assert_eq!(result.total, 15);
    assert_eq!(result.current, 1);
    assert_eq!(result.size, 10);
}

#[tokio::test]
async fn test_get_users_page_custom() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo.clone(), role_repo.clone());

    for i in 1..=15 {
        let req = CreateUserRequest {
            username: format!("user{}", i),
            phone: None,
            email: Some(format!("user{}@example.com", i)),
            real_name: None,
            password: None,
            org_id: 1,
            remarks: None,
            card: None,
            sex: None,
        };
        repo.create(&req, &(i as i64)).await.unwrap();
    }

    let result = service
        .get_users_page(UserPageQuery {
            current: 2,
            size: 5,
            username: None,
            real_name: None,
            phone: None,
            email: None,
            org_id: None,
            enable: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 5);
    assert_eq!(result.total, 15);
    assert_eq!(result.current, 2);
    assert_eq!(result.size, 5);
}

#[tokio::test]
async fn test_get_users_page_out_of_range() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo.clone(), role_repo.clone());

    for i in 1..=3 {
        let req = CreateUserRequest {
            username: format!("user{}", i),
            phone: None,
            email: Some(format!("user{}@example.com", i)),
            real_name: None,
            password: None,
            org_id: 1,
            remarks: None,
            card: None,
            sex: None,
        };
        repo.create(&req, &(i as i64)).await.unwrap();
    }

    let result = service
        .get_users_page(UserPageQuery {
            current: 10,
            size: 5,
            username: None,
            real_name: None,
            phone: None,
            email: None,
            org_id: None,
            enable: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 0);
    assert_eq!(result.total, 3);
}

#[tokio::test]
async fn test_get_users_page_empty() {
    let repo = Arc::new(FakeUserRepository::new());
    let role_repo = Arc::new(FakeRoleRepository);
    let service = UserService::new(repo, role_repo);

    let result = service
        .get_users_page(UserPageQuery {
            current: 1,
            size: 10,
            username: None,
            real_name: None,
            phone: None,
            email: None,
            org_id: None,
            enable: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 0);
    assert_eq!(result.total, 0);
}
