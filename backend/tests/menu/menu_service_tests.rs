use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::common::error::AppError;
use x_rust::common::traits::{
    DynFuture, MenuRepository, RoleRepository, SeaOrmOptResult, SeaOrmResult,
};
use x_rust::menu::domain::{CreateMenuRequest, Menu, UpdateMenuRequest};
use x_rust::menu::service::MenuService;
use x_rust::role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use x_rust::user::domain::User;

struct FakeMenuRepository {
    menus: Arc<Mutex<HashMap<String, Menu>>>,
}

struct FakeRoleRepository {
    roles: Arc<Mutex<HashMap<String, Role>>>,
    user_roles: Arc<Mutex<HashMap<String, Vec<String>>>>,
}

impl FakeMenuRepository {
    fn new() -> Self {
        Self {
            menus: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl FakeRoleRepository {
    fn new() -> Self {
        Self {
            roles: Arc::new(Mutex::new(HashMap::new())),
            user_roles: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl MenuRepository for FakeMenuRepository {
    fn create(&self, req: &CreateMenuRequest, id: &str) -> DynFuture<SeaOrmResult<Menu>> {
        let menus = self.menus.clone();
        let id = id.to_string();
        let name = req.name.clone();
        let code = req.code.clone();
        let permission = req.permission.clone();
        let path_url = req.path_url.clone();
        let icon = req.icon.clone();
        let parent_id = req.parent_id.clone();
        let component = req.component.clone();
        let sort = req.sort;
        let keep_alive = req.keep_alive;
        let r#type = req.r#type;
        let remarks = req.remarks.clone();
        let leaf = req.leaf;
        let role_code = req.role_code.clone();
        let disabled = req.disabled;
        let find_auth_id = req.find_auth_id;
        Box::pin(async move {
            let menu = Menu {
                id: id.clone(),
                name,
                code,
                permission,
                path_url,
                icon,
                parent_id,
                component,
                sort,
                keep_alive,
                r#type,
                is_deleted: 0,
                remarks,
                leaf,
                role_code,
                disabled,
                find_auth_id,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            };
            menus.lock().unwrap().insert(id, menu.clone());
            Ok(menu)
        })
    }

    fn find_by_id(&self, id: &str) -> DynFuture<SeaOrmOptResult<Menu>> {
        let menus = self.menus.clone();
        let id = id.to_string();
        Box::pin(async move { Ok(menus.lock().unwrap().get(&id).cloned()) })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        let menus = self.menus.clone();
        Box::pin(async move {
            Ok(menus
                .lock()
                .unwrap()
                .values()
                .filter(|m| m.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn find_by_parent_id(&self, parent_id: Option<&str>) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        let menus = self.menus.clone();
        let parent_id = parent_id.map(String::from);
        Box::pin(async move {
            Ok(menus
                .lock()
                .unwrap()
                .values()
                .filter(|m| {
                    m.is_deleted == 0
                        && match &parent_id {
                            Some(pid) => m.parent_id.as_ref() == Some(pid),
                            None => m.parent_id.is_none(),
                        }
                })
                .cloned()
                .collect())
        })
    }

    fn find_tree(&self) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        let menus = self.menus.clone();
        Box::pin(async move {
            Ok(menus
                .lock()
                .unwrap()
                .values()
                .filter(|m| m.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn find_menus_by_role_id(&self, role_id: &str) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        let menus = self.menus.clone();
        let role_id = role_id.to_string();
        Box::pin(async move {
            Ok(menus
                .lock()
                .unwrap()
                .values()
                .filter(|m| m.is_deleted == 0 && m.role_code.as_ref() == Some(&role_id))
                .cloned()
                .collect())
        })
    }

    fn update(&self, id: &str, req: &UpdateMenuRequest) -> DynFuture<SeaOrmOptResult<Menu>> {
        let menus = self.menus.clone();
        let id = id.to_string();
        let name = req.name.clone();
        let code = req.code.clone();
        let permission = req.permission.clone();
        let path_url = req.path_url.clone();
        let icon = req.icon.clone();
        let parent_id = req.parent_id.clone();
        let component = req.component.clone();
        let sort = req.sort;
        let keep_alive = req.keep_alive;
        let r#type = req.r#type;
        let remarks = req.remarks.clone();
        let leaf = req.leaf;
        let role_code = req.role_code.clone();
        let disabled = req.disabled;
        let find_auth_id = req.find_auth_id;
        Box::pin(async move {
            let mut menus_lock = menus.lock().unwrap();
            if let Some(menu) = menus_lock.get_mut(&id) {
                if let Some(v) = name {
                    menu.name = v;
                }
                if let Some(v) = code {
                    menu.code = Some(v);
                }
                if let Some(v) = permission {
                    menu.permission = Some(v);
                }
                if let Some(v) = path_url {
                    menu.path_url = Some(v);
                }
                if let Some(v) = icon {
                    menu.icon = Some(v);
                }
                if let Some(v) = parent_id {
                    menu.parent_id = Some(v);
                }
                if let Some(v) = component {
                    menu.component = Some(v);
                }
                if let Some(v) = sort {
                    menu.sort = Some(v);
                }
                if let Some(v) = keep_alive {
                    menu.keep_alive = Some(v);
                }
                if let Some(v) = r#type {
                    menu.r#type = Some(v);
                }
                if let Some(v) = remarks {
                    menu.remarks = Some(v);
                }
                if let Some(v) = leaf {
                    menu.leaf = Some(v);
                }
                if let Some(v) = role_code {
                    menu.role_code = Some(v);
                }
                if let Some(v) = disabled {
                    menu.disabled = Some(v);
                }
                if let Some(v) = find_auth_id {
                    menu.find_auth_id = Some(v);
                }
                menu.updated_at = Utc::now();
                Ok(Some(menu.clone()))
            } else {
                Ok(None)
            }
        })
    }

    fn delete(&self, id: &str) -> DynFuture<SeaOrmResult<bool>> {
        let menus = self.menus.clone();
        let id = id.to_string();
        Box::pin(async move {
            let mut menus_lock = menus.lock().unwrap();
            if let Some(menu) = menus_lock.get_mut(&id) {
                menu.is_deleted = 1;
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }
}

impl RoleRepository for FakeRoleRepository {
    fn create(&self, req: &CreateRoleRequest, id: &str) -> DynFuture<SeaOrmResult<Role>> {
        let roles = self.roles.clone();
        let id = id.to_string();
        let name = req.name.clone();
        let code = req.code.clone();
        let description = req.description.clone();
        let remarks = req.remarks.clone();
        let ds_type = req.ds_type;
        let ds_scope = req.ds_scope.clone();
        Box::pin(async move {
            let role = Role {
                id: id.clone(),
                name,
                code,
                description,
                create_time: Some(Utc::now()),
                update_time: Some(Utc::now()),
                is_deleted: 0,
                remarks,
                is_edit: Some(true),
                ds_type,
                ds_scope,
            };
            roles.lock().unwrap().insert(id, role.clone());
            Ok(role)
        })
    }

    fn find_by_id(&self, id: &str) -> DynFuture<SeaOrmOptResult<Role>> {
        let roles = self.roles.clone();
        let id = id.to_string();
        Box::pin(async move { Ok(roles.lock().unwrap().get(&id).cloned()) })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Role>>> {
        let roles = self.roles.clone();
        Box::pin(async move {
            Ok(roles
                .lock()
                .unwrap()
                .values()
                .filter(|r| r.is_deleted == 0)
                .cloned()
                .collect())
        })
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
                vec.retain(|r| r.name.contains(v));
            }
            if let Some(ref v) = query.code {
                vec.retain(|r| r.code.as_ref() == Some(v));
            }
            let total = vec.len() as i64;
            let offset = (query.page() - 1) * query.size();
            let records: Vec<Role> = vec
                .into_iter()
                .skip(offset as usize)
                .take(query.size() as usize)
                .collect();
            Ok((records, total))
        })
    }

    fn update(&self, id: &str, req: &UpdateRoleRequest) -> DynFuture<SeaOrmOptResult<Role>> {
        let roles = self.roles.clone();
        let id = id.to_string();
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
                if let Some(v) = name {
                    role.name = v;
                }
                if let Some(v) = code {
                    role.code = Some(v);
                }
                if let Some(v) = description {
                    role.description = Some(v);
                }
                if let Some(v) = remarks {
                    role.remarks = Some(v);
                }
                if let Some(v) = is_edit {
                    role.is_edit = Some(v);
                }
                if let Some(v) = ds_type {
                    role.ds_type = Some(v);
                }
                if let Some(v) = ds_scope {
                    role.ds_scope = Some(v);
                }
                role.update_time = Some(Utc::now());
                Ok(Some(role.clone()))
            } else {
                Ok(None)
            }
        })
    }

    fn delete(&self, id: &str) -> DynFuture<SeaOrmResult<bool>> {
        let roles = self.roles.clone();
        let id = id.to_string();
        Box::pin(async move {
            let mut roles_lock = roles.lock().unwrap();
            if let Some(role) = roles_lock.get_mut(&id) {
                role.is_deleted = 1;
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }

    fn assign_role_to_user(&self, user_id: &str, role_id: &str) -> DynFuture<SeaOrmResult<()>> {
        let user_roles = self.user_roles.clone();
        let user_id = user_id.to_string();
        let role_id = role_id.to_string();
        Box::pin(async move {
            user_roles
                .lock()
                .unwrap()
                .entry(user_id)
                .or_default()
                .push(role_id);
            Ok(())
        })
    }

    fn remove_role_from_user(&self, user_id: &str, role_id: &str) -> DynFuture<SeaOrmResult<bool>> {
        let user_roles = self.user_roles.clone();
        let user_id = user_id.to_string();
        let role_id = role_id.to_string();
        Box::pin(async move {
            let mut map = user_roles.lock().unwrap();
            if let Some(roles) = map.get_mut(&user_id) {
                let len_before = roles.len();
                roles.retain(|r| r != &role_id);
                Ok(roles.len() < len_before)
            } else {
                Ok(false)
            }
        })
    }

    fn find_roles_by_user_id(&self, user_id: &str) -> DynFuture<SeaOrmResult<Vec<Role>>> {
        let user_roles = self.user_roles.clone();
        let roles = self.roles.clone();
        let user_id = user_id.to_string();
        Box::pin(async move {
            let map = user_roles.lock().unwrap();
            let role_ids = map.get(&user_id).cloned().unwrap_or_default();
            let roles_lock = roles.lock().unwrap();
            Ok(role_ids
                .iter()
                .filter_map(|rid| roles_lock.get(rid))
                .filter(|r| r.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn find_users_by_role_id(&self, _role_id: &str) -> DynFuture<SeaOrmResult<Vec<User>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

#[tokio::test]
async fn test_create_menu_success() {
    let menu_repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(menu_repo, role_repo);
    let req = CreateMenuRequest {
        name: "Dashboard".to_string(),
        code: Some("dashboard".to_string()),
        permission: Some("menu:dashboard".to_string()),
        path_url: Some("/dashboard".to_string()),
        icon: Some("dashboard-icon".to_string()),
        parent_id: None,
        component: Some("DashboardView".to_string()),
        sort: Some(1),
        keep_alive: Some(0),
        r#type: Some(1),
        remarks: Some("Main dashboard".to_string()),
        leaf: Some(true),
        role_code: Some("admin".to_string()),
        disabled: Some(false),
        find_auth_id: Some(1),
    };
    let result = service.create_menu(req).await.unwrap();
    assert_eq!(result.name, "Dashboard");
    assert_eq!(result.code, Some("dashboard".to_string()));
    assert_eq!(result.is_deleted, 0);
}

#[tokio::test]
async fn test_get_menu_success() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo.clone(), role_repo);
    let menu_id = "menu-1".to_string();
    let req = CreateMenuRequest {
        name: "Settings".to_string(),
        code: None,
        permission: None,
        path_url: None,
        icon: None,
        parent_id: None,
        component: None,
        sort: None,
        keep_alive: None,
        r#type: None,
        remarks: None,
        leaf: None,
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    repo.create(&req, &menu_id).await.unwrap();
    let result = service.get_menu(&menu_id).await.unwrap();
    assert_eq!(result.id, menu_id);
    assert_eq!(result.name, "Settings");
}

#[tokio::test]
async fn test_get_menu_not_found() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo, role_repo);
    let result = service.get_menu("nonexistent-id").await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_all_menus() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo.clone(), role_repo);
    repo.create(
        &CreateMenuRequest {
            name: "Menu 1".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: None,
            component: None,
            sort: None,
            keep_alive: None,
            r#type: None,
            remarks: None,
            leaf: None,
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        "m1",
    )
    .await
    .unwrap();
    repo.create(
        &CreateMenuRequest {
            name: "Menu 2".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: None,
            component: None,
            sort: None,
            keep_alive: None,
            r#type: None,
            remarks: None,
            leaf: None,
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        "m2",
    )
    .await
    .unwrap();
    let result = service.get_all_menus().await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_get_menu_tree() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo.clone(), role_repo);
    repo.create(
        &CreateMenuRequest {
            name: "Root".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: None,
            component: None,
            sort: Some(1),
            keep_alive: None,
            r#type: Some(1),
            remarks: None,
            leaf: Some(false),
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        "root",
    )
    .await
    .unwrap();
    repo.create(
        &CreateMenuRequest {
            name: "Child".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: Some("root".to_string()),
            component: None,
            sort: Some(2),
            keep_alive: None,
            r#type: Some(1),
            remarks: None,
            leaf: Some(true),
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        "child",
    )
    .await
    .unwrap();
    let result = service.get_menu_tree().await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_get_menus_by_parent() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo.clone(), role_repo);
    repo.create(
        &CreateMenuRequest {
            name: "Parent".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: None,
            component: None,
            sort: None,
            keep_alive: None,
            r#type: None,
            remarks: None,
            leaf: None,
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        "parent",
    )
    .await
    .unwrap();
    repo.create(
        &CreateMenuRequest {
            name: "Child 1".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: Some("parent".to_string()),
            component: None,
            sort: None,
            keep_alive: None,
            r#type: None,
            remarks: None,
            leaf: None,
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        "child1",
    )
    .await
    .unwrap();
    repo.create(
        &CreateMenuRequest {
            name: "Child 2".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: Some("parent".to_string()),
            component: None,
            sort: None,
            keep_alive: None,
            r#type: None,
            remarks: None,
            leaf: None,
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        "child2",
    )
    .await
    .unwrap();
    let result = service.get_menus_by_parent(Some("parent")).await.unwrap();
    assert_eq!(result.len(), 2);
    let root_menus = service.get_menus_by_parent(None).await.unwrap();
    assert_eq!(root_menus.len(), 1);
}

#[tokio::test]
async fn test_update_menu_success() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo.clone(), role_repo);
    let menu_id = "menu-1".to_string();
    repo.create(
        &CreateMenuRequest {
            name: "Old Name".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: None,
            component: None,
            sort: None,
            keep_alive: None,
            r#type: None,
            remarks: None,
            leaf: None,
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        &menu_id,
    )
    .await
    .unwrap();
    let req = UpdateMenuRequest {
        name: Some("New Name".to_string()),
        code: Some("new-code".to_string()),
        permission: None,
        path_url: None,
        icon: None,
        parent_id: None,
        component: None,
        sort: Some(10),
        keep_alive: None,
        r#type: None,
        remarks: None,
        leaf: None,
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    let result = service.update_menu(&menu_id, req).await.unwrap();
    assert_eq!(result.name, "New Name");
    assert_eq!(result.code, Some("new-code".to_string()));
    assert_eq!(result.sort, Some(10));
}

#[tokio::test]
async fn test_update_menu_not_found() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo, role_repo);
    let req = UpdateMenuRequest {
        name: Some("Updated".to_string()),
        code: None,
        permission: None,
        path_url: None,
        icon: None,
        parent_id: None,
        component: None,
        sort: None,
        keep_alive: None,
        r#type: None,
        remarks: None,
        leaf: None,
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    let result = service.update_menu("nonexistent-id", req).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_menu_success() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo.clone(), role_repo);
    let menu_id = "menu-1".to_string();
    repo.create(
        &CreateMenuRequest {
            name: "Temp Menu".to_string(),
            code: None,
            permission: None,
            path_url: None,
            icon: None,
            parent_id: None,
            component: None,
            sort: None,
            keep_alive: None,
            r#type: None,
            remarks: None,
            leaf: None,
            role_code: None,
            disabled: None,
            find_auth_id: None,
        },
        &menu_id,
    )
    .await
    .unwrap();
    let result = service.delete_menu(&menu_id).await;
    assert!(result.is_ok());
    let all = service.get_all_menus().await.unwrap();
    assert_eq!(all.len(), 0);
}

#[tokio::test]
async fn test_delete_menu_not_found() {
    let repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = MenuService::new(repo, role_repo);
    let result = service.delete_menu("nonexistent-id").await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}
