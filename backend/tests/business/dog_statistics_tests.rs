//! Dashboard statistics tests.
//!
//! The welcome page shows four summary cards (models / media / commands /
//! services) and a weekly bar chart. Everything must be derived from real
//! rows: totals come from the tables, the weekly series buckets `create_time`
//! per weekday (Beijing time, Monday first) and the percent badge is the
//! week-over-week growth.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use x_rust::api::AppState;
use x_rust::app::App;
use x_rust::business::dog::command::domain::CreateCommandRequest;
use x_rust::business::dog::media::domain::CreateMediaRequest;
use x_rust::business::dog::model::domain::CreateModelRequest;
use x_rust::business::dog::services::domain::CreateServicesRequest;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};

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

async fn create_model(state: &AppState, name: &str) {
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
        .expect("model must be created");
}

async fn create_media(state: &AppState, name: &str) {
    state
        .dog_media_service
        .create(CreateMediaRequest {
            media_name: name.to_string(),
            media_type: None,
            media_path: None,
            media_size: None,
            remarks: None,
        })
        .await
        .expect("media must be created");
}

async fn create_command(state: &AppState, name: &str) {
    state
        .dog_command_service
        .create(CreateCommandRequest {
            command_name: name.to_string(),
            command_type: None,
            command_params: None,
            device_id: None,
            status: None,
            return_value: None,
            task_id: None,
        })
        .await
        .expect("command must be created");
}

async fn create_services(state: &AppState) {
    state
        .dog_services_service
        .create(CreateServicesRequest {
            r#type: Some("巡检".to_string()),
            params: None,
        })
        .await
        .expect("services must be created");
}

#[tokio::test]
async fn empty_database_reports_zeroes_instead_of_placeholders() {
    let state = test_state().await;

    let stats = state
        .dog_statistics_service
        .get_all_statistics()
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
    assert!(weekly[0].model_count.iter().all(|count| *count == 0));
    assert!(weekly[1].command_count.iter().all(|count| *count == 0));
}

#[tokio::test]
async fn summary_cards_reflect_stored_rows_and_week_to_date_series() {
    let state = test_state().await;

    create_model(&state, "第一人称模型").await;
    create_media(&state, "巡检视频").await;
    create_command(&state, "前进").await;
    create_services(&state).await;

    let stats = state
        .dog_statistics_service
        .get_all_statistics()
        .await
        .unwrap();

    let value_of = |name: &str| {
        stats
            .iter()
            .find(|summary| summary.name == name)
            .unwrap_or_else(|| panic!("{name} card must exist"))
            .value
    };
    assert_eq!(value_of("模型数量"), 1);
    assert_eq!(value_of("媒体数量"), 1);
    assert_eq!(value_of("指令数量"), 1);
    assert_eq!(value_of("服务数量"), 1);

    for summary in &stats {
        assert_eq!(summary.value, 1, "{} counts the stored row", summary.name);
        // Rows were just created, so exactly one weekday bucket is filled.
        assert!(
            summary.data.contains(&1),
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
    for series in [
        &weekly[1].model_count,
        &weekly[1].media_count,
        &weekly[1].command_count,
        &weekly[1].services_count,
    ] {
        assert_eq!(series.len(), 7);
        assert_eq!(series.iter().sum::<i64>(), 1);
    }
    assert_eq!(weekly[1].model_count, stats[0].data);
}
