//! 地图归属园区：`dog_map.park_code` / `park_name` 落库、按园区编码过滤、
//! 派生新版本时继承园区。
//!
//! 园区台账在 `business/` 业务平台，中台只保存业务侧的园区编码与名称，
//! 供业务平台按园区反查地图（见 `DOG.md` §3.0）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use x_rust::api::AppState;
use x_rust::app::App;
use x_rust::business::dog::map::domain::{CreateMapRequest, MapPageQuery};
use x_rust::common::data_scope::DataScope;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};

const JWT_SECRET: &str = "test-secret";

#[derive(Default)]
struct MemoryTokenStore {
    tokens: Mutex<HashMap<String, String>>,
}

impl TokenStore for MemoryTokenStore {
    fn set_token(
        &self,
        user_id: &str,
        token: &str,
        _ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        self.tokens
            .lock()
            .unwrap()
            .insert(format!("auth:token:{user_id}"), token.to_string());
        Box::pin(async { Ok(()) })
    }

    fn get_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let value = self
            .tokens
            .lock()
            .unwrap()
            .get(&format!("auth:token:{user_id}"))
            .cloned();
        Box::pin(async move { Ok(value) })
    }

    fn delete_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        self.tokens
            .lock()
            .unwrap()
            .remove(&format!("auth:token:{user_id}"));
        Box::pin(async { Ok(()) })
    }

    fn set_refresh_token(
        &self,
        user_id: &str,
        token: &str,
        _ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        self.tokens
            .lock()
            .unwrap()
            .insert(format!("auth:refresh:{user_id}"), token.to_string());
        Box::pin(async { Ok(()) })
    }

    fn get_refresh_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let value = self
            .tokens
            .lock()
            .unwrap()
            .get(&format!("auth:refresh:{user_id}"))
            .cloned();
        Box::pin(async move { Ok(value) })
    }

    fn delete_refresh_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        self.tokens
            .lock()
            .unwrap()
            .remove(&format!("auth:refresh:{user_id}"));
        Box::pin(async { Ok(()) })
    }
}

async fn test_state() -> AppState {
    let app = App::new("sqlite::memory:")
        .await
        .expect("in-memory database must migrate");
    app.build_state_with_token_store(Arc::new(MemoryTokenStore::default()), JWT_SECRET)
}

fn map_request(name: &str, park_code: Option<&str>, park_name: Option<&str>) -> CreateMapRequest {
    CreateMapRequest {
        map_name: name.to_string(),
        map_type: None,
        park_code: park_code.map(str::to_string),
        park_name: park_name.map(str::to_string),
        map_data: None,
        file_name: None,
        file_path: None,
        file_format: None,
        file_size: None,
        remarks: None,
        map_version: Some("v1.0".to_string()),
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

fn page_query(park_code: Option<&str>) -> MapPageQuery {
    MapPageQuery {
        map_name: None,
        file_name: None,
        map_status: None,
        park_code: park_code.map(str::to_string),
        begin_time: None,
        end_time: None,
        current: 1,
        size: 10,
    }
}

#[tokio::test]
async fn park_columns_round_trip_and_filter_by_code() {
    let state = test_state().await;
    let scope = DataScope::new(1, true);

    let park_a = state
        .dog_map_service
        .create(
            map_request("园区A地图", Some("PK-001"), Some("园区A")),
            &scope,
        )
        .await
        .expect("map must be created");
    state
        .dog_map_service
        .create(
            map_request("园区B地图", Some("PK-002"), Some("园区B")),
            &scope,
        )
        .await
        .expect("map must be created");
    // 手工上传、未关联园区的地图
    state
        .dog_map_service
        .create(map_request("未关联地图", None, None), &scope)
        .await
        .expect("map must be created");

    let filtered = state
        .dog_map_service
        .list_page(page_query(Some("PK-001")), &scope)
        .await
        .expect("page query must succeed");
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.records[0].id, park_a.id);
    assert_eq!(filtered.records[0].park_code.as_deref(), Some("PK-001"));
    assert_eq!(filtered.records[0].park_name.as_deref(), Some("园区A"));

    let all = state
        .dog_map_service
        .list_page(page_query(None), &scope)
        .await
        .expect("page query must succeed");
    assert_eq!(all.total, 3);
}

#[tokio::test]
async fn park_code_is_trimmed_and_blank_becomes_null() {
    let state = test_state().await;
    let scope = DataScope::new(1, true);

    let map = state
        .dog_map_service
        .create(map_request("空白园区", Some("   "), Some("  ")), &scope)
        .await
        .expect("map must be created");
    assert_eq!(map.park_code, None);
    assert_eq!(map.park_name, None);

    let map = state
        .dog_map_service
        .create(
            map_request("带空格园区", Some("  PK-009  "), Some(" 园区九 ")),
            &scope,
        )
        .await
        .expect("map must be created");
    assert_eq!(map.park_code.as_deref(), Some("PK-009"));
    assert_eq!(map.park_name.as_deref(), Some("园区九"));
}

#[tokio::test]
async fn new_version_inherits_park() {
    let state = test_state().await;
    let scope = DataScope::new(1, true);

    let original = state
        .dog_map_service
        .create(
            map_request("园区A建图", Some("PK-001"), Some("园区A")),
            &scope,
        )
        .await
        .expect("map must be created");

    let derived = state
        .dog_map_service
        .new_version(original.id, &scope)
        .await
        .expect("new version must be created");
    assert_eq!(derived.park_code.as_deref(), Some("PK-001"));
    assert_eq!(derived.park_name.as_deref(), Some("园区A"));
}
