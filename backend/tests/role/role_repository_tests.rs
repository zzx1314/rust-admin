use sea_orm::Database;
use uuid::Uuid;
use x_rust::common::traits::{RoleRepository, UserRepository};
use x_rust::role::domain::{CreateRoleRequest, UpdateRoleRequest};
use x_rust::role::repository::SeaOrmRoleRepository;
use x_rust::user::domain::CreateUserRequest;
use x_rust::user::repository::SeaOrmUserRepository;

fn test_db_path() -> String {
    let id = Uuid::new_v4();
    format!(
        "/home/zhangzexin/IdeaProjects/x-rust/data/role_test_{}.db",
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
async fn test_role_repo_create_and_find() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmRoleRepository::new(test_db.conn.clone().into());

    let id = "role-1";
    let req = CreateRoleRequest {
        name: "Admin".to_string(),
        code: Some("ADMIN".to_string()),
        description: Some("Administrator role".to_string()),
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };

    let created_role = repo.create(&req, id).await.unwrap();

    let found_role = repo.find_by_id(id).await.unwrap().unwrap();

    assert_eq!(found_role.id, id);
    assert_eq!(found_role.name, "Admin");
    assert_eq!(
        found_role.description,
        Some("Administrator role".to_string())
    );
    assert_eq!(found_role.create_time, created_role.create_time);
    assert_eq!(found_role.update_time, created_role.update_time);
}

#[tokio::test]
async fn test_role_repo_find_users_by_role() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmRoleRepository::new(test_db.conn.clone().into());
    let user_repo = SeaOrmUserRepository::new(test_db.conn.clone().into());

    let role_id = "role-1";
    let role_req = CreateRoleRequest {
        name: "Admin".to_string(),
        code: None,
        description: Some("Administrator".to_string()),
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };
    repo.create(&role_req, role_id).await.unwrap();

    let user_req1 = CreateUserRequest {
        username: "johndoe".to_string(),
        phone: None,
        email: Some("john@example.com".to_string()),
        real_name: Some("John Doe".to_string()),
        password: None,
        org_id: None,
        remarks: None,
        card: None,
        sex: None,
    };
    let user1 = user_repo.create(&user_req1, "1").await.unwrap();
    let user_id1 = user1.id;

    let user_req2 = CreateUserRequest {
        username: "janesmith".to_string(),
        phone: None,
        email: Some("jane@example.com".to_string()),
        real_name: Some("Jane Smith".to_string()),
        password: None,
        org_id: None,
        remarks: None,
        card: None,
        sex: None,
    };
    let user2 = user_repo.create(&user_req2, "2").await.unwrap();
    let user_id2 = user2.id;

    repo.assign_role_to_user(&user_id1, role_id).await.unwrap();
    repo.assign_role_to_user(&user_id2, role_id).await.unwrap();

    let users = repo.find_users_by_role_id(role_id).await.unwrap();

    assert_eq!(users.len(), 2);
    assert!(users.iter().any(|u| u.id == user_id1
        && u.username == "johndoe"
        && u.email.as_deref() == Some("john@example.com")));
    assert!(users.iter().any(|u| u.id == user_id2
        && u.username == "janesmith"
        && u.email.as_deref() == Some("jane@example.com")));
}

#[tokio::test]
async fn test_role_repo_find_all() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmRoleRepository::new(test_db.conn.clone().into());

    let id1 = "role-1";
    let req1 = CreateRoleRequest {
        name: "Admin".to_string(),
        code: None,
        description: Some("Administrator".to_string()),
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };
    repo.create(&req1, id1).await.unwrap();

    let id2 = "role-2";
    let req2 = CreateRoleRequest {
        name: "User".to_string(),
        code: None,
        description: Some("Regular user".to_string()),
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };
    repo.create(&req2, id2).await.unwrap();

    let roles = repo.find_all().await.unwrap();

    assert_eq!(roles.len(), 2);
    assert!(roles.iter().any(|r| r.id == id1
        && r.name == "Admin"
        && r.description == Some("Administrator".to_string())));
    assert!(roles.iter().any(|r| r.id == id2
        && r.name == "User"
        && r.description == Some("Regular user".to_string())));
}

#[tokio::test]
async fn test_role_repo_update() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmRoleRepository::new(test_db.conn.clone().into());

    let id = "role-1";
    let req = CreateRoleRequest {
        name: "Admin".to_string(),
        code: None,
        description: Some("Administrator role".to_string()),
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };
    repo.create(&req, id).await.unwrap();

    let update_req = UpdateRoleRequest {
        name: Some("Super Admin".to_string()),
        code: None,
        description: Some("Super administrator role".to_string()),
        remarks: None,
        is_edit: None,
        ds_type: None,
        ds_scope: None,
    };

    let updated_role = repo.update(id, &update_req).await.unwrap().unwrap();

    assert_eq!(updated_role.name, "Super Admin");
    assert_eq!(
        updated_role.description,
        Some("Super administrator role".to_string())
    );
    assert_eq!(updated_role.id, id);
}

#[tokio::test]
async fn test_role_repo_assign_and_remove() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmRoleRepository::new(test_db.conn.clone().into());
    let user_repo = SeaOrmUserRepository::new(test_db.conn.clone().into());

    let user_req = CreateUserRequest {
        username: "johndoe".to_string(),
        phone: None,
        email: Some("john@example.com".to_string()),
        real_name: Some("John Doe".to_string()),
        password: None,
        org_id: None,
        remarks: None,
        card: None,
        sex: None,
    };
    let user = user_repo.create(&user_req, "1").await.unwrap();
    let user_id_str = user.id;

    let role_id = "role-1";
    let role_req = CreateRoleRequest {
        name: "Admin".to_string(),
        code: None,
        description: Some("Administrator".to_string()),
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };
    repo.create(&role_req, role_id).await.unwrap();

    repo.assign_role_to_user(&user_id_str, role_id)
        .await
        .unwrap();

    let roles = repo.find_roles_by_user_id(&user_id_str).await.unwrap();
    assert_eq!(roles.len(), 1);
    assert_eq!(roles[0].id, role_id);

    let removed = repo
        .remove_role_from_user(&user_id_str, role_id)
        .await
        .unwrap();
    assert!(removed);

    let roles_after = repo.find_roles_by_user_id(&user_id_str).await.unwrap();
    assert!(roles_after.is_empty());
}

#[tokio::test]
async fn test_role_repo_find_roles_by_user() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmRoleRepository::new(test_db.conn.clone().into());
    let user_repo = SeaOrmUserRepository::new(test_db.conn.clone().into());

    let user_req = CreateUserRequest {
        username: "johndoe".to_string(),
        phone: None,
        email: Some("john@example.com".to_string()),
        real_name: None,
        password: None,
        org_id: None,
        remarks: None,
        card: None,
        sex: None,
    };
    let user = user_repo.create(&user_req, "1").await.unwrap();
    let user_id_str = user.id;

    let role_id1 = "role-1";
    let role_req1 = CreateRoleRequest {
        name: "Admin".to_string(),
        code: None,
        description: None,
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };
    repo.create(&role_req1, role_id1).await.unwrap();

    let role_id2 = "role-2";
    let role_req2 = CreateRoleRequest {
        name: "User".to_string(),
        code: None,
        description: None,
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };
    repo.create(&role_req2, role_id2).await.unwrap();

    repo.assign_role_to_user(&user_id_str, role_id1)
        .await
        .unwrap();
    repo.assign_role_to_user(&user_id_str, role_id2)
        .await
        .unwrap();

    let roles = repo.find_roles_by_user_id(&user_id_str).await.unwrap();

    assert_eq!(roles.len(), 2);
    assert!(roles.iter().any(|r| r.id == role_id1 && r.name == "Admin"));
    assert!(roles.iter().any(|r| r.id == role_id2 && r.name == "User"));
}

#[tokio::test]
async fn test_role_repo_delete() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmRoleRepository::new(test_db.conn.clone().into());

    let id = "role-1";
    let req = CreateRoleRequest {
        name: "Admin".to_string(),
        code: None,
        description: Some("Administrator role".to_string()),
        remarks: None,
        ds_type: None,
        ds_scope: None,
    };
    repo.create(&req, id).await.unwrap();

    let deleted = repo.delete(id).await.unwrap();
    assert!(deleted);

    let found_role = repo.find_by_id(id).await.unwrap();
    assert!(found_role.is_none());
}
