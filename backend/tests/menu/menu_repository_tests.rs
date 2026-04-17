use sea_orm::Database;
use uuid::Uuid;
use x_rust::common::traits::MenuRepository;
use x_rust::menu::domain::{CreateMenuRequest, UpdateMenuRequest};
use x_rust::menu::repository::SeaOrmMenuRepository;

fn uid() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}

fn test_db_path() -> String {
    let id = Uuid::new_v4();
    format!(
        "/home/zhangzexin/IdeaProjects/x-rust/data/menu_test_{}.db",
        id
    )
}

struct TestDb {
    conn: sea_orm::DatabaseConnection,
    path: String,
}

impl TestDb {
    async fn new() -> Self {
        let db_path = test_db_path();
        let url = format!("sqlite:{}", db_path);

        if std::path::Path::new(&db_path).exists() {
            std::fs::remove_file(&db_path).ok();
        }
        std::fs::write(&db_path, "").ok();

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool.close().await;

        let conn = Database::connect(&url).await.unwrap();
        Self {
            conn,
            path: db_path,
        }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

#[tokio::test]
async fn test_menu_repo_create_and_find() {
    let test_db = TestDb::new().await;
    let conn = test_db.conn.clone();
    let repo = SeaOrmMenuRepository::new(conn.into());

    let id = uid();
    let req = CreateMenuRequest {
        name: "Dashboard".to_string(),
        code: Some("dashboard".to_string()),
        permission: Some("menu:dashboard".to_string()),
        path_url: Some("/dashboard".to_string()),
        icon: Some("icon-dashboard".to_string()),
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

    let created_menu = repo.create(&req, &id).await.unwrap();

    let found_menu = repo.find_by_id(&id).await.unwrap().unwrap();

    assert_eq!(found_menu.id, id);
    assert_eq!(found_menu.name, "Dashboard");
    assert_eq!(found_menu.code, Some("dashboard".to_string()));
    assert_eq!(found_menu.permission, Some("menu:dashboard".to_string()));
    assert_eq!(found_menu.path_url, Some("/dashboard".to_string()));
    assert_eq!(found_menu.icon, Some("icon-dashboard".to_string()));
    assert_eq!(found_menu.parent_id, None);
    assert_eq!(found_menu.component, Some("DashboardView".to_string()));
    assert_eq!(found_menu.sort, Some(1));
    assert_eq!(found_menu.keep_alive, Some(0));
    assert_eq!(found_menu.r#type, Some(1));
    assert_eq!(found_menu.is_deleted, 0);
    assert_eq!(found_menu.remarks, Some("Main dashboard".to_string()));
    assert_eq!(found_menu.leaf, Some(true));
    assert_eq!(found_menu.role_code, Some("admin".to_string()));
    assert_eq!(found_menu.disabled, Some(false));
    assert_eq!(found_menu.find_auth_id, Some(1));
    assert_eq!(found_menu.created_at, created_menu.created_at);
    assert_eq!(found_menu.updated_at, created_menu.updated_at);
}

#[tokio::test]
async fn test_menu_repo_find_by_id_not_found() {
    let test_db = TestDb::new().await;
    let conn = test_db.conn.clone();
    let repo = SeaOrmMenuRepository::new(conn.into());

    let result = repo.find_by_id("nonexistent-id").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_menu_repo_find_all() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmMenuRepository::new(test_db.conn.clone().into());

    let req1 = CreateMenuRequest {
        name: "Menu 1".to_string(),
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
        leaf: None,
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    let id1 = uid();
    let id2 = uid();
    repo.create(&req1, &id1).await.unwrap();

    let req2 = CreateMenuRequest {
        name: "Menu 2".to_string(),
        code: None,
        permission: None,
        path_url: None,
        icon: None,
        parent_id: None,
        component: None,
        sort: Some(2),
        keep_alive: None,
        r#type: Some(1),
        remarks: None,
        leaf: None,
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    repo.create(&req2, &id2).await.unwrap();

    let menus = repo.find_all().await.unwrap();

    assert_eq!(menus.len(), 2);
    assert!(menus.iter().any(|m| m.id == id1 && m.name == "Menu 1"));
    assert!(menus.iter().any(|m| m.id == id2 && m.name == "Menu 2"));
}

#[tokio::test]
async fn test_menu_repo_find_tree() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmMenuRepository::new(test_db.conn.clone().into());

    let req1 = CreateMenuRequest {
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
    };
    let root_id = uid();
    let child_id = uid();
    repo.create(&req1, &root_id).await.unwrap();

    let req2 = CreateMenuRequest {
        name: "Child".to_string(),
        code: None,
        permission: None,
        path_url: None,
        icon: None,
        parent_id: Some(root_id.clone()),
        component: None,
        sort: Some(2),
        keep_alive: None,
        r#type: Some(1),
        remarks: None,
        leaf: Some(true),
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    repo.create(&req2, &child_id).await.unwrap();

    let tree = repo.find_tree().await.unwrap();

    assert_eq!(tree.len(), 2);
}

#[tokio::test]
async fn test_menu_repo_find_by_parent_id() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmMenuRepository::new(test_db.conn.clone().into());

    let req1 = CreateMenuRequest {
        name: "Parent".to_string(),
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
    };
    let parent_id = uid();
    let child_id = uid();
    repo.create(&req1, &parent_id).await.unwrap();

    let req2 = CreateMenuRequest {
        name: "Child 1".to_string(),
        code: None,
        permission: None,
        path_url: None,
        icon: None,
        parent_id: Some(parent_id.clone()),
        component: None,
        sort: Some(2),
        keep_alive: None,
        r#type: Some(1),
        remarks: None,
        leaf: Some(true),
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    repo.create(&req2, &child_id).await.unwrap();

    let req3 = CreateMenuRequest {
        name: "Child 2".to_string(),
        code: None,
        permission: None,
        path_url: None,
        icon: None,
        parent_id: Some(parent_id.clone()),
        component: None,
        sort: Some(3),
        keep_alive: None,
        r#type: Some(1),
        remarks: None,
        leaf: Some(true),
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    repo.create(&req3, "child2").await.unwrap();

    let children = repo.find_by_parent_id(Some(&parent_id)).await.unwrap();
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].id, child_id);

    let roots = repo.find_by_parent_id(None).await.unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].id, parent_id);
}

#[tokio::test]
async fn test_menu_repo_update() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmMenuRepository::new(test_db.conn.clone().into());

    let id = uid();
    let req = CreateMenuRequest {
        name: "Old Name".to_string(),
        code: Some("old-code".to_string()),
        permission: None,
        path_url: None,
        icon: None,
        parent_id: None,
        component: None,
        sort: Some(1),
        keep_alive: None,
        r#type: Some(1),
        remarks: None,
        leaf: None,
        role_code: None,
        disabled: None,
        find_auth_id: None,
    };
    repo.create(&req, &id).await.unwrap();

    let update_req = UpdateMenuRequest {
        name: Some("New Name".to_string()),
        code: Some("new-code".to_string()),
        permission: Some("new:perm".to_string()),
        path_url: Some("/new-path".to_string()),
        icon: Some("new-icon".to_string()),
        parent_id: Some("new-parent".to_string()),
        component: Some("NewComponent".to_string()),
        sort: Some(10),
        keep_alive: Some(1),
        r#type: Some(2),
        remarks: Some("Updated remarks".to_string()),
        leaf: Some(true),
        role_code: Some("new-role".to_string()),
        disabled: Some(true),
        find_auth_id: Some(2),
    };

    let updated_menu = repo.update(&id, &update_req).await.unwrap().unwrap();

    assert_eq!(updated_menu.name, "New Name");
    assert_eq!(updated_menu.code, Some("new-code".to_string()));
    assert_eq!(updated_menu.permission, Some("new:perm".to_string()));
    assert_eq!(updated_menu.path_url, Some("/new-path".to_string()));
    assert_eq!(updated_menu.icon, Some("new-icon".to_string()));
    assert_eq!(updated_menu.parent_id, Some("new-parent".to_string()));
    assert_eq!(updated_menu.component, Some("NewComponent".to_string()));
    assert_eq!(updated_menu.sort, Some(10));
    assert_eq!(updated_menu.keep_alive, Some(1));
    assert_eq!(updated_menu.r#type, Some(2));
    assert_eq!(updated_menu.remarks, Some("Updated remarks".to_string()));
    assert_eq!(updated_menu.leaf, Some(true));
    assert_eq!(updated_menu.role_code, Some("new-role".to_string()));
    assert_eq!(updated_menu.disabled, Some(true));
    assert_eq!(updated_menu.find_auth_id, Some(2));
}

#[tokio::test]
async fn test_menu_repo_update_not_found() {
    let update_req = UpdateMenuRequest {
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

    let test_db = TestDb::new().await;
    let conn = test_db.conn.clone();
    let repo = SeaOrmMenuRepository::new(conn.into());

    let result = repo.update("nonexistent-id", &update_req).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_menu_repo_delete() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmMenuRepository::new(test_db.conn.clone().into());

    let id = uid();
    let req = CreateMenuRequest {
        name: "To Delete".to_string(),
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
    repo.create(&req, &id).await.unwrap();

    let deleted = repo.delete(&id).await.unwrap();
    assert!(deleted);

    let found = repo.find_by_id(&id).await.unwrap();
    assert!(found.is_none());

    // Should not appear in find_all
    let all = repo.find_all().await.unwrap();
    assert_eq!(all.len(), 0);

    // Should not appear in find_tree
    let tree = repo.find_tree().await.unwrap();
    assert_eq!(tree.len(), 0);
}

#[tokio::test]
async fn test_menu_repo_delete_not_found() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmMenuRepository::new(test_db.conn.clone().into());

    let deleted = repo.delete("nonexistent-id").await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_menu_repo_create_with_minimal_fields() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmMenuRepository::new(test_db.conn.clone().into());

    let req = CreateMenuRequest {
        name: "Minimal Menu".to_string(),
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

    let created = repo.create(&req, "minimal").await.unwrap();
    assert_eq!(created.name, "Minimal Menu");
    assert_eq!(created.is_deleted, 0);
}
