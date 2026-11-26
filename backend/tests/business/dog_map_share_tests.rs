//! 地图分享：一张中台地图共享给其他设备使用，不用重新建图。
//!
//! 分享关系落在 `dog_map_share`（多对多）。分享后：分配给目标设备的中台用户
//! 也能看到这张图（只读），「设备编号」筛选同时匹配上报设备与分享设备；
//! 写操作仍然只认地图归属人，分享不会把别人的地图变成可编辑的。

use std::sync::Arc;

use x_rust::app::App;
use x_rust::business::dog::device::domain::{AssignUsersRequest, CreateDeviceRequest};
use x_rust::business::dog::map::domain::{
    CreateMapRequest, MapPageQuery, ShareMapRequest, UpdateMapRequest,
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

fn map_request(name: &str, device_code: Option<&str>) -> CreateMapRequest {
    CreateMapRequest {
        map_name: name.to_string(),
        map_type: None,
        park_code: None,
        park_name: None,
        device_code: device_code.map(str::to_string),
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

fn page_query(device_code: Option<&str>) -> MapPageQuery {
    MapPageQuery {
        map_name: None,
        file_name: None,
        map_status: None,
        park_code: None,
        device_code: device_code.map(str::to_string),
        begin_time: None,
        end_time: None,
        current: 1,
        size: 10,
    }
}

fn rename(name: &str) -> UpdateMapRequest {
    UpdateMapRequest {
        map_name: Some(name.to_string()),
        map_type: None,
        park_code: None,
        park_name: None,
        device_code: None,
        map_data: None,
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

fn share_request(codes: &[&str], mode: Option<&str>) -> ShareMapRequest {
    ShareMapRequest {
        device_codes: codes.iter().map(|code| code.to_string()).collect(),
        mode: mode.map(str::to_string),
    }
}

#[tokio::test]
async fn shared_map_is_visible_to_the_target_device_user_and_filterable() {
    let state = test_state().await;
    let admin = DataScope::new(1, true);
    let user_id = 9001;
    let user = DataScope::new(user_id, false);

    let source = state
        .dog_device_service
        .create(device_request("RD-001"), &admin)
        .await
        .expect("source device must be created");
    let target = state
        .dog_device_service
        .create(device_request("RD-002"), &admin)
        .await
        .expect("target device must be created");
    let map = state
        .dog_map_service
        .create(map_request("园区地图", Some("RD-001")), &admin)
        .await
        .expect("map must be created");
    assert_ne!(source.id, target.id);

    // 未分享也没分配：普通用户看不到这张图
    assert!(
        state
            .dog_map_service
            .get_by_id(map.id, &user)
            .await
            .is_err()
    );
    assert_eq!(
        state
            .dog_map_service
            .list_page(page_query(None), &user)
            .await
            .expect("list must work")
            .total,
        0
    );

    // 把目标设备分配给用户，再把地图分享给这台设备
    state
        .dog_device_service
        .assign_users(AssignUsersRequest {
            device_ids: vec![target.id],
            user_ids: vec![user_id],
            mode: Some("replace".to_string()),
        })
        .await
        .expect("assign must work");
    let result = state
        .dog_map_service
        .share(map.id, share_request(&["RD-002"], None), &admin)
        .await
        .expect("share must work");
    assert_eq!(result.shared_device_codes, vec!["RD-002".to_string()]);
    assert_eq!(result.added, 1);
    assert_eq!(result.removed, 0);

    // 分享后：目标设备的用户可以读到（不用重新建图）
    let visible = state
        .dog_map_service
        .get_by_id(map.id, &user)
        .await
        .expect("shared map must be readable");
    assert_eq!(visible.id, map.id);
    assert_eq!(
        state
            .dog_map_service
            .list_page(page_query(None), &user)
            .await
            .expect("list must work")
            .total,
        1
    );

    // 「设备编号」筛选同时匹配分享设备
    let filtered = state
        .dog_map_service
        .list_page(page_query(Some("RD-002")), &user)
        .await
        .expect("filter must work");
    assert_eq!(filtered.total, 1);
    assert_eq!(filtered.records[0].id, map.id);
    assert_eq!(
        filtered.records[0].shared_device_codes,
        vec!["RD-002".to_string()]
    );

    // 但写操作仍然只认归属人：分享不是把地图交给别人编辑
    assert!(
        state
            .dog_map_service
            .update(map.id, rename("别人改名"), &user)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn every_user_assigned_to_the_same_dog_sees_its_maps() {
    let state = test_state().await;
    let admin = DataScope::new(1, true);
    // 一台狗绑定多个人：地图回传时只记一个归属人（业务平台取第一个用户），
    // 后绑定的人也必须能看到狗体里的地图。
    let owner = 9101;
    let co_user = 9102;
    let stranger = 9103;

    let device = state
        .dog_device_service
        .create(device_request("RD-100"), &admin)
        .await
        .expect("device must be created");
    state
        .dog_device_service
        .assign_users(AssignUsersRequest {
            device_ids: vec![device.id],
            user_ids: vec![owner, co_user],
            mode: Some("replace".to_string()),
        })
        .await
        .expect("assign must work");

    let map = state
        .dog_map_service
        .create(
            map_request("狗体地图", Some("RD-100")),
            &DataScope::new(owner, false),
        )
        .await
        .expect("map must be created");
    assert_eq!(map.user_id, owner);

    // 归属人：一直可见
    assert_eq!(
        state
            .dog_map_service
            .list_page(page_query(None), &DataScope::new(owner, false))
            .await
            .expect("list must work")
            .total,
        1
    );

    // 后绑定的用户：未分享也应该能看到这张图
    let co_scope = DataScope::new(co_user, false);
    let visible = state
        .dog_map_service
        .get_by_id(map.id, &co_scope)
        .await
        .expect("co-assigned user must read the dog map");
    assert_eq!(visible.id, map.id);
    let listed = state
        .dog_map_service
        .list_page(page_query(None), &co_scope)
        .await
        .expect("list must work");
    assert_eq!(listed.total, 1);
    assert_eq!(listed.records[0].id, map.id);

    // 仍然不是可编辑的：写操作只认归属人
    assert!(
        state
            .dog_map_service
            .update(map.id, rename("别人改名"), &co_scope)
            .await
            .is_err()
    );

    // 没绑这台狗的人照旧看不到
    let stranger_scope = DataScope::new(stranger, false);
    assert!(
        state
            .dog_map_service
            .get_by_id(map.id, &stranger_scope)
            .await
            .is_err()
    );
    assert_eq!(
        state
            .dog_map_service
            .list_page(page_query(None), &stranger_scope)
            .await
            .expect("list must work")
            .total,
        0
    );
}

#[tokio::test]
async fn share_is_idempotent_and_replace_clears_it() {
    let state = test_state().await;
    let admin = DataScope::new(1, true);

    let map = state
        .dog_map_service
        .create(map_request("园区地图", Some("RD-001")), &admin)
        .await
        .expect("map must be created");

    // 上报设备不重复计入分享
    let result = state
        .dog_map_service
        .share(map.id, share_request(&["RD-001", "RD-002"], None), &admin)
        .await
        .expect("share must work");
    assert_eq!(result.shared_device_codes, vec!["RD-002".to_string()]);

    // 重复分享同一台设备不会新增记录
    let again = state
        .dog_map_service
        .share(map.id, share_request(&["RD-002"], Some("append")), &admin)
        .await
        .expect("share must work");
    assert_eq!(again.added, 0);
    assert_eq!(again.shared_device_codes, vec!["RD-002".to_string()]);

    // append 追加另一台
    let appended = state
        .dog_map_service
        .share(map.id, share_request(&["RD-003"], Some("append")), &admin)
        .await
        .expect("share must work");
    assert_eq!(
        appended.shared_device_codes,
        vec!["RD-002".to_string(), "RD-003".to_string()]
    );

    // replace 以本次提交为准
    let replaced = state
        .dog_map_service
        .share(map.id, share_request(&["RD-003"], Some("replace")), &admin)
        .await
        .expect("share must work");
    assert_eq!(replaced.removed, 1);
    assert_eq!(replaced.shared_device_codes, vec!["RD-003".to_string()]);

    let shares = state
        .dog_map_service
        .list_shares(map.id, &admin)
        .await
        .expect("shares must be listed");
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].device_code, "RD-003");

    // 空 replace = 清空分享；再卸载一次是幂等的
    let cleared = state
        .dog_map_service
        .share(map.id, share_request(&[], Some("replace")), &admin)
        .await
        .expect("share must work");
    assert_eq!(cleared.removed, 1);
    assert!(cleared.shared_device_codes.is_empty());

    let unshared = state
        .dog_map_service
        .unshare(map.id, "RD-003", &admin)
        .await
        .expect("unshare must be idempotent");
    assert_eq!(unshared.removed, 0);
}

#[tokio::test]
async fn new_version_inherits_shared_devices() {
    let state = test_state().await;
    let admin = DataScope::new(1, true);

    let map = state
        .dog_map_service
        .create(map_request("园区地图", Some("RD-001")), &admin)
        .await
        .expect("map must be created");
    state
        .dog_map_service
        .share(map.id, share_request(&["RD-002"], None), &admin)
        .await
        .expect("share must work");

    // 派生新版本（草稿）后，被分享的设备不应因为换版本而失去这张图
    let derived = state
        .dog_map_service
        .new_version(map.id, &admin)
        .await
        .expect("new version must be created");
    let shares = state
        .dog_map_service
        .list_shares(derived.id, &admin)
        .await
        .expect("shares must be listed");
    assert_eq!(shares.len(), 1);
    assert_eq!(shares[0].device_code, "RD-002");
}

#[tokio::test]
async fn empty_append_is_rejected_and_unknown_device_is_not_required_to_exist() {
    let state = test_state().await;
    let admin = DataScope::new(1, true);

    let manual = state
        .dog_map_service
        .create(map_request("手工上传地图", None), &admin)
        .await
        .expect("map must be created");

    assert!(
        state
            .dog_map_service
            .share(manual.id, share_request(&[], None), &admin)
            .await
            .is_err(),
        "append 模式必须至少选一台设备"
    );

    // 手工上传的地图（没有上报设备）也能分享，这正是「不用重新建图」的场景
    let result = state
        .dog_map_service
        .share(manual.id, share_request(&["RD-201"], None), &admin)
        .await
        .expect("share must work");
    assert_eq!(result.shared_device_codes, vec!["RD-201".to_string()]);
}
