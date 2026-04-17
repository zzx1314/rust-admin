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
        "/home/zhangzexin/IdeaProjects/x-rust/data/org_api_{}.db",
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
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("Failed to migrate");
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

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("Failed to connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to migrate");
    pool.close().await;

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
    use x_rust::user::repository::SeaOrmUserRepository;
    use x_rust::user::service::UserService;

    let conn = Arc::new(conn);
    let user_repo: Arc<dyn UserRepository> = Arc::new(SeaOrmUserRepository::new(conn.clone()));
    let user_service = Arc::new(UserService::new(user_repo.clone()));
    let role_repo: Arc<dyn RoleRepository> = Arc::new(SeaOrmRoleRepository::new(conn.clone()));
    let role_service = Arc::new(RoleService::new(role_repo.clone()));
    let token_store: Arc<dyn TokenStore> = Arc::new(FakeTokenStore::new());
    let auth_service = Arc::new(AuthService::new(user_repo, token_store, "test-secret"));
    let menu_repo: Arc<dyn MenuRepository> = Arc::new(SeaOrmMenuRepository::new(conn.clone()));
    let menu_service = Arc::new(MenuService::new(menu_repo, role_repo.clone()));
    let org_repo: Arc<dyn OrgRepository> = Arc::new(SeaOrmOrgRepository::new(conn.clone()));
    let org_service = Arc::new(OrgService::new(org_repo));

    let state = AppState {
        user_service,
        role_service,
        auth_service,
        menu_service,
        org_service,
    };

    (create_router(state), test_db)
}

fn auth_request(token: &str, mut req: Request<Body>) -> Request<Body> {
    req.headers_mut().insert(
        axum::http::header::AUTHORIZATION,
        axum::http::HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
    );
    req
}

async fn login(app: axum::Router, test_db: &TestDb) -> String {
    use x_rust::common::traits::UserRepository;
    use x_rust::common::util::md5_encrypt;
    use x_rust::user::domain::CreateUserRequest;

    let conn = sea_orm::Database::connect(&format!("sqlite:{}", test_db.path))
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
                .uri("/auth/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "username=testuser&password={}&grant_type=password&scope=server",
                    encrypted_password
                )))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = resp.status();
    let body: Bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();

    if status != StatusCode::OK {
        let json: Value = serde_json::from_slice(&body).unwrap();
        panic!(
            "Login failed with status {:?}: {}",
            status,
            serde_json::to_string_pretty(&json).unwrap()
        );
    }

    let json: Value = serde_json::from_slice(&body).unwrap();
    json.get("data")
        .unwrap_or_else(|| {
            panic!(
                "Missing 'data' field in response: {}",
                serde_json::to_string_pretty(&json).unwrap()
            )
        })
        .get("access_token")
        .unwrap_or_else(|| {
            panic!(
                "Missing 'access_token' field in data: {}",
                serde_json::to_string_pretty(&json).unwrap()
            )
        })
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "'access_token' is not a string: {}",
                serde_json::to_string_pretty(&json).unwrap()
            )
        })
        .to_string()
}

#[tokio::test]
async fn test_get_org_tree_no_filter() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    let response = app
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri("/orgs/tree")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let data = json.get("data").unwrap();
    assert!(data.is_array());
}

#[tokio::test]
async fn test_get_org_tree_with_name_filter() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    app.clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/orgs/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Engineering","sort":1}"#))
                .unwrap(),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/orgs/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Sales","sort":2}"#))
                .unwrap(),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri("/orgs/tree?name=Eng")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let data = json.get("data").unwrap();
    let arr = data.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0].get("name").unwrap(), "Engineering");
}

#[tokio::test]
async fn test_get_org_tree_with_type_filter() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    app.clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/orgs/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Company","sort":1,"type":"company"}"#,
                ))
                .unwrap(),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/orgs/")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"name":"Dept","sort":2,"type":"department"}"#,
                ))
                .unwrap(),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri("/orgs/tree?type=department")
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let data = json.get("data").unwrap();
    let arr = data.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0].get("name").unwrap(), "Dept");
}

#[tokio::test]
async fn test_org_crud_flow() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    let create_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/orgs/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Initial Org","sort":1}"#))
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::OK);
    let body: Bytes = to_bytes(create_resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let org = json.get("data").unwrap();
    let org_id = org.get("id").unwrap().as_str().unwrap();

    let get_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri(&format!("/orgs/{}", org_id))
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let body: Bytes = to_bytes(get_resp.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let fetched = json.get("data").unwrap();
    assert_eq!(fetched.get("name").unwrap(), "Initial Org");

    let update_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("PUT")
                .uri(&format!("/orgs/{}", org_id))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Updated Org","sort":10}"#))
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
    assert_eq!(updated.get("name").unwrap(), "Updated Org");
    assert_eq!(updated.get("sort").unwrap().as_i64().unwrap(), 10);

    let delete_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("DELETE")
                .uri(&format!("/orgs/{}", org_id))
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
                .uri(&format!("/orgs/{}", org_id))
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(get_after.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_get_orgs_by_parent() {
    let (app, test_db) = create_test_app().await;
    let token = login(app.clone(), &test_db).await;

    let parent_resp = app
        .clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/orgs/")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"name":"Parent Org","sort":1}"#))
                .unwrap(),
        ))
        .await
        .unwrap();
    let body: Bytes = to_bytes(parent_resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let parent_id = json
        .get("data")
        .unwrap()
        .get("id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();

    app.clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/orgs/")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"name":"Child 1","sort":2,"parent_id":"{}"}}"#,
                    parent_id
                )))
                .unwrap(),
        ))
        .await
        .unwrap();

    app.clone()
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("POST")
                .uri("/orgs/")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"name":"Child 2","sort":3,"parent_id":"{}"}}"#,
                    parent_id
                )))
                .unwrap(),
        ))
        .await
        .unwrap();

    let response = app
        .oneshot(auth_request(
            &token,
            Request::builder()
                .method("GET")
                .uri(&format!("/orgs/parent?parent_id={}", parent_id))
                .body(Body::empty())
                .unwrap(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    let data = json.get("data").unwrap();
    let arr = data.as_array().unwrap();
    assert_eq!(arr.len(), 2);
}

#[tokio::test]
async fn test_org_requires_auth() {
    let (app, _test_db) = create_test_app().await;

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/orgs/tree")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
