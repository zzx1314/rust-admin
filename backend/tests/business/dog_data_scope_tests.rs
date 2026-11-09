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
use x_rust::business::dog::map::domain::{CreateMapRequest, MapPageQuery, UploadMapRequest};
use x_rust::business::dog::park::domain::{CreateParkRequest, ParkPageQuery, UpdateParkRequest};
use x_rust::business::dog::route_library::domain::{CreateRouteRequest, RoutePageQuery};
use x_rust::business::dog::task::domain::{CreateTaskRequest, TaskPageQuery};
use x_rust::business::dog::user_device::domain::CreateUserDeviceRequest;
use x_rust::business::dog::workflow::domain::CreateWorkflowRequest;
use x_rust::common::data_scope::DataScope;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};
use x_rust::system::sys_user::domain::CreateUserRequest;

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
        map_version: None,
        frame_id: None,
        crs: None,
        map_origin: None,
        resolution: None,
        floors: None,
        road_count: None,
        nogo_count: None,
        map_status: None,
        map_hash: None,
        map_width: None,
        map_height: None,
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
async fn park_list_shows_owner_username() {
    let state = test_state().await;
    let owner = state
        .user_service
        .create_user(CreateUserRequest {
            username: "park_owner".to_string(),
            phone: None,
            email: None,
            real_name: None,
            password: None,
            org_id: 1,
            remarks: None,
            card: None,
            sex: None,
            role: None,
        })
        .await
        .expect("test user must be created");

    let scope = owner_scope(owner.id);
    state
        .dog_park_service
        .create(create_park("A 园区"), &scope)
        .await
        .expect("owner may create a park");

    let page = state
        .dog_park_service
        .list_page(
            ParkPageQuery {
                park_name: None,
                begin_time: None,
                end_time: None,
                current: 1,
                size: 10,
            },
            &admin_scope(),
        )
        .await
        .expect("administrator may list parks");

    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].user_id, owner.id);
    assert_eq!(
        page.records[0].user_name.as_deref(),
        Some("park_owner"),
        "park list must resolve the owner user name"
    );
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
async fn maps_require_a_park_and_report_its_name() {
    let state = test_state().await;
    let a = owner_scope(USER_A);

    let err = state
        .dog_map_service
        .create(
            CreateMapRequest {
                park_id: None,
                ..create_map("无园区地图", 0)
            },
            &a,
        )
        .await
        .expect_err("creating a map without a park must fail");
    assert!(matches!(err, AppError::BadRequest(_)));

    let park = state
        .dog_park_service
        .create(create_park("A 园区"), &a)
        .await
        .expect("user A may create a park");
    state
        .dog_map_service
        .create(create_map("A 地图", park.id), &a)
        .await
        .expect("user A may add a map to their park");

    let page = state
        .dog_map_service
        .list_page(
            MapPageQuery {
                map_name: None,
                file_name: None,
                park_id: Some(park.id),
                map_status: None,
                publishable_only: None,
                begin_time: None,
                end_time: None,
                current: 1,
                size: 10,
            },
            &a,
        )
        .await
        .expect("map list must load");

    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].park_id, Some(park.id));
    assert_eq!(
        page.records[0].park_name.as_deref(),
        Some("A 园区"),
        "map list must resolve the park name"
    );
}

#[tokio::test]
async fn maps_can_be_uploaded_as_zip_packages() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let park = state
        .dog_park_service
        .create(create_park("A 园区"), &a)
        .await
        .expect("user A may create a park");
    let upload_root = std::env::temp_dir().join(format!(
        "dog_map_upload_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let package = sample_map_zip();
    let map = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                id: None,
                map_name: "园区地图".to_string(),
                map_type: Some("离线地图".to_string()),
                park_id: Some(park.id),
                remarks: Some("测试地图包".to_string()),
                ..Default::default()
            },
            "park-map.zip",
            &package,
            &upload_root,
            &a,
        )
        .await
        .expect("zip package should be stored");

    assert_eq!(map.map_name, "园区地图");
    assert_eq!(map.file_name.as_deref(), Some("park-map.zip"));
    assert_eq!(map.file_format.as_deref(), Some("zip"));
    assert_eq!(map.file_size, Some(package.len() as i64));
    assert_eq!(map.park_id, Some(park.id));
    assert_eq!(map.map_width, Some(4));
    assert_eq!(map.map_height, Some(3));
    assert_eq!(map.resolution.as_deref(), Some("0.05"));
    assert_eq!(map.map_origin.as_deref(), Some("-8.7, -17.7, 0.0"));
    assert_eq!(map.map_hash.as_deref().map(str::len), Some(64));
    let stored = upload_root.join(map.file_path.clone().unwrap());
    assert!(stored.exists(), "uploaded package must be written to disk");

    // 表单分辨率与地图包冲突时必须拒绝，避免手填值与真实栅格不一致。
    let err = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                map_name: "冲突地图".to_string(),
                park_id: Some(park.id),
                resolution: Some("0.10".to_string()),
                ..Default::default()
            },
            "conflict.zip",
            &package,
            &upload_root,
            &a,
        )
        .await
        .expect_err("conflicting resolution must be rejected");
    assert!(matches!(err, AppError::BadRequest(_)));

    // 新增地图必须选择园区
    let err = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                map_name: "缺园区".to_string(),
                ..Default::default()
            },
            "no-park.zip",
            b"PK\x03\x04",
            &upload_root,
            &a,
        )
        .await
        .expect_err("a park must be selected when creating a map");
    assert!(matches!(err, AppError::BadRequest(_)));

    let err = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                map_name: "错误格式".to_string(),
                park_id: Some(park.id),
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
async fn routes_can_be_linked_to_a_park() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let park = state
        .dog_park_service
        .create(create_park("A 园区"), &a)
        .await
        .expect("user A may create a park");

    // 园区必须先发布地图基线，路线才能冻结坐标系。
    let map = state
        .dog_map_service
        .create(create_map("A 地图", park.id), &a)
        .await
        .expect("user A may add a map to their park");
    state
        .dog_map_service
        .publish(map.id, &a)
        .await
        .expect("published baseline must bind to the park");

    let route = state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: "园区路线".to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: Some(park.id),
            map_id: None,
            model_id: 1,
            route_data: None,
        })
        .await
        .expect("route with park must be created");
    assert_eq!(route.park_id, Some(park.id));

    let page = state
        .dog_route_library_service
        .list_page(RoutePageQuery {
            route_name: None,
            route_type: None,
            route_status: None,
            park_id: Some(park.id),
            begin_time: None,
            end_time: None,
            current: 1,
            size: 10,
        })
        .await
        .expect("route list must load");
    assert_eq!(page.records.len(), 1);
    assert_eq!(page.records[0].park_id, Some(park.id));
    assert_eq!(
        page.records[0].park_name.as_deref(),
        Some("A 园区"),
        "route list must resolve the park name"
    );

    let err = state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: "非法园区路线".to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: Some(999_999),
            map_id: None,
            model_id: 1,
            route_data: None,
        })
        .await
        .expect_err("unknown park must be rejected");
    assert!(matches!(err, AppError::BadRequest(_)));
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
        route_name: None,
        device_id_string: None,
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

    let park = state
        .dog_park_service
        .create(create_park("A 园区"), &owner_scope(USER_A))
        .await
        .expect("user A may create a park");

    let boundary = "----dog_map_test_boundary";
    let map_info = json!({
        "mapName": "园区地图",
        "mapType": "离线地图",
        "parkId": park.id,
        "remarks": "测试地图包"
    })
    .to_string();
    let package = sample_map_zip();
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
    body.extend_from_slice(&package);
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
    assert_eq!(body["data"]["fileSize"], package.len() as i64);
    assert_eq!(body["data"]["mapWidth"], 4);
    assert_eq!(body["data"]["mapHeight"], 3);
    assert_eq!(body["data"]["resolution"], "0.05");
    assert_eq!(body["data"]["mapHash"].as_str().map(str::len), Some(64));
    assert_eq!(body["data"]["parkId"], park.id);
    assert_eq!(body["data"]["parkName"], "A 园区");
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

/// 构造一个最小地图包：ROS map_server 的 `model.pgm`（4x3 栅格）+ `model.yaml`
fn sample_map_zip() -> Vec<u8> {
    use std::io::Write;

    let mut pgm = b"P5\n4 3\n255\n".to_vec();
    pgm.extend((0..12u8).map(|value| value.saturating_mul(20)));
    let yaml = b"image: model.pgm\nresolution: 0.05\norigin: [-8.7, -17.7, 0.0]\nnegate: 0\n";

    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options: zip::write::SimpleFileOptions = Default::default();
        writer.start_file("model.pgm", options).unwrap();
        writer.write_all(&pgm).unwrap();
        writer.start_file("model.yaml", options).unwrap();
        writer.write_all(yaml).unwrap();
        writer.finish().unwrap();
    }
    buffer
}

fn temp_upload_root(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{}_{}_{}",
        prefix,
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[tokio::test]
async fn map_packages_are_rendered_to_png_with_grid_metadata() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let upload_root = temp_upload_root("dog_map_image");
    let park = state
        .dog_park_service
        .create(create_park("园区"), &a)
        .await
        .unwrap();

    let zip_bytes = sample_map_zip();
    let map = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                id: None,
                map_name: "园区地图".to_string(),
                map_type: Some("离线地图".to_string()),
                park_id: Some(park.id),
                remarks: None,
                ..Default::default()
            },
            "park-map.zip",
            &zip_bytes,
            &upload_root,
            &a,
        )
        .await
        .expect("map package should be stored");

    let image = state
        .dog_map_service
        .load_image(map.id, &upload_root, &a)
        .await
        .expect("uploaded package must render");

    assert!(image.image.starts_with("data:image/png;base64,"));
    assert_eq!((image.width, image.height), (4, 3));
    assert!((image.resolution - 0.05).abs() < f64::EPSILON);
    assert_eq!(image.origin, vec![-8.7, -17.7, 0.0]);

    // 未上传地图包的地图要给出明确的错误，而不是空图
    let empty = state
        .dog_map_service
        .create(create_map("无地图包", park.id), &a)
        .await
        .unwrap();
    let error = state
        .dog_map_service
        .load_image(empty.id, &upload_root, &a)
        .await
        .expect_err("map without package must fail");
    assert!(matches!(error, AppError::BadRequest(_)));

    std::fs::remove_dir_all(&upload_root).ok();
}

#[tokio::test]
async fn map_image_endpoint_serves_png_for_route_planning() {
    use sea_orm_migration::MigratorTrait;
    use x_rust::api::routes::create_router;
    use x_rust::migration::Migrator;

    let upload_root = temp_upload_root("dog_map_image_http");
    let conn = sea_orm::Database::connect("sqlite::memory:").await.unwrap();
    <Migrator as MigratorTrait>::up(&conn, None).await.unwrap();

    let app = App::from_connection_with_upload_dir(conn, upload_root.clone());
    let token_store = Arc::new(FakeTokenStore::new());
    let state = app.build_state_with_token_store(token_store.clone(), JWT_SECRET);

    let token = token_for(USER_A, "user_a");
    token_store.insert_token(USER_A, &token);

    let park = state
        .dog_park_service
        .create(create_park("A 园区"), &owner_scope(USER_A))
        .await
        .expect("user A may create a park");

    let map = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                id: None,
                map_name: "园区地图".to_string(),
                map_type: Some("离线地图".to_string()),
                park_id: Some(park.id),
                remarks: None,
                ..Default::default()
            },
            "park-map.zip",
            &sample_map_zip(),
            &upload_root,
            &owner_scope(USER_A),
        )
        .await
        .expect("map package should be stored");

    let request = Request::builder()
        .method("GET")
        .uri(format!("/api/dogMap/{}/image", map.id))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = create_router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = body_json(response).await;
    assert_eq!(body["code"], 10200);
    assert!(
        body["data"]["image"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert_eq!(body["data"]["width"], 4);
    assert_eq!(body["data"]["height"], 3);
    assert_eq!(body["data"]["origin"][0], -8.7);

    std::fs::remove_dir_all(&upload_root).ok();
}

/// 地图基线：发布后归档同园区旧版本、把园区指针切到新版本；
/// 已创建路线仍冻结在旧版本，不会被自动迁移。
#[tokio::test]
async fn map_baselines_are_immutable_and_frozen_on_routes() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let park = state
        .dog_park_service
        .create(create_park("基线园区"), &a)
        .await
        .expect("park must be created");
    assert_eq!(park.map_status, "未绑定");

    let v1 = state
        .dog_map_service
        .create(
            CreateMapRequest {
                map_version: Some("v1.0".to_string()),
                frame_id: Some("zj_map".to_string()),
                crs: Some("GCJ-02 + ENU".to_string()),
                resolution: Some("0.05".to_string()),
                ..create_map("一期地图", park.id)
            },
            &a,
        )
        .await
        .expect("draft baseline must be created");
    assert_eq!(v1.map_status, "草稿");

    let park_after_draft = state
        .dog_park_service
        .get_by_id(park.id, &a)
        .await
        .expect("park must reload");
    assert_eq!(park_after_draft.map_status, "待标定");

    // 草稿版本不能被路线引用。
    let err = state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: "草稿基线路线".to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: Some(park.id),
            map_id: Some(v1.id),
            model_id: v1.id,
            route_data: None,
        })
        .await
        .expect_err("draft baseline must not be used by routes");
    assert!(matches!(err, AppError::BadRequest(_)));

    // 未绑定基线的园区也不能创建路线。
    let err = state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: "无基线路线".to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: Some(park.id),
            map_id: None,
            model_id: 1,
            route_data: None,
        })
        .await
        .expect_err("park without baseline must reject routes");
    assert!(matches!(err, AppError::BadRequest(_)));

    let published_v1 = state
        .dog_map_service
        .publish(v1.id, &a)
        .await
        .expect("baseline must publish");
    assert_eq!(published_v1.map_status, "已发布");
    assert!(published_v1.publish_time.is_some());

    let park = state
        .dog_park_service
        .get_by_id(park.id, &a)
        .await
        .expect("park must reload");
    assert_eq!(park.map_id, Some(v1.id));
    assert_eq!(park.map_status, "已标定");

    // 路线冻结 v1.0 的版本与坐标系。
    let route = state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: "一期巡检".to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: Some(park.id),
            map_id: None,
            model_id: v1.id,
            route_data: Some("[[0,0],[10,5]]".to_string()),
        })
        .await
        .expect("route must freeze the current baseline");
    assert_eq!(route.map_id, Some(v1.id));
    assert_eq!(route.map_version.as_deref(), Some("v1.0"));
    assert_eq!(route.map_frame.as_deref(), Some("zj_map"));

    // 派生 v1.1 并发布：旧版本归档，园区指针切换，路线仍指向 v1.0。
    let v2 = state
        .dog_map_service
        .new_version(v1.id, &a)
        .await
        .expect("new version must be published");
    assert_eq!(v2.map_version.as_deref(), Some("v1.1"));
    assert_eq!(v2.map_status, "已发布");

    let archived = state
        .dog_map_service
        .get_by_id(v1.id, &a)
        .await
        .expect("old version must stay");
    assert_eq!(archived.map_status, "已归档");

    let park = state
        .dog_park_service
        .get_by_id(park.id, &a)
        .await
        .expect("park must reload");
    assert_eq!(park.map_id, Some(v2.id));

    let route = state
        .dog_route_library_service
        .get_by_id(route.id)
        .await
        .expect("route must stay on the frozen version");
    assert_eq!(route.map_id, Some(v1.id));
    assert_eq!(route.map_version.as_deref(), Some("v1.0"));

    let refs = state
        .dog_map_service
        .refs(v1.id, &a)
        .await
        .expect("refs must load");
    assert_eq!(refs.routes.len(), 1);
    assert_eq!(refs.routes[0].waypoint_count, 2);

    // 仍被路线引用的版本不可删除。
    let err = state
        .dog_map_service
        .delete(v1.id, &a)
        .await
        .expect_err("referenced baseline must not be deleted");
    assert!(matches!(err, AppError::BadRequest(_)));
}

/// 任务下发：路线冻结的地图基线快照必须进入指令参数，供设备校验 mapHash。
#[tokio::test]
async fn task_dispatch_carries_frozen_map_baseline() {
    let state = test_state().await;
    let a = owner_scope(USER_A);
    let admin = DataScope::new(USER_A, true);
    let upload_root = temp_upload_root("dog_map_dispatch");
    let park = state
        .dog_park_service
        .create(create_park("下发园区"), &a)
        .await
        .expect("park must be created");
    let map = state
        .dog_map_service
        .save_upload(
            UploadMapRequest {
                map_name: "下发地图".to_string(),
                park_id: Some(park.id),
                map_version: Some("v1.0".to_string()),
                frame_id: Some("map".to_string()),
                ..Default::default()
            },
            "dispatch.zip",
            &sample_map_zip(),
            &upload_root,
            &a,
        )
        .await
        .expect("map package must be stored");
    let map_hash = map.map_hash.clone().expect("uploaded map must have a hash");
    state
        .dog_map_service
        .publish(map.id, &a)
        .await
        .expect("map must publish");

    let route = state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: "下发路线".to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: Some(park.id),
            map_id: None,
            model_id: map.id,
            route_data: Some("[[0,0],[10,5]]".to_string()),
        })
        .await
        .expect("route must freeze the map");

    let device = state
        .dog_device_service
        .create(create_device("DOG-MAP-1"))
        .await
        .expect("device must be created");

    state
        .dog_workflow_service
        .create(CreateWorkflowRequest {
            uuid: "wf-map-dispatch".to_string(),
            nodes: None,
            edges: None,
            task_id: None,
            command_json_string: Some(r#"{"mission":"patrol"}"#.to_string()),
        })
        .await
        .expect("workflow must be created");

    let task = state
        .dog_task_service
        .create(
            CreateTaskRequest {
                task_name: "地图校验任务".to_string(),
                task_description: None,
                task_status: None,
                workflow_id: None,
                workflow_uuid: Some("wf-map-dispatch".to_string()),
                device_id: Some(device.id),
                route_id: Some(route.id),
            },
            &admin,
        )
        .await
        .expect("task must be created");

    state
        .dog_task_service
        .start_task(task.id, &admin)
        .await
        .expect("task must start");

    let commands = state
        .dog_command_service
        .list_all()
        .await
        .expect("commands must load");
    let command = commands
        .into_iter()
        .find(|command| command.task_id == Some(task.id))
        .expect("task command must exist");
    let params: Value = serde_json::from_str(command.command_params.as_deref().unwrap_or_default())
        .expect("command params must be JSON");
    assert_eq!(params["mission"], "patrol");
    assert_eq!(params["map"]["mapId"], map.id);
    assert_eq!(params["map"]["mapVersion"], "v1.0");
    assert_eq!(params["map"]["mapHash"], map_hash);
    assert_eq!(params["map"]["frameId"], "map");
    assert_eq!(params["map"]["width"], 4);
    assert_eq!(params["map"]["height"], 3);

    // 没有内容哈希的基线（例如手工创建的旧地图）禁止下发。
    let legacy = state
        .dog_map_service
        .create(create_map("无哈希地图", park.id), &a)
        .await
        .expect("legacy map must be created");
    state
        .dog_map_service
        .publish(legacy.id, &a)
        .await
        .expect("legacy map must publish");
    let legacy_route = state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: "无哈希路线".to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: Some(park.id),
            map_id: Some(legacy.id),
            model_id: legacy.id,
            route_data: None,
        })
        .await
        .expect("legacy route must be created");
    state
        .dog_workflow_service
        .create(CreateWorkflowRequest {
            uuid: "wf-map-legacy".to_string(),
            nodes: None,
            edges: None,
            task_id: None,
            command_json_string: Some(r#"{"mission":"patrol"}"#.to_string()),
        })
        .await
        .expect("workflow must be created");
    let legacy_task = state
        .dog_task_service
        .create(
            CreateTaskRequest {
                task_name: "无哈希任务".to_string(),
                task_description: None,
                task_status: None,
                workflow_id: None,
                workflow_uuid: Some("wf-map-legacy".to_string()),
                device_id: Some(device.id),
                route_id: Some(legacy_route.id),
            },
            &admin,
        )
        .await
        .expect("legacy task must be created");
    let err = state
        .dog_task_service
        .start_task(legacy_task.id, &admin)
        .await
        .expect_err("task without map hash must be rejected");
    assert!(matches!(err, AppError::BadRequest(_)));

    std::fs::remove_dir_all(&upload_root).ok();
}
