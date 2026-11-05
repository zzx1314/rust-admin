//! Data-level permission tests for the dog business module.
//!
//! Parks, maps and tasks belong to the user that created them. Regular users
//! only see and mutate their own rows; holders of the built-in administrator
//! role (`sys_role.code = "110"`) see every row.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use tower::ServiceExt;

use x_rust::api::AppState;
use x_rust::app::App;
use x_rust::business::dog::map::domain::CreateMapRequest;
use x_rust::business::dog::park::domain::{CreateParkRequest, UpdateParkRequest};
use x_rust::business::dog::task::domain::{CreateTaskRequest, TaskPageQuery};
use x_rust::common::data_scope::DataScope;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};

const USER_A: i64 = 101;
const USER_B: i64 = 102;
const JWT_SECRET: &str = "test-secret";

struct FakeTokenStore {
    store: Arc<Mutex<HashMap<String, String>>>,
}

impl FakeTokenStore {
    fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn insert_token(&self, user_id: i64, token: &str) {
        self.store
            .lock()
            .unwrap()
            .insert(format!("auth:token:{}", user_id), token.to_string());
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

    fn set_refresh_token(
        &self,
        user_id: &str,
        token: &str,
        _ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        let token = token.to_string();
        Box::pin(async move {
            store.lock().unwrap().insert(key, token);
            Ok(())
        })
    }

    fn get_refresh_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        Box::pin(async move { Ok(store.lock().unwrap().get(&key).cloned()) })
    }

    fn delete_refresh_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        Box::pin(async move {
            store.lock().unwrap().remove(&key);
            Ok(())
        })
    }
}

async fn test_state() -> AppState {
    let app = App::new("sqlite::memory:")
        .await
        .expect("in-memory database must migrate");
    app.build_state_with_token_store(Arc::new(FakeTokenStore::new()), JWT_SECRET)
}

fn token_for(user_id: i64, username: &str) -> String {
    #[derive(Serialize)]
    struct Claims {
        sub: i64,
        username: String,
        exp: u64,
        iat: u64,
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = Claims {
        sub: user_id,
        username: username.to_string(),
        exp: now + 3600,
        iat: now,
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("token encoding must work")
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn create_park(name: &str) -> CreateParkRequest {
    CreateParkRequest {
        park_name: name.to_string(),
        park_description: None,
        park_address: None,
    }
}

fn create_map(name: &str, park_id: i64) -> CreateMapRequest {
    CreateMapRequest {
        map_name: name.to_string(),
        map_type: None,
        map_data: None,
        park_id,
    }
}

fn create_task(name: &str) -> CreateTaskRequest {
    CreateTaskRequest {
        task_name: name.to_string(),
        task_description: None,
        task_status: None,
        workflow_id: None,
        workflow_uuid: None,
        device_id: None,
        route_id: None,
    }
}

fn owner_scope(user_id: i64) -> DataScope {
    DataScope::new(user_id, false)
}

fn admin_scope() -> DataScope {
    DataScope::new(1, true)
}

#[tokio::test]
async fn parks_are_visible_only_to_their_owner() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let b = owner_scope(USER_B);

    let park = state
        .dog_park_service
        .create(create_park("A 园区"), &a)
        .await
        .expect("user A may create a park");

    assert_eq!(park.user_id, USER_A);

    let a_parks = state.dog_park_service.list_all(&a).await.unwrap();
    assert_eq!(a_parks.len(), 1);

    let b_parks = state.dog_park_service.list_all(&b).await.unwrap();
    assert!(b_parks.is_empty(), "user B must not see A's park");

    let err = state
        .dog_park_service
        .get_by_id(park.id, &b)
        .await
        .expect_err("user B must not read A's park");
    assert!(matches!(err, AppError::NotFound(_)));

    let err = state
        .dog_park_service
        .update(
            park.id,
            UpdateParkRequest {
                park_name: Some("hijacked".to_string()),
                park_description: None,
                park_address: None,
            },
            &b,
        )
        .await
        .expect_err("user B must not update A's park");
    assert!(matches!(err, AppError::NotFound(_)));

    let err = state
        .dog_park_service
        .delete(park.id, &b)
        .await
        .expect_err("user B must not delete A's park");
    assert!(matches!(err, AppError::NotFound(_)));

    let admin_parks = state
        .dog_park_service
        .list_all(&admin_scope())
        .await
        .unwrap();
    assert_eq!(admin_parks.len(), 1, "administrators see every park");
}

#[tokio::test]
async fn maps_follow_park_ownership() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let b = owner_scope(USER_B);

    let park = state
        .dog_park_service
        .create(create_park("A 园区"), &a)
        .await
        .unwrap();

    let map = state
        .dog_map_service
        .create(create_map("A 地图", park.id), &a)
        .await
        .expect("user A may add a map to their park");
    assert_eq!(map.user_id, USER_A);
    assert_eq!(map.park_id, park.id);

    let err = state
        .dog_map_service
        .create(create_map("B 地图", park.id), &b)
        .await
        .expect_err("user B must not add a map to A's park");
    assert!(matches!(err, AppError::BadRequest(_)));

    let b_maps = state.dog_map_service.list_all(&b).await.unwrap();
    assert!(b_maps.is_empty());

    let admin_maps = state
        .dog_map_service
        .list_all(&admin_scope())
        .await
        .unwrap();
    assert_eq!(admin_maps.len(), 1);
}

#[tokio::test]
async fn tasks_are_scoped_to_their_creator() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let b = owner_scope(USER_B);

    let task_a = state
        .dog_task_service
        .create(create_task("A 任务"), &a)
        .await
        .unwrap();
    let task_b = state
        .dog_task_service
        .create(create_task("B 任务"), &b)
        .await
        .unwrap();

    assert_eq!(task_a.user_id, Some(USER_A));
    assert_eq!(task_b.user_id, Some(USER_B));

    let page_query = || TaskPageQuery {
        task_name: None,
        task_status: None,
        device_id: None,
        user_id: None,
        current: 1,
        size: 10,
    };

    let a_page = state
        .dog_task_service
        .list_page(page_query(), &a)
        .await
        .unwrap();
    assert_eq!(a_page.records.len(), 1);
    assert_eq!(a_page.records[0].id, task_a.id);

    let b_page = state
        .dog_task_service
        .list_page(page_query(), &b)
        .await
        .unwrap();
    assert_eq!(b_page.records.len(), 1);
    assert_eq!(b_page.records[0].id, task_b.id);

    let admin_page = state
        .dog_task_service
        .list_page(page_query(), &admin_scope())
        .await
        .unwrap();
    assert_eq!(admin_page.records.len(), 2);

    let err = state
        .dog_task_service
        .get_by_id(task_a.id, &b)
        .await
        .expect_err("user B must not read A's task");
    assert!(matches!(err, AppError::NotFound(_)));

    let err = state
        .dog_task_service
        .delete(task_a.id, &b)
        .await
        .expect_err("user B must not delete A's task");
    assert!(matches!(err, AppError::NotFound(_)));

    state.dog_task_service.delete(task_a.id, &a).await.unwrap();
    let a_page = state
        .dog_task_service
        .list_page(page_query(), &a)
        .await
        .unwrap();
    assert!(a_page.records.is_empty());
}

#[tokio::test]
async fn admin_role_is_detected_from_the_seeded_role_code() {
    let state = test_state().await;

    assert!(!state.auth_service.is_admin(USER_A).await.unwrap());

    // Role 1 is the seeded `sysadm` role with code "110".
    state
        .role_service
        .assign_role(&USER_A, &1)
        .await
        .expect("assigning the admin role must work");
    assert!(state.auth_service.is_admin(USER_A).await.unwrap());

    let park = state
        .dog_park_service
        .create(create_park("A 园区"), &owner_scope(USER_A))
        .await
        .unwrap();
    let admin = DataScope::new(USER_A, state.auth_service.is_admin(USER_A).await.unwrap());
    let visible = state.dog_park_service.list_all(&admin).await.unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].id, park.id);
}

#[tokio::test]
async fn http_handlers_receive_the_authenticated_user() {
    use x_rust::api::routes::create_router;

    let token_store = Arc::new(FakeTokenStore::new());
    let app = App::new("sqlite::memory:").await.unwrap();
    let state = app.build_state_with_token_store(token_store.clone(), JWT_SECRET);
    let router = create_router(state);

    let token_a = token_for(USER_A, "user_a");
    token_store.insert_token(USER_A, &token_a);

    let request = Request::builder()
        .method("POST")
        .uri("/api/dogPark")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token_a))
        .body(Body::from(json!({ "parkName": "A 园区" }).to_string()))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["code"], 10200);
    assert_eq!(body["data"]["userId"], USER_A);

    let token_b = token_for(USER_B, "user_b");
    token_store.insert_token(USER_B, &token_b);

    let request = Request::builder()
        .method("GET")
        .uri("/api/dogPark/getPage?current=1&size=10")
        .header("authorization", format!("Bearer {}", token_b))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["data"]["total"], 0);
}
