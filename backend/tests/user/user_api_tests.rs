use axum::body::{Body, Bytes, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use x_rust::api::routes::create_router;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};
use x_rust::common::util::encrypt_password;

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

fn test_db_path() -> String {
    let id = uuid::Uuid::new_v4();
    format!(
        "/home/zhangzexin/IdeaProjects/rust-admin/backend/data/user_api_{}.db",
        id
    )
}

struct TestDb {
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
            .expect("Failed to connect");

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS p_sys_user (
                id TEXT PRIMARY KEY,
                username TEXT NOT NULL,
                email TEXT,
                phone TEXT,
                password TEXT,
                org_id TEXT,
                lock_time TEXT,
                last_login_time TEXT,
                try_count INTEGER DEFAULT 0,
                lock_flag INTEGER DEFAULT 1,
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                is_deleted INTEGER DEFAULT 0,
                remarks TEXT,
                real_name TEXT,
                pass_update_time TEXT,
                card TEXT,
                is_show INTEGER DEFAULT 1,
                enable INTEGER DEFAULT 1,
                first_login INTEGER DEFAULT 1,
                sex TEXT
            )"
        )
        .execute(&pool)
        .await
        .expect("Failed to create user table");

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS p_sys_role (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                code TEXT,
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                is_deleted INTEGER DEFAULT 0,
                remarks TEXT,
                description TEXT,
                is_edit INTEGER DEFAULT 1,
                ds_type INTEGER,
                ds_scope TEXT
            )"
        )
        .execute(&pool)
        .await
        .expect("Failed to create role table");

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS p_sys_user_role (
                user_id TEXT NOT NULL,
                role_id TEXT NOT NULL,
                PRIMARY KEY (user_id, role_id)
            )"
        )
        .execute(&pool)
        .await
        .expect("Failed to create user_role table");

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS p_sys_menu (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                code TEXT,
                permission TEXT,
                path_url TEXT,
                icon TEXT,
                parent_id TEXT,
                component TEXT,
                sort INTEGER DEFAULT 0,
                keep_alive INTEGER DEFAULT 0,
                type INTEGER DEFAULT 0,
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                is_deleted INTEGER DEFAULT 0,
                remarks TEXT,
                leaf INTEGER DEFAULT 0,
                role_code TEXT,
                disabled INTEGER DEFAULT 0,
                find_auth_id INTEGER
            )"
        )
        .execute(&pool)
        .await
        .expect("Failed to create menu table");

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS p_sys_role_menu (
                role_id TEXT NOT NULL,
                menu_id TEXT NOT NULL,
                PRIMARY KEY (role_id, menu_id)
            )"
        )
        .execute(&pool)
        .await
        .expect("Failed to create role_menu table");

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS p_sys_org (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                sort INTEGER DEFAULT 0,
                parent_id TEXT,
                create_time TEXT NOT NULL,
                update_time TEXT NOT NULL,
                is_deleted INTEGER DEFAULT 0,
                remarks TEXT,
                org_duty TEXT,
                desrc TEXT,
                type TEXT,
                parent_name TEXT,
                is_out INTEGER
            )"
        )
        .execute(&pool)
        .await
        .expect("Failed to create org table");

        pool.close().await;

        Self { path: db_path }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

async fn create_test_app() -> (axum::Router, TestDb) {
    let test_db = TestDb::new().await;
    let url = format!("sqlite:{}", test_db.path);

    let conn = sea_orm::Database::connect(&url)
        .await
        .expect("Failed to connect");

    use std::sync::Arc;
    use x_rust::api::AppState;
    use x_rust::auth::service::AuthService;
    use x_rust::common::traits::{
        MenuRepository, OrgRepository, RoleRepository, TokenStore, UserRepository,
    };
    use x_rust::menu::repository::SeaOrmMenuRepository;
    use x_rust::menu::service::MenuService;
    use x_rust::org::repository::SeaOrmOrgRepository;
    use x_rust::org::service::OrgService;
    use x_rust::role::repository::SeaOrmRoleRepository;
    use x_rust::role::service::RoleService;
    use x_rust::sys_auth::service::SysAuthService;
    use x_rust::user::repository::SeaOrmUserRepository;
    use x_rust::user::service::UserService;

    let conn = Arc::new(conn);
    let user_repo: Arc<dyn UserRepository> = Arc::new(SeaOrmUserRepository::new(conn.clone()));
    let role_repo: Arc<dyn RoleRepository> = Arc::new(SeaOrmRoleRepository::new(conn.clone()));
    let user_service = Arc::new(UserService::new(user_repo.clone(), role_repo.clone()));
    let role_service = Arc::new(RoleService::new(role_repo.clone()));
    let token_store: Arc<dyn TokenStore> = Arc::new(FakeTokenStore::new());
    let auth_service = Arc::new(AuthService::new(
        user_repo,
        token_store,
        role_repo.clone(),
        "test-secret",
    ));
    let menu_repo: Arc<dyn MenuRepository> = Arc::new(SeaOrmMenuRepository::new(conn.clone()));
    let menu_service = Arc::new(MenuService::new(menu_repo.clone(), role_repo.clone()));
    let org_repo: Arc<dyn OrgRepository> = Arc::new(SeaOrmOrgRepository::new(conn.clone()));
    let org_service = Arc::new(OrgService::new(org_repo));
    let sys_auth_service = Arc::new(SysAuthService::new(menu_repo, role_repo.clone()));

    let state = AppState {
        user_service,
        role_service,
        auth_service,
        menu_service,
        org_service,
        sys_auth_service,
    };
    let router = create_router(state);
    (router, test_db)
}

fn auth_request(token: &str, mut req: Request<Body>) -> Request<Body> {
    req.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        axum::http::HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
    );
    req
}

async fn login(app: axum::Router, test_db: &TestDb) -> String {
    use sea_orm::Database;
    use std::sync::Arc;
    use x_rust::common::traits::UserRepository;
    use x_rust::common::util::md5_encrypt;
    use x_rust::user::domain::CreateUserRequest;

    let conn = Database::connect(&format!("sqlite:{}", test_db.path))
        .await
        .unwrap();

    let password_hash = md5_encrypt("password123");
    let user_repo = x_rust::user::repository::SeaOrmUserRepository::new(Arc::new(conn));
    user_repo
        .create(
            &CreateUserRequest {
                username: "testuser".to_string(),
                phone: None,
                email: None,
                real_name: None,
                password: Some(password_hash),
                org_id: None,
                remarks: None,
                card: None,
                sex: None,
            },
            "1",
        )
        .await
        .ok();

    let encrypted_password = encrypt_password("password123");
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "username=testuser&password={}&grant_type=password&scope=server",
                    encrypted_password
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    let body: Bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    if let Some(access_token) = json.get("access_token") {
        access_token.as_str().unwrap().to_string()
    } else {
        panic!("Login failed, response: {:?}", json);
    }
}

#[tokio::test]
async fn test_create_user() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    let response = app
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/api/sysUser")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"username":"newuser","email":"test2@example.com","real_name":"Test User"}"#,
                ))
                .unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let data = json.get("data").unwrap();
    assert!(data.get("id").is_some());
    assert_eq!(data.get("username").unwrap(), "newuser");
    assert_eq!(data.get("email").unwrap(), "test2@example.com");
}

#[tokio::test]
async fn test_get_users_page_default() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    let response = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri("/api/sysUser/getPage")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    let status = response.status();
    let body: Bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body_str = String::from_utf8_lossy(&body);
    assert!(status.is_success(), "Got error body: {}", body_str);
    let json: Value = serde_json::from_slice(&body).unwrap();
    let data = json.get("data").unwrap();
    assert!(data.get("records").unwrap().is_array());
    assert!(data.get("total").unwrap().as_i64().unwrap() >= 1);
    assert_eq!(data.get("current").unwrap().as_i64().unwrap(), 1);
    assert_eq!(data.get("size").unwrap().as_i64().unwrap(), 10);
}

#[tokio::test]
async fn test_user_crud_flow() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    let create_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/api/sysUser")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"username":"initial","email":"initial@example.com"}"#,
                ))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::OK);
    let body: Bytes = to_bytes(create_resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let user = json.get("data").unwrap();
    let user_id = user.get("id").unwrap().as_str().unwrap();

    let get_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri(&format!("/api/sysUser/{}", user_id))
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);

    let update_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("PUT")
                .uri(&format!("/api/sysUser/{}", user_id))
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"username":"updated","email":"updated@example.com"}"#,
                ))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(update_resp.status(), StatusCode::OK);
    let body: Bytes = to_bytes(update_resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let updated = json.get("data").unwrap();
    assert_eq!(updated.get("username").unwrap(), "updated");

    let delete_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("DELETE")
                .uri(&format!("/api/sysUser/{}", user_id))
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(delete_resp.status(), StatusCode::NO_CONTENT);

    let get_after = app
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri(&format!("/api/sysUser/{}", user_id))
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(get_after.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_user_requires_auth() {
    let (app, _db_path) = create_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/sysUser")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_user_invalid_token() {
    let (app, _db_path) = create_test_app().await;

    let response = app
        .oneshot(auth_request(
            "invalid-token",
            Request::builder()
                .method("GET")
                .uri("/api/sysUser")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_get_users_page_with_params() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    for i in 1..=5 {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let create_resp = app
            .clone()
            .oneshot(auth_request(
                &token,
                Request::builder()
                    .method("POST")
                    .uri("/api/sysUser")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"username":"pageuser{}","email":"pageuser{}@example.com"}}"#,
                        i, i
                    )))
                    .unwrap(),
            ))
            .await
            .unwrap();
        assert_eq!(
            create_resp.status(),
            StatusCode::OK,
            "Failed to create user {}",
            i
        );
    }

    let response = app
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri("/api/sysUser/getPage?current=1&size=2")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let data = json.get("data").unwrap();
    let total = data.get("total").unwrap().as_i64().unwrap();
    let records_len = data.get("records").unwrap().as_array().unwrap().len();
    assert_eq!(records_len, 2, "Expected 2 records, got {}", records_len);
    assert!(total >= 6, "Expected total >= 6, got {}", total);
    assert_eq!(data.get("current").unwrap().as_i64().unwrap(), 1);
    assert_eq!(data.get("size").unwrap().as_i64().unwrap(), 2);
}

#[tokio::test]
async fn test_get_users_page_requires_auth() {
    let (app, _db_path) = create_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/sysUser/getPage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
