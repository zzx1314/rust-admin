use sea_orm::Database;
use x_rust::common::traits::UserRepository;
use x_rust::user::domain::{CreateUserRequest, UpdateUserRequest};
use x_rust::user::repository::SeaOrmUserRepository;

fn uid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}

struct TestDb {
    path: String,
}

impl TestDb {
    async fn new() -> Self {
        let id = uuid::Uuid::new_v4();
        let db_path = format!(
            "/home/zhangzexin/IdeaProjects/x-rust/data/user_repo_{}.db",
            id
        );
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

        Self { path: db_path }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

async fn create_test_db() -> (sea_orm::DatabaseConnection, TestDb) {
    let test_db = TestDb::new().await;
    let url = format!("sqlite:{}", test_db.path);

    let conn = Database::connect(&url).await.unwrap();
    (conn, test_db)
}

#[tokio::test]
async fn test_user_repo_create_and_find() {
    let (pool, _test_db) = create_test_db().await;
    let repo = SeaOrmUserRepository::new(pool.into());

    let id = uid();
    let req = CreateUserRequest {
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

    let created_user = repo.create(&req, &id).await.unwrap();

    let found_user = repo.find_by_id(&id).await.unwrap().unwrap();

    assert_eq!(found_user.id, id);
    assert_eq!(found_user.username, "johndoe");
    assert_eq!(found_user.email, Some("john@example.com".to_string()));
    assert_eq!(found_user.create_time, created_user.create_time);
    assert_eq!(found_user.update_time, created_user.update_time);
}

#[tokio::test]
async fn test_user_repo_find_by_id_not_found() {
    let (pool, _test_db) = create_test_db().await;
    let repo = SeaOrmUserRepository::new(pool.into());

    let result = repo.find_by_id("999").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_user_repo_find_all() {
    let (pool, _test_db) = create_test_db().await;
    let repo = SeaOrmUserRepository::new(pool.into());

    let req1 = CreateUserRequest {
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
    let id1 = uid();
    let id2 = uid();
    repo.create(&req1, &id1).await.unwrap();

    let req2 = CreateUserRequest {
        username: "janedoe".to_string(),
        phone: None,
        email: Some("jane@example.com".to_string()),
        real_name: None,
        password: None,
        org_id: None,
        remarks: None,
        card: None,
        sex: None,
    };
    repo.create(&req2, &id2).await.unwrap();

    let users = repo.find_all().await.unwrap();

    assert_eq!(users.len(), 2);
    assert!(users.iter().any(|u| u.id == id1 && u.username == "johndoe"));
    assert!(users.iter().any(|u| u.id == id2 && u.username == "janedoe"));
}

#[tokio::test]
async fn test_user_repo_update() {
    let (pool, _test_db) = create_test_db().await;
    let repo = SeaOrmUserRepository::new(pool.into());

    let id = uid();
    let req = CreateUserRequest {
        username: "updateduser".to_string(),
        phone: None,
        email: None,
        real_name: None,
        password: None,
        org_id: None,
        remarks: None,
        card: None,
        sex: None,
    };
    repo.create(&req, &id).await.unwrap();

    let update_req = UpdateUserRequest {
        username: Some("updated".to_string()),
        phone: None,
        email: Some("updated@example.com".to_string()),
        real_name: None,
        org_id: None,
        remarks: None,
        card: None,
        is_show: None,
        enable: None,
        sex: None,
    };

    let updated_user = repo.update(&id, &update_req).await.unwrap().unwrap();

    assert_eq!(updated_user.id, id);
}

#[tokio::test]
async fn test_user_repo_update_not_found() {
    let (pool, _test_db) = create_test_db().await;
    let repo = SeaOrmUserRepository::new(pool.into());

    use x_rust::user::domain::UpdateUserRequest;
    let update_req = UpdateUserRequest {
        username: Some("updated".to_string()),
        phone: None,
        email: None,
        real_name: None,
        org_id: None,
        remarks: None,
        card: None,
        is_show: None,
        enable: None,
        sex: None,
    };

    let result = repo.update("999", &update_req).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_user_repo_delete() {
    let (pool, _test_db) = create_test_db().await;
    let repo = SeaOrmUserRepository::new(pool.into());

    let id = uid();
    let req = CreateUserRequest {
        username: "deleteme".to_string(),
        phone: None,
        email: Some("delete@example.com".to_string()),
        real_name: None,
        password: None,
        org_id: None,
        remarks: None,
        card: None,
        sex: None,
    };
    repo.create(&req, &id).await.unwrap();

    let deleted = repo.delete(&id).await.unwrap();
    assert!(deleted);

    let found_user = repo.find_by_id(&id).await.unwrap();
    assert!(found_user.is_none());
}

#[tokio::test]
async fn test_user_repo_delete_not_found() {
    let (pool, _test_db) = create_test_db().await;
    let repo = SeaOrmUserRepository::new(pool.into());

    let deleted = repo.delete("999").await.unwrap();
    assert!(!deleted);
}
