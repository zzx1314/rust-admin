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
use x_rust::business::dog::device::domain::CreateDeviceRequest;
use x_rust::business::dog::map::domain::{CreateMapRequest, UploadMapRequest};
use x_rust::business::dog::park::domain::{CreateParkRequest, UpdateParkRequest};
use x_rust::business::dog::task::domain::{CreateTaskRequest, TaskPageQuery};
use x_rust::business::dog::user_device::domain::CreateUserDeviceRequest;
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
        park_id: Some(park_id),
        file_name: None,
        file_path: None,
        file_format: None,
        file_size: None,
        remarks: None,
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

fn create_task_for_device(name: &str, device_id: i64) -> CreateTaskRequest {
    CreateTaskRequest {
        device_id: Some(device_id),
        ..create_task(name)
    }
}

fn create_device(code: &str) -> CreateDeviceRequest {
    CreateDeviceRequest {
        device_id: code.to_string(),
        device_type: None,
        status: None,
        device_ip: None,
        model: None,
        speed: None,
        height: None,
        battery: None,
        course: None,
        location: None,
    }
}

fn bind_request(device_id: i64) -> CreateUserDeviceRequest {
    CreateUserDeviceRequest {
        user_id: None,
        device_id,
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
    assert_eq!(map.park_id, Some(park.id));

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
async fn maps_can_be_uploaded_as_zip_packages() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let upload_root = std::env::temp_dir().join(format!(
        "dog_map_upload_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let map = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                id: None,
                map_name: "园区地图".to_string(),
                map_type: Some("离线地图".to_string()),
                park_id: None,
                remarks: Some("测试地图包".to_string()),
            },
            "park-map.zip",
            b"PK\x03\x04",
            &upload_root,
            &a,
        )
        .await
        .expect("zip package should be stored");

    assert_eq!(map.map_name, "园区地图");
    assert_eq!(map.file_name.as_deref(), Some("park-map.zip"));
    assert_eq!(map.file_format.as_deref(), Some("zip"));
    assert_eq!(map.file_size, Some(4));
    assert!(map.park_id.is_none());
    let stored = upload_root.join(map.file_path.clone().unwrap());
    assert!(stored.exists(), "uploaded package must be written to disk");

    let err = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                map_name: "错误格式".to_string(),
                ..Default::default()
            },
            "map.rar",
            b"x",
            &upload_root,
            &a,
        )
        .await
        .expect_err("only .zip packages are accepted");
    assert!(matches!(err, AppError::BadRequest(_)));

    std::fs::remove_dir_all(&upload_root).ok();
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
        begin_time: None,
        end_time: None,
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

#[tokio::test]
async fn devices_are_visible_only_to_bound_users() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let b = owner_scope(USER_B);

    let device = state
        .dog_device_service
        .create(create_device("dev-a"))
        .await
        .expect("device creation must work");

    assert_eq!(
        state
            .dog_device_service
            .list_all(&admin_scope())
            .await
            .unwrap()
            .len(),
        1,
        "administrators see unbound devices"
    );
    assert!(
        state
            .dog_device_service
            .list_all(&a)
            .await
            .unwrap()
            .is_empty(),
        "unbound devices are invisible to regular users"
    );

    let binding = state
        .dog_user_device_service
        .bind(bind_request(device.id), &a)
        .await
        .expect("user A claims the unbound device");
    assert_eq!(binding.user_id, USER_A);
    assert_eq!(binding.device_code.as_deref(), Some("dev-a"));

    assert_eq!(
        state.dog_device_service.list_all(&a).await.unwrap().len(),
        1
    );
    assert!(
        state
            .dog_device_service
            .list_all(&b)
            .await
            .unwrap()
            .is_empty()
    );

    let err = state
        .dog_user_device_service
        .bind(bind_request(device.id), &b)
        .await
        .expect_err("user B cannot claim an already bound device");
    assert!(matches!(err, AppError::Conflict(_)));

    let err = state
        .dog_device_service
        .get_by_id(device.id, &b)
        .await
        .expect_err("user B cannot read a device bound to someone else");
    assert!(matches!(err, AppError::NotFound(_)));

    state
        .dog_user_device_service
        .delete(binding.id, &a)
        .await
        .expect("user A may unbind");
    assert!(
        state
            .dog_device_service
            .list_all(&a)
            .await
            .unwrap()
            .is_empty()
    );

    let rebound = state
        .dog_user_device_service
        .bind(bind_request(device.id), &a)
        .await
        .expect("binding again restores the soft-deleted row");
    assert_eq!(rebound.id, binding.id);
    assert_eq!(
        state.dog_device_service.list_all(&a).await.unwrap().len(),
        1
    );

    // Administrators can bind any user; a device may be shared by several users.
    state
        .dog_user_device_service
        .bind(
            CreateUserDeviceRequest {
                user_id: Some(USER_B),
                device_id: device.id,
            },
            &admin_scope(),
        )
        .await
        .expect("administrator binds user B to the same device");
    assert_eq!(
        state.dog_device_service.list_all(&b).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn tasks_reject_devices_the_user_cannot_access() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let b = owner_scope(USER_B);

    let device = state
        .dog_device_service
        .create(create_device("dev-task"))
        .await
        .unwrap();
    state
        .dog_user_device_service
        .bind(bind_request(device.id), &a)
        .await
        .unwrap();

    let task = state
        .dog_task_service
        .create(create_task_for_device("A 任务", device.id), &a)
        .await
        .expect("user A may reference their own device");
    assert_eq!(task.device_id, Some(device.id));

    let err = state
        .dog_task_service
        .create(create_task_for_device("B 任务", device.id), &b)
        .await
        .expect_err("user B cannot reference a device they cannot access");
    assert!(matches!(err, AppError::BadRequest(_)));

    state
        .dog_task_service
        .create(
            create_task_for_device("admin 任务", device.id),
            &admin_scope(),
        )
        .await
        .expect("administrators may reference any device");
}

#[tokio::test]
async fn gateway_assign_interface_resolves_devices_by_hardware_id() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let b = owner_scope(USER_B);

    state
        .dog_device_service
        .create(create_device("dev-assign"))
        .await
        .unwrap();

    let err = state
        .dog_user_device_service
        .bind_by_device_code("missing-device", None, &admin_scope())
        .await
        .expect_err("unregistered devices cannot be assigned");
    assert!(matches!(err, AppError::BadRequest(_)));

    let binding = state
        .dog_user_device_service
        .bind_by_device_code("dev-assign", Some(USER_B), &admin_scope())
        .await
        .expect("administrators assign by hardware device id");
    assert_eq!(binding.user_id, USER_B);
    assert_eq!(binding.device_code.as_deref(), Some("dev-assign"));

    assert_eq!(
        state.dog_device_service.list_all(&b).await.unwrap().len(),
        1
    );
    assert!(
        state
            .dog_device_service
            .list_all(&a)
            .await
            .unwrap()
            .is_empty()
    );

    let err = state
        .dog_user_device_service
        .bind_by_device_code("dev-assign", None, &a)
        .await
        .expect_err("already assigned devices cannot be claimed");
    assert!(matches!(err, AppError::Conflict(_)));

    assert!(
        state
            .dog_user_device_service
            .list_by_device_code("dev-assign", &a)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        state
            .dog_user_device_service
            .list_by_device_code("dev-assign", &admin_scope())
            .await
            .unwrap()
            .len(),
        1
    );

    state
        .dog_user_device_service
        .unbind_by_device_code("dev-assign", Some(USER_B), &admin_scope())
        .await
        .expect("administrators unassign by hardware device id");
    assert!(
        state
            .dog_device_service
            .list_all(&b)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn http_map_upload_endpoint_stores_zip_package() {
    use sea_orm_migration::MigratorTrait;
    use x_rust::api::routes::create_router;
    use x_rust::migration::Migrator;

    let upload_root = std::env::temp_dir().join(format!(
        "dog_map_http_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let conn = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
    <Migrator as MigratorTrait>::up(&conn, None).await.unwrap();

    let token_store = Arc::new(FakeTokenStore::new());
    let app = App::from_connection_with_upload_dir(conn, upload_root.clone());
    let state = app.build_state_with_token_store(token_store.clone(), JWT_SECRET);
    let token = token_for(USER_A, "user_a");
    token_store.insert_token(USER_A, &token);

    let boundary = "----dog_map_test_boundary";
    let map_info = json!({
        "mapName": "园区地图",
        "mapType": "离线地图",
        "remarks": "测试地图包"
    })
    .to_string();
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"mapInfo\"\r\n\r\n{map_info}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"map.zip\"\r\nContent-Type: application/zip\r\n\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"PK\x03\x04");
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

    let request = Request::builder()
        .method("POST")
        .uri("/api/dogMap/upload")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(body))
        .unwrap();
    let response = create_router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["code"], 10200);
    assert_eq!(body["data"]["mapName"], "园区地图");
    assert_eq!(body["data"]["fileName"], "map.zip");
    assert_eq!(body["data"]["fileFormat"], "zip");
    assert_eq!(body["data"]["fileSize"], 4);
    assert!(
        body["data"]["filePath"]
            .as_str()
            .unwrap()
            .starts_with("maps/")
    );

    std::fs::remove_dir_all(&upload_root).ok();
}

#[tokio::test]
async fn http_gateway_assign_endpoint_works() {
    use x_rust::api::routes::create_router;

    let token_store = Arc::new(FakeTokenStore::new());
    let app = App::new("sqlite::memory:").await.unwrap();
    let state = app.build_state_with_token_store(token_store.clone(), JWT_SECRET);
    state
        .dog_device_service
        .create(create_device("dev-http"))
        .await
        .unwrap();
    state
        .role_service
        .assign_role(&USER_A, &1)
        .await
        .expect("make user A an administrator");

    let token = token_for(USER_A, "admin_a");
    token_store.insert_token(USER_A, &token);
    let router = create_router(state);

    let request = Request::builder()
        .method("POST")
        .uri("/api/dogGateway/assign")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::from(
            json!({ "deviceId": "dev-http", "userId": USER_B }).to_string(),
        ))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["code"], 10200);
    assert_eq!(body["data"]["userId"], USER_B);
    assert_eq!(body["data"]["deviceCode"], "dev-http");

    let request = Request::builder()
        .method("GET")
        .uri("/api/dogGateway/assign/dev-http")
        .header("authorization", format!("Bearer {}", token))
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
}
