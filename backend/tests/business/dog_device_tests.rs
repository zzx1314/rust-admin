//! 设备台账与用户分配：创建、按分配关系过滤、批量分配 / 解除分配。

use std::sync::Arc;

use x_rust::app::App;
use x_rust::business::dog::device::domain::{
    AssignUsersRequest, CreateDeviceRequest, DeviceFilter, DevicePageQuery, UnassignUsersRequest,
};
use x_rust::common::data_scope::DataScope;
use x_rust::system::auth::repository::RedisTokenStore;

async fn test_state() -> x_rust::api::AppState {
    let app = App::new("sqlite::memory:")
        .await
        .expect("in-memory database must migrate");
    app.build_state_with_token_store(
        Arc::new(RedisTokenStore::new("redis://127.0.0.1:6379")),
        "test-secret",
    )
}

fn device_request(code: &str) -> CreateDeviceRequest {
    CreateDeviceRequest {
        device_id: code.to_string(),
        device_name: Some(format!("{code} 号机器狗")),
        model: Some("X30".to_string()),
        status: Some("在线待命".to_string()),
        firmware: None,
        device_ip: None,
        park_code: Some("PK-001".to_string()),
        park_name: Some("园区A".to_string()),
        remarks: None,
        user_ids: None,
    }
}

fn page_query() -> DevicePageQuery {
    DevicePageQuery {
        device_id: None,
        device_name: None,
        status: None,
        park_code: None,
        user_id: None,
        current: 1,
        size: 10,
    }
}

#[tokio::test]
async fn devices_are_filtered_by_assignment() {
    let state = test_state().await;
    let admin = DataScope::new(1, true);
    let user_id = 9001;
    let user_scope = DataScope::new(user_id, false);

    let first = state
        .dog_device_service
        .create(device_request("RD-001"), &admin)
        .await
        .expect("device must be created");
    let second = state
        .dog_device_service
        .create(device_request("RD-002"), &admin)
        .await
        .expect("device must be created");

    // 未分配：普通用户看不到任何设备
    let page = state
        .dog_device_service
        .list_page(page_query(), &user_scope)
        .await
        .expect("list must work");
    assert_eq!(page.total, 0);

    // 只分配第一台
    state
        .dog_device_service
        .assign_users(AssignUsersRequest {
            device_ids: vec![first.id],
            user_ids: vec![user_id],
            mode: Some("replace".to_string()),
        })
        .await
        .expect("assign must work");

    let page = state
        .dog_device_service
        .list_page(page_query(), &user_scope)
        .await
        .expect("list must work");
    assert_eq!(page.total, 1);
    assert_eq!(page.records[0].device_id, "RD-001");

    // 第二台对普通用户不可见
    assert!(
        state
            .dog_device_service
            .get(second.id, &user_scope)
            .await
            .is_err()
    );

    // 解除分配后回到不可见
    state
        .dog_device_service
        .unassign_users(UnassignUsersRequest {
            device_ids: vec![first.id],
            user_ids: vec![user_id],
        })
        .await
        .expect("unassign must work");
    let page = state
        .dog_device_service
        .list_page(page_query(), &user_scope)
        .await
        .expect("list must work");
    assert_eq!(page.total, 0);
}

#[tokio::test]
async fn upsert_is_idempotent_by_device_code() {
    let state = test_state().await;
    let admin = DataScope::new(1, true);

    let created = state
        .dog_device_service
        .upsert(device_request("RD-100"), &admin)
        .await
        .expect("first upsert must create");
    let mut changed = device_request("RD-100");
    changed.status = Some("执行任务".to_string());
    let updated = state
        .dog_device_service
        .upsert(changed, &admin)
        .await
        .expect("second upsert must update");

    assert_eq!(created.id, updated.id);
    assert_eq!(updated.status, "执行任务");
    assert_eq!(
        state
            .dog_device_service
            .list_all(DeviceFilter::default(), &admin)
            .await
            .expect("list must work")
            .len(),
        1
    );
}
