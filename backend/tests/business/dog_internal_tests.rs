//! 中台内部接口（`/api/dogInternal/*`）测试。
//!
//! 这组接口让业务平台用共享服务令牌完成「设备台账同步 / 分配关系查询 /
//! 设备删除 / 地图回传」，从而不需要中台账号与 JWT 会话。重点验证：
//!   1. 没有令牌或令牌错误一律 401；
//!   2. 台账 upsert 幂等、分配关系原样返回（业务平台据此做数据权限）；
//!   3. 删除幂等（重复删除不报错）；
//!   4. 地图包回传落到 dog_map，并按 `ownerUserId` 归属到设备所属用户。

use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use x_rust::api::AppState;
use x_rust::app::App;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};
use zip::write::SimpleFileOptions;

const JWT_SECRET: &str = "test-secret";
const TOKEN: &str = "test-service-token";

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

async fn test_state(token: Option<&str>) -> AppState {
    let app = App::new("sqlite::memory:")
        .await
        .expect("in-memory database must migrate");
    let mut state = app.build_state_with_token_store(Arc::new(FakeTokenStore::new()), JWT_SECRET);
    state.business_api_token = token.map(str::to_string);
    state
}

/// 启动一个真实监听的中台，返回基地址。
async fn spawn_zentai(state: AppState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind zentai");
    let addr = listener.local_addr().expect("local addr");
    let router = x_rust::api::routes::create_router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("http://{addr}")
}

async fn json(response: reqwest::Response) -> Value {
    let text = response.text().await.unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_else(|_| json!({ "raw": text }))
}

async fn upsert_device(base: &str, token: &str, body: Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/api/dogInternal/devices/upsert"))
        .header("x-service-token", token)
        .json(&body)
        .send()
        .await
        .expect("upsert request")
}

#[tokio::test]
async fn internal_endpoints_require_service_token() {
    let base = spawn_zentai(test_state(Some(TOKEN)).await).await;

    for header in [None, Some("wrong-token")] {
        let mut request = reqwest::Client::new().get(format!(
            "{base}/api/dogInternal/devices/assignments"
        ));
        if let Some(token) = header {
            request = request.header("x-service-token", token);
        }
        let response = request.send().await.expect("request");
        assert_eq!(response.status(), 401, "令牌缺失/错误必须拒绝");
    }
}

#[tokio::test]
async fn internal_endpoints_reject_when_token_not_configured() {
    // 中台没配 APP_DOG__BUSINESS_API_TOKEN：内部接口整体不可用（400 提示配置）
    let base = spawn_zentai(test_state(None).await).await;
    let response = reqwest::Client::new()
        .get(format!("{base}/api/dogInternal/devices/assignments"))
        .header("x-service-token", TOKEN)
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 400);
    let body = json(response).await;
    assert!(
        body["msg"]
            .as_str()
            .unwrap_or_default()
            .contains("APP_DOG__BUSINESS_API_TOKEN"),
        "应提示缺失的配置项，实际：{}",
        body["msg"]
    );
}

#[tokio::test]
async fn upsert_then_list_assignments() {
    let state = test_state(Some(TOKEN)).await;
    // 分配关系在查询时会 JOIN p_sys_user，所以只能用库里真实存在的用户
    let users = state
        .dog_device_service
        .assignable_users()
        .await
        .expect("可分配用户");
    let user_id = users.first().expect("种子数据应有中台用户").id;
    let base = spawn_zentai(state).await;

    let created = upsert_device(
        &base,
        TOKEN,
        json!({
            "deviceId": "RD-200",
            "deviceName": "测试狗",
            "status": "在线待命",
            "parkCode": "P01",
            "parkName": "A 区",
            "userIds": [user_id],
        }),
    )
    .await;
    assert_eq!(created.status(), 200);
    assert_eq!(json(created).await["data"]["deviceId"], "RD-200");

    // 再 upsert 一次：幂等，不会产生第二条台账
    let again = upsert_device(&base, TOKEN, json!({"deviceId": "RD-200", "status": "执行任务"})).await;
    assert_eq!(again.status(), 200);

    let response = reqwest::Client::new()
        .get(format!("{base}/api/dogInternal/devices/assignments"))
        .header("x-service-token", TOKEN)
        .send()
        .await
        .expect("assignments request");
    assert_eq!(response.status(), 200);
    let body = json(response).await;
    let devices = body["data"].as_array().expect("data must be array");
    assert_eq!(devices.len(), 1, "重复 upsert 不应产生多条台账");
    assert_eq!(devices[0]["deviceId"], "RD-200");
    assert_eq!(devices[0]["userId"], json!(0), "平台同步的台账创建者记为 0");
    let user_ids: Vec<i64> = devices[0]["userIds"]
        .as_array()
        .expect("userIds must be array")
        .iter()
        .filter_map(Value::as_i64)
        .collect();
    assert_eq!(user_ids, vec![user_id], "分配关系必须原样返回");
}

#[tokio::test]
async fn delete_is_idempotent() {
    let base = spawn_zentai(test_state(Some(TOKEN)).await).await;
    let created = upsert_device(&base, TOKEN, json!({"deviceId": "RD-201"})).await;
    assert_eq!(created.status(), 200);

    let client = reqwest::Client::new();
    for expected in [true, false] {
        let response = client
            .delete(format!("{base}/api/dogInternal/devices/RD-201"))
            .header("x-service-token", TOKEN)
            .send()
            .await
            .expect("delete request");
        assert_eq!(response.status(), 200);
        assert_eq!(json(response).await["data"]["deleted"], json!(expected));
    }
}

#[tokio::test]
async fn map_upload_lands_in_dog_map_with_owner() {
    let state = test_state(Some(TOKEN)).await;
    let owner = state
        .dog_device_service
        .assignable_users()
        .await
        .expect("可分配用户")
        .first()
        .expect("种子数据应有中台用户")
        .id;
    let base = spawn_zentai(state).await;

    let info = json!({
        "mapName": "A 区机器人建图",
        "mapType": "2D栅格",
        "parkCode": "P01",
        "parkName": "A 区",
        "ownerUserId": owner,
        "remarks": "业务平台回传",
    });
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(sample_package())
                .file_name("map.zip")
                .mime_str("application/zip")
                .expect("mime"),
        )
        .text("mapInfo", info.to_string());
    let response = reqwest::Client::new()
        .post(format!("{base}/api/dogInternal/maps/upload"))
        .header("x-service-token", TOKEN)
        .multipart(form)
        .send()
        .await
        .expect("upload request");

    assert_eq!(response.status(), 200);
    let body = json(response).await;
    assert!(body["data"]["id"].as_i64().unwrap_or(0) > 0);
    assert_eq!(body["data"]["mapName"], "A 区机器人建图");
    assert_eq!(
        body["data"]["userId"],
        json!(owner),
        "地图应归属 ownerUserId"
    );
    assert_eq!(body["data"]["parkCode"], "P01");
    assert!(body["data"]["filePath"].as_str().is_some());
}

/// 一个最小可解析的地图包：`model.pgm` + `model.yaml`。
fn sample_package() -> Vec<u8> {
    // 2x2 灰度 PGM
    let pgm = b"P5\n2 2\n255\n\x00\x40\x80\xff".to_vec();
    let yaml = b"image: model.pgm\nresolution: 0.05\norigin: [1.0, 2.0, 0.0]\nnegate: 0\n";
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in [("model.pgm", pgm), ("model.yaml", yaml.to_vec())] {
        writer.start_file(name, options).expect("start file");
        writer.write_all(&data).expect("write file");
    }
    writer.finish().expect("finish zip").into_inner()
}
