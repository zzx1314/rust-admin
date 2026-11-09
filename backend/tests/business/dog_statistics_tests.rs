//! Dashboard statistics tests.
//!
//! The welcome page shows four summary cards, a weekly bar chart and a device
//! type bar chart. Everything must be derived from real rows: totals come from
//! the tables, the weekly series buckets `create_time` per weekday (Beijing
//! time, Monday first) and the percent badge is the week-over-week growth.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use x_rust::api::AppState;
use x_rust::app::App;
use x_rust::business::dog::device::domain::CreateDeviceRequest;
use x_rust::business::dog::model::domain::CreateModelRequest;
use x_rust::business::dog::route_library::domain::CreateRouteRequest;
use x_rust::business::dog::task::domain::CreateTaskRequest;
use x_rust::common::data_scope::DataScope;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};

const JWT_SECRET: &str = "test-secret";
const USER_A: i64 = 101;
const USER_B: i64 = 102;

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

fn admin_scope() -> DataScope {
    DataScope::new(1, true)
}

fn owner_scope(user_id: i64) -> DataScope {
    DataScope::new(user_id, false)
}

async fn create_device(state: &AppState, code: &str, device_type: Option<&str>) {
    state
        .dog_device_service
        .create(CreateDeviceRequest {
            device_id: code.to_string(),
            device_type: device_type.map(str::to_string),
            status: None,
            device_ip: None,
            model: None,
            speed: None,
            height: None,
            battery: None,
            course: None,
            location: None,
        })
        .await
        .expect("device must be created");
}

async fn create_model(state: &AppState, name: &str) -> i64 {
    state
        .dog_model_service
        .create(CreateModelRequest {
            model_name: name.to_string(),
            model_type: None,
            file_name: None,
            file_path: None,
            file_format: None,
            file_size: None,
            remarks: None,
        })
        .await
        .expect("model must be created")
        .id
}

async fn create_route(state: &AppState, name: &str, model_id: i64) {
    state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: name.to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: None,
            map_id: None,
            model_id,
            route_data: None,
        })
        .await
        .expect("route must be created");
}

async fn create_task(state: &AppState, name: &str, scope: &DataScope) {
    state
        .dog_task_service
        .create(
            CreateTaskRequest {
                task_name: name.to_string(),
                task_description: None,
                task_status: None,
                workflow_id: None,
                workflow_uuid: None,
                device_id: None,
                route_id: None,
            },
            scope,
        )
        .await
        .expect("task must be created");
}

#[tokio::test]
async fn empty_database_reports_zeroes_instead_of_placeholders() {
    let state = test_state().await;

    let stats = state
        .dog_statistics_service
        .get_all_statistics(&admin_scope())
        .await
        .unwrap();

    assert_eq!(stats.len(), 4);
    for summary in &stats {
        assert_eq!(summary.value, 0, "{} must start at zero", summary.name);
        assert_eq!(
            summary.percent, "0%",
            "{} has no history, growth must be 0%",
            summary.name
        );
        assert_eq!(summary.data.len(), 7, "each card shows a full week");
        assert!(summary.data.iter().all(|count| *count == 0));
    }

    let weekly = state
        .dog_statistics_service
        .get_weekly_statistics()
        .await
        .unwrap();
    assert_eq!(weekly.len(), 2, "previous week + current week");
    assert!(weekly[0].device_count.iter().all(|count| *count == 0));
    assert!(weekly[1].task_count.iter().all(|count| *count == 0));

    let device_types = state
        .dog_statistics_service
        .get_device_type_statistics(&admin_scope())
        .await
        .unwrap();
    assert_eq!(device_types["xdata"], serde_json::json!([]));
    assert_eq!(device_types["ydata"], serde_json::json!([]));
}

#[tokio::test]
async fn summary_cards_reflect_stored_rows_and_week_to_date_series() {
    let state = test_state().await;
    let scope = admin_scope();

    create_device(&state, "C4C6E66E998E", Some("X1")).await;
    let model_id = create_model(&state, "第一人称模型").await;
    create_route(&state, "测试路线", model_id).await;
    create_task(&state, "测试任务", &scope).await;

    let stats = state
        .dog_statistics_service
        .get_all_statistics(&scope)
        .await
        .unwrap();

    let value_of = |name: &str| {
        stats
            .iter()
            .find(|summary| summary.name == name)
            .unwrap_or_else(|| panic!("{name} card must exist"))
            .value
    };
    assert_eq!(value_of("设备数量"), 1);
    assert_eq!(value_of("模型数量"), 1);
    assert_eq!(value_of("路线数量"), 1);

    for summary in &stats {
        assert_eq!(summary.value, 1, "{} counts the stored row", summary.name);
        // Rows were just created, so exactly one weekday bucket is filled.
        assert!(
            summary.data.iter().any(|count| *count == 1),
            "{} must bucket today's row into the current week",
            summary.name
        );
        assert_eq!(summary.data.iter().sum::<i64>(), 1);
        assert_eq!(
            summary.percent, "+100%",
            "{} grew from zero to one this week",
            summary.name
        );
    }

    let weekly = state
        .dog_statistics_service
        .get_weekly_statistics()
        .await
        .unwrap();
    assert_eq!(weekly[0].device_count.len(), 7);
    assert_eq!(weekly[0].device_count.iter().sum::<i64>(), 0);
    for series in [
        &weekly[1].device_count,
        &weekly[1].model_count,
        &weekly[1].route_count,
        &weekly[1].task_count,
    ] {
        assert_eq!(series.len(), 7);
        assert_eq!(series.iter().sum::<i64>(), 1);
    }
    assert_eq!(weekly[1].device_count, stats[0].data);
}

#[tokio::test]
async fn device_type_chart_groups_devices_and_sorts_by_count() {
    let state = test_state().await;

    create_device(&state, "DEV-1", Some("X1")).await;
    create_device(&state, "DEV-2", Some("X1")).await;
    create_device(&state, "DEV-3", Some("X2")).await;
    create_device(&state, "DEV-4", None).await;

    let data = state
        .dog_statistics_service
        .get_device_type_statistics(&admin_scope())
        .await
        .unwrap();

    assert_eq!(
        data["xdata"],
        serde_json::json!(["X1", "X2", "未知"]),
        "largest group first, untyped devices grouped as 未知"
    );
    assert_eq!(data["ydata"], serde_json::json!([2, 1, 1]));
}

#[tokio::test]
async fn task_counts_respect_the_user_data_scope() {
    let state = test_state().await;

    create_task(&state, "A 的任务", &owner_scope(USER_A)).await;
    create_task(&state, "B 的任务", &owner_scope(USER_B)).await;

    let task_card = |stats: &[x_rust::business::dog::statistics::service::StatisticsSummary]| {
        stats
            .iter()
            .find(|summary| summary.name == "任务数量")
            .expect("task card must exist")
            .clone()
    };

    let a_stats = state
        .dog_statistics_service
        .get_all_statistics(&owner_scope(USER_A))
        .await
        .unwrap();
    let b_stats = state
        .dog_statistics_service
        .get_all_statistics(&owner_scope(USER_B))
        .await
        .unwrap();
    let admin_stats = state
        .dog_statistics_service
        .get_all_statistics(&admin_scope())
        .await
        .unwrap();

    assert_eq!(task_card(&a_stats).value, 1);
    assert_eq!(task_card(&b_stats).value, 1);
    assert_eq!(task_card(&admin_stats).value, 2);
}
