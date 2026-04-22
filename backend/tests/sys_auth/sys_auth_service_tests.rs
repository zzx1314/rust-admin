use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::common::error::AppError;
use x_rust::common::traits::{
    DynFuture, MenuRepository, RoleRepository, SeaOrmOptResult, SeaOrmResult,
};
use x_rust::system::sys_auth::service::SysAuthService;
use x_rust::system::sys_menu::domain::{CreateMenuRequest, Menu, UpdateMenuRequest};
use x_rust::system::sys_role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use x_rust::system::sys_user::domain::User;

// ==================== Fake Menu Repository ====================

struct FakeMenuRepository {
    menus: Arc<Mutex<HashMap<i64, Menu>>>,
    role_menus: Arc<Mutex<HashMap<i64, Vec<i64>>>>,
}

impl FakeMenuRepository {
    fn new() -> Self {
        Self {
            menus: Arc::new(Mutex::new(HashMap::new())),
            role_menus: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn assign_menu_to_role(&self, role_id: i64, menu_id: i64) {
        self.role_menus
            .lock()
            .unwrap()
            .entry(role_id)
            .or_default()
            .push(menu_id);
    }
}

impl MenuRepository for FakeMenuRepository {
    fn create(&self, req: &CreateMenuRequest, id: &i64) -> DynFuture<SeaOrmResult<Menu>> {
        let menus = self.menus.clone();
        let id = *id;
        let name = req.name.clone();
        let code = req.code.clone();
        let permission = req.permission.clone();
        let path_url = req.path_url.clone();
        let icon = req.icon.clone();
        let parent_id = req.parent_id;
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
                id,
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
                create_time: Utc::now(),
                update_time: Utc::now(),
            };
            menus.lock().unwrap().insert(id, menu.clone());
            Ok(menu)
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Menu>> {
        let menus = self.menus.clone();
        let id = *id;
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

    fn find_by_parent_id(&self, _parent_id: Option<i64>) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn find_tree(&self) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn find_menus_by_role_id(&self, role_id: &i64) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        let menus = self.menus.clone();
        let role_menus = self.role_menus.clone();
        let role_id = *role_id;
        Box::pin(async move {
            let menu_ids = role_menus
                .lock()
                .unwrap()
                .get(&role_id)
                .cloned()
                .unwrap_or_default();
            let menus_lock = menus.lock().unwrap();
            Ok(menu_ids
                .iter()
                .filter_map(|id| menus_lock.get(id))
                .filter(|m| m.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn update(&self, _id: &i64, _req: &UpdateMenuRequest) -> DynFuture<SeaOrmOptResult<Menu>> {
        Box::pin(async move { Ok(None) })
    }

    fn delete(&self, _id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        Box::pin(async move { Ok(false) })
    }
}

// ==================== Fake Role Repository ====================

struct FakeRoleRepository {
    roles: Arc<Mutex<HashMap<i64, Role>>>,
}

impl FakeRoleRepository {
    fn new() -> Self {
        Self {
            roles: Arc::new(Mutex::new(HashMap::new())),
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
                id,
                name,
                code,
                description,
                create_time: Utc::now(),
                update_time: Utc::now(),
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

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Role>> {
        let roles = self.roles.clone();
        let id = *id;
        Box::pin(async move { Ok(roles.lock().unwrap().get(&id).cloned()) })
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

// ==================== Tests ====================

#[tokio::test]
async fn test_get_role_auth_returns_data() {
    let menu_repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = SysAuthService::new(menu_repo.clone(), role_repo.clone());

    // Create a role with code "admin"
    let role_id: i64 = 1001;
    role_repo
        .create(
            &CreateRoleRequest {
                name: "Administrator".to_string(),
                code: Some("admin".to_string()),
                description: None,
                remarks: None,
                ds_type: None,
                ds_scope: None,
            },
            &role_id,
        )
        .await
        .unwrap();

    // Create a parent menu (page, type=0)
    let parent_menu_id: i64 = 2001;
    menu_repo
        .create(
            &CreateMenuRequest {
                name: "User Management".to_string(),
                code: Some("userMgmt".to_string()),
                permission: None,
                path_url: Some("/system/user".to_string()),
                icon: Some("user".to_string()),
                parent_id: Some(0),
                component: Some("system/user/index".to_string()),
                sort: Some(1),
                keep_alive: Some(0),
                r#type: Some(0),
                remarks: None,
                leaf: Some(false),
                role_code: Some("admin".to_string()),
                disabled: Some(false),
                find_auth_id: None,
            },
            &parent_menu_id,
        )
        .await
        .unwrap();

    // Create button menus (type=2) under the parent
    let btn1_id: i64 = 2002;
    menu_repo
        .create(
            &CreateMenuRequest {
                name: "Add User".to_string(),
                code: None,
                permission: Some("system:user:add".to_string()),
                path_url: None,
                icon: None,
                parent_id: Some(parent_menu_id),
                component: None,
                sort: None,
                keep_alive: None,
                r#type: Some(2),
                remarks: None,
                leaf: None,
                role_code: Some("admin".to_string()),
                disabled: None,
                find_auth_id: None,
            },
            &btn1_id,
        )
        .await
        .unwrap();

    let btn2_id: i64 = 2003;
    menu_repo
        .create(
            &CreateMenuRequest {
                name: "Edit User".to_string(),
                code: None,
                permission: Some("system:user:edit".to_string()),
                path_url: None,
                icon: None,
                parent_id: Some(parent_menu_id),
                component: None,
                sort: None,
                keep_alive: None,
                r#type: Some(2),
                remarks: None,
                leaf: None,
                role_code: Some("admin".to_string()),
                disabled: None,
                find_auth_id: None,
            },
            &btn2_id,
        )
        .await
        .unwrap();

    // Assign menus to role
    menu_repo.assign_menu_to_role(role_id, parent_menu_id);
    menu_repo.assign_menu_to_role(role_id, btn1_id);
    menu_repo.assign_menu_to_role(role_id, btn2_id);

    // Call get_role_auth with role_code string (not role_id)
    let result = service.get_role_auth("admin").await;

    // Must succeed — not error
    assert!(result.is_ok(), "get_role_auth should succeed, got error: {:?}", result.err());

    let data = result.unwrap();

    // Data must not be empty
    assert!(!data.is_empty(), "get_role_auth should return non-empty data");

    // Verify structure: should have 1 SysAuthMenuVo for the parent page
    assert_eq!(data.len(), 1, "should have exactly 1 auth menu group");

    let auth_menu = &data[0];

    // Title should contain parent menu name
    assert!(
        auth_menu.title.contains("User Management"),
        "title should contain parent menu name, got: {}",
        auth_menu.title
    );

    // Auth list should contain the 2 button permissions
    assert_eq!(auth_menu.auth_list.len(), 2, "auth_list should have 2 buttons");

    // All buttons should be checked (use_auth_list == auth_list)
    assert!(auth_menu.is_check_all, "all buttons should be checked");
}

#[tokio::test]
async fn test_get_role_auth_role_not_found() {
    let menu_repo = Arc::new(FakeMenuRepository::new());
    let role_repo = Arc::new(FakeRoleRepository::new());
    let service = SysAuthService::new(menu_repo, role_repo);

    let result = service.get_role_auth("nonexistent").await;
    assert!(
        matches!(result, Err(AppError::NotFound(_))),
        "should return NotFound for unknown role code"
    );
}
