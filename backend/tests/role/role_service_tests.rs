use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, RoleRepository, SeaOrmOptResult, SeaOrmResult};
use x_rust::system::sys_role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use x_rust::system::sys_role::service::RoleService;
use x_rust::system::sys_user::domain::User;

// ==================== Fake Role Repository ====================

struct FakeRoleRepository {
    roles: Arc<Mutex<HashMap<i64, Role>>>,
    user_roles: Arc<Mutex<HashMap<i64, Vec<i64>>>>,
    role_users: Arc<Mutex<HashMap<i64, Vec<i64>>>>,
}

impl FakeRoleRepository {
    fn new() -> Self {
        Self {
            roles: Arc::new(Mutex::new(HashMap::new())),
            user_roles: Arc::new(Mutex::new(HashMap::new())),
            role_users: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl RoleRepository for FakeRoleRepository {
    fn create(&self, req: &CreateRoleRequest, id: &i64) -> DynFuture<SeaOrmResult<Role>> {
        let roles = self.roles.clone();
        let id = *id;
        let name = req.name.clone();
        let code = req.code.clone();
        let description = req.description.clone();
        let remarks = req.remarks.clone();
        let ds_type = req.ds_type;
        let ds_scope = req.ds_scope.clone();
        Box::pin(async move {
            let role = Role {
                id: id,
                name: name.clone(),
                code,
                description: description.clone(),
                create_time: Utc::now(),
                update_time: Utc::now(),
                is_deleted: 0,
                remarks,
                is_edit: Some(false),
                ds_type,
                ds_scope,
            };
            roles.lock().unwrap().insert(id, role.clone());
            Ok(role)
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Role>> {
        let roles = self.roles.clone();
        let id = *id;
        Box::pin(async move { Ok(roles.lock().unwrap().get(&id).cloned()) })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Role>>> {
        let roles = self.roles.clone();
        Box::pin(async move { Ok(roles.lock().unwrap().values().cloned().collect()) })
    }

    fn find_all_with_page(
        &self,
        query: &RolePageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<Role>, i64)>> {
        let roles = self.roles.clone();
        let query = query.clone();
        Box::pin(async move {
            let mut vec: Vec<Role> = roles
                .lock()
                .unwrap()
                .values()
                .filter(|r| r.is_deleted == 0)
                .cloned()
                .collect();

            if let Some(ref v) = query.name {
                let pattern = v.to_lowercase();
                vec.retain(|r| r.name.to_lowercase().contains(&pattern));
            }
            if let Some(ref v) = query.code {
                vec.retain(|r| r.code.as_ref().map_or(false, |c| c == v));
            }

            vec.sort_by(|a, b| b.id.cmp(&a.id));
            let total = vec.len() as i64;
            let offset = (query.page() - 1) * query.size();
            let records: Vec<Role> = vec
                .iter()
                .skip(offset as usize)
                .take(query.size() as usize)
                .cloned()
                .collect();
            Ok((records, total))
        })
    }

    fn update(&self, id: &i64, req: &UpdateRoleRequest) -> DynFuture<SeaOrmOptResult<Role>> {
        let roles = self.roles.clone();
        let id = *id;
        let name = req.name.clone();
        let code = req.code.clone();
        let description = req.description.clone();
        let remarks = req.remarks.clone();
        let is_edit = req.is_edit;
        let ds_type = req.ds_type;
        let ds_scope = req.ds_scope.clone();
        Box::pin(async move {
            let mut roles_lock = roles.lock().unwrap();
            if let Some(role) = roles_lock.get_mut(&id) {
                if let Some(n) = name {
                    role.name = n;
                }
                if let Some(c) = code {
                    role.code = Some(c);
                }
                if let Some(d) = description {
                    role.description = Some(d);
                }
                if let Some(r) = remarks {
                    role.remarks = Some(r);
                }
                if let Some(e) = is_edit {
                    role.is_edit = Some(e);
                }
                if let Some(t) = ds_type {
                    role.ds_type = Some(t);
                }
                if let Some(s) = ds_scope {
                    role.ds_scope = Some(s);
                }
                role.update_time = Utc::now();
                Ok(Some(role.clone()))
            } else {
                Ok(None)
            }
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let roles = self.roles.clone();
        let id = *id;
        Box::pin(async move { Ok(roles.lock().unwrap().remove(&id).is_some()) })
    }

    fn assign_role_to_user(&self, user_id: &i64, role_id: &i64) -> DynFuture<SeaOrmResult<()>> {
        let user_roles = self.user_roles.clone();
        let role_users = self.role_users.clone();
        let user_id = *user_id;
        let role_id = *role_id;
        Box::pin(async move {
            user_roles
                .lock()
                .unwrap()
                .entry(user_id)
                .or_default()
                .push(role_id);
            role_users
                .lock()
                .unwrap()
                .entry(role_id)
                .or_default()
                .push(user_id);
            Ok(())
        })
    }

    fn remove_role_from_user(&self, user_id: &i64, role_id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let user_roles = self.user_roles.clone();
        let role_users = self.role_users.clone();
        let user_id = *user_id;
        let role_id = *role_id;
        Box::pin(async move {
            let removed_from_user = user_roles
                .lock()
                .unwrap()
                .get_mut(&user_id)
                .map(|roles| {
                    let len_before = roles.len();
                    roles.retain(|r| r != &role_id);
                    roles.len() < len_before
                })
                .unwrap_or(false);

            if removed_from_user {
                if let Some(users) = role_users.lock().unwrap().get_mut(&role_id) {
                    users.retain(|u| u != &user_id);
                }
            }
            Ok(removed_from_user)
        })
    }

    fn find_roles_by_user_id(&self, user_id: &i64) -> DynFuture<SeaOrmResult<Vec<Role>>> {
        let roles = self.roles.clone();
        let user_roles = self.user_roles.clone();
        let user_id = *user_id;
        Box::pin(async move {
            let roles_lock = roles.lock().unwrap();
            let user_roles_lock = user_roles.lock().unwrap();
            let result: Vec<Role> = user_roles_lock
                .get(&user_id)
                .map(|role_ids| {
                    role_ids
                        .iter()
                        .filter_map(|rid| roles_lock.get(rid).cloned())
                        .collect()
                })
                .unwrap_or_default();
            Ok(result)
        })
    }

    fn find_users_by_role_id(&self, role_id: &i64) -> DynFuture<SeaOrmResult<Vec<User>>> {
        let role_users = self.role_users.clone();
        let role_id = *role_id;
        Box::pin(async move {
            let users = role_users
                .lock()
                .unwrap()
                .get(&role_id)
                .cloned()
                .unwrap_or_default();
            Ok(users
                .into_iter()
                .enumerate()
                .map(|(_i, uid)| User {
                    id: uid,
                    username: format!("User {}", uid),
                    phone: None,
                    email: Some(format!("{}@example.com", uid)),
                    real_name: None,
                    password: None,
                    org_id: 1,
                    lock_time: None,
                    last_login_time: None,
                    try_count: Some(0),
                    lock_flag: Some(1),
                    create_time: Utc::now(),
                    update_time: Utc::now(),
                    is_deleted: 0,
                    remarks: None,
                    pass_update_time: None,
                    card: None,
                    is_show: Some(1),
                    enable: Some(1),
                    first_login: Some(1),
                    sex: None,
                })
                .collect())
        })
    }

    fn find_by_code(&self, code: &str) -> DynFuture<SeaOrmOptResult<Role>> {
        let roles = self.roles.clone();
        let code = code.to_string();
        Box::pin(async move {
            Ok(roles
                .lock()
                .unwrap()
                .values()
                .find(|r| r.code.as_ref() == Some(&code) && r.is_deleted == 0)
                .cloned())
        })
    }

    fn set_menus(&self, _role_id: &i64, _menu_ids: &[i64]) -> DynFuture<SeaOrmResult<()>> {
        Box::pin(async move { Ok(()) })
    }
}

// ==================== Role Service Tests ====================

#[tokio::test]
async fn test_create_role_success() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo);

    let req = CreateRoleRequest {
        name: "Admin".to_string(),
        code: Some("ADMIN".to_string()),
        description: Some("Administrator".to_string()),
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };

    let result = service.create_role(req).await.unwrap();
    assert_eq!(result.name, "Admin");
    assert_eq!(result.description, Some("Administrator".to_string()));
}

#[tokio::test]
async fn test_get_role_success() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo.clone());

    let role_id = 1i64;
    repo.create(
        &CreateRoleRequest {
            name: "Editor".to_string(),
            code: Some("EDITOR".to_string()),
            description: Some("Can edit".to_string()),
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &role_id,
    )
    .await
    .unwrap();

    let result = service.get_role(&role_id).await.unwrap();
    assert_eq!(result.id, role_id);
    assert_eq!(result.name, "Editor");
}

#[tokio::test]
async fn test_get_role_not_found() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo);

    let result = service.get_role(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_all_roles() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo.clone());

    repo.create(
        &CreateRoleRequest {
            name: "Admin".to_string(),
            code: None,
            description: None.map(|s: &str| s.to_string()),
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateRoleRequest {
            name: "Editor".to_string(),
            code: None,
            description: None.map(|s: &str| s.to_string()),
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &2i64,
    )
    .await
    .unwrap();

    let result = service.get_all_roles().await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_update_role_success() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo.clone());

    let role_id = 1i64;
    repo.create(
        &CreateRoleRequest {
            name: "Old Name".to_string(),
            code: None,
            description: None,
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &role_id,
    )
    .await
    .unwrap();

    let req = UpdateRoleRequest {
        name: Some("New Name".to_string()),
        code: None,
        description: Some("Updated description".to_string()),
        remarks: None,
        is_edit: None,
        ds_type: None,
        ds_scope: None,
    };
    let result = service.update_role(&role_id, req).await.unwrap();
    assert_eq!(result.name, "New Name");
    assert_eq!(result.description, Some("Updated description".to_string()));
}

#[tokio::test]
async fn test_update_role_not_found() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo);

    let req = UpdateRoleRequest {
        name: Some("Updated".to_string()),
        code: None,
        description: None,
        remarks: None,
        is_edit: None,
        ds_type: None,
        ds_scope: None,
    };
    let result = service.update_role(&999i64, req).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_role_success() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo.clone());

    let role_id = 1i64;
    repo.create(
        &CreateRoleRequest {
            name: "Temp".to_string(),
            code: None,
            description: None,
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &role_id,
    )
    .await
    .unwrap();

    let result = service.delete_role(&role_id).await;
    assert!(result.is_ok());

    let find_result = service.get_role(&role_id).await;
    assert!(matches!(find_result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_role_not_found() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo);

    let result = service.delete_role(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_assign_role_success() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo.clone());

    repo.create(
        &CreateRoleRequest {
            name: "Editor".to_string(),
            code: None,
            description: None.map(|s: &str| s.to_string()),
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &1i64,
    )
    .await
    .unwrap();

    let result = service.assign_role(&1i64, &1i64).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_assign_role_not_found() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo);

    let result = service.assign_role(&1i64, &999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_remove_role_success() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo.clone());

    repo.create(
        &CreateRoleRequest {
            name: "Editor".to_string(),
            code: None,
            description: None.map(|s: &str| s.to_string()),
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    service.assign_role(&1i64, &1i64).await.unwrap();

    let result = service.remove_role(&1i64, &1i64).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_remove_role_not_found() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo);

    let result = service.remove_role(&1i64, &1i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_roles_for_user() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo.clone());

    repo.create(
        &CreateRoleRequest {
            name: "Admin".to_string(),
            code: None,
            description: None.map(|s: &str| s.to_string()),
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateRoleRequest {
            name: "Editor".to_string(),
            code: None,
            description: None.map(|s: &str| s.to_string()),
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &2i64,
    )
    .await
    .unwrap();
    service.assign_role(&1i64, &1i64).await.unwrap();
    service.assign_role(&1i64, &2i64).await.unwrap();

    let result = service.get_roles_for_user(&1i64).await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_get_users_for_role() {
    let repo = Arc::new(FakeRoleRepository::new());
    let service = RoleService::new(repo.clone());

    repo.create(
        &CreateRoleRequest {
            name: "Admin".to_string(),
            code: None,
            description: None.map(|s: &str| s.to_string()),
            remarks: None,
            ds_type: None,
            ds_scope: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    service.assign_role(&1i64, &1i64).await.unwrap();
    service.assign_role(&2i64, &1i64).await.unwrap();

    let result = service.get_users_for_role(&1i64).await.unwrap();
    assert_eq!(result.len(), 2);
}
