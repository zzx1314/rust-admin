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
        let mut request =
            reqwest::Client::new().get(format!("{base}/api/dogInternal/devices/assignments"));
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
    let again = upsert_device(
        &base,
        TOKEN,
        json!({"deviceId": "RD-200", "status": "执行任务"}),
    )
    .await;
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
        "deviceCode": "RD-201",
        "ownerUserId": owner,
        "remarks": "业务平台回传",
        "frameId": "ros_map_42 / 局部米制",
        "floors": "1F / B1",
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
    assert_eq!(
        body["data"]["deviceCode"], "RD-201",
        "地图应记录上报设备编号"
    );
    assert_eq!(
        body["data"]["frameId"], "ros_map_42 / 局部米制",
        "frame_id 应原样落库"
    );
    assert_eq!(body["data"]["floors"], "1F / B1", "覆盖楼层应原样落库");
    assert!(body["data"]["filePath"].as_str().is_some());
}

/// `POST /api/dogInternal/maps/exists`：只认未软删除的地图。
///
/// 业务平台据此判断「业务库已导入、中台又删了」的地图：这类地图不能算已同步，
/// 否则设备端永远不会重传。
#[tokio::test]
async fn map_exists_ignores_soft_deleted_maps() {
    let state = test_state(Some(TOKEN)).await;
    let service = state.dog_map_service.clone();
    let base = spawn_zentai(state).await;

    let info = json!({
        "mapName": "B 区机器人建图",
        "mapType": "2D栅格",
        "parkCode": "P02",
        "parkName": "B 区",
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
    let map_id = json(response).await["data"]["id"]
        .as_i64()
        .expect("地图 id");

    async fn exists(base: &str, ids: Vec<i64>) -> Value {
        let response = reqwest::Client::new()
            .post(format!("{base}/api/dogInternal/maps/exists"))
            .header("x-service-token", TOKEN)
            .json(&json!({ "ids": ids }))
            .send()
            .await
            .expect("exists request");
        assert_eq!(response.status(), 200);
        json(response).await
    }

    let body = exists(&base, vec![map_id, 999_999_999]).await;
    assert_eq!(body["data"]["existing"], json!([map_id]));

    service
        .delete(map_id, &x_rust::common::data_scope::DataScope::new(0, true))
        .await
        .expect("删除地图");
    let body = exists(&base, vec![map_id]).await;
    assert_eq!(
        body["data"]["existing"],
        json!([]),
        "软删除的地图不能再算存在"
    );
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

// ---------------------------------------------------------------------------
// 媒体资源回传：板端 AI 存证图（`/media/media-data/picture/<plan>/pt<n>/*.jpg`）
// ---------------------------------------------------------------------------

fn jpeg_bytes(payload: u8) -> Vec<u8> {
    // 服务端只校验文件头（真实 JPG 的 SOI），后面字节随便，够证明「落盘的是原字节」
    let mut bytes = vec![0xFF, 0xD8, 0xFF];
    bytes.extend_from_slice(&[payload; 128]);
    bytes
}

async fn upload_media(base: &str, info: &Value, file_name: &str, bytes: Vec<u8>) -> Value {
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes)
                .file_name(file_name.to_string())
                .mime_str("image/jpeg")
                .expect("mime"),
        )
        .text("mediaInfo", info.to_string());
    let response = reqwest::Client::new()
        .post(format!("{base}/api/dogInternal/media/upload"))
        .header("x-service-token", TOKEN)
        .multipart(form)
        .send()
        .await
        .expect("media upload request");
    json(response).await
}

#[tokio::test]
async fn media_upload_lands_in_dog_media_and_dedupes_by_sha256() {
    let mut state = test_state(Some(TOKEN)).await;
    // 落盘目录指到临时目录：默认的 `data/device_uploads` 会把测试图片写进仓库
    let upload_dir = std::env::temp_dir().join(format!(
        "zentai-media-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&upload_dir);
    state.upload_dir = upload_dir.clone();
    let service = state.dog_media_service.clone();
    let base = spawn_zentai(state).await;

    let info = json!({
        "deviceCode": "RD-201",
        "planId": "TK-9",
        "pointIndex": 2,
        "capturedAt": "2026-11-20 10:00:00",
        "source": "ai-patrol",
        "remarks": "dogcam_01.jpg",
    });
    let body = upload_media(&base, &info, "dogcam_01.jpg", jpeg_bytes(1)).await;
    assert_eq!(body["code"], json!(10200), "上传应成功：{body}");
    let data = &body["data"];
    assert_eq!(data["created"], json!(true));
    let media = &data["media"];
    let id = media["id"].as_i64().expect("媒体 id");
    assert_eq!(media["deviceCode"], "RD-201");
    assert_eq!(media["planId"], "TK-9");
    assert_eq!(media["pointIndex"], 2);
    assert_eq!(media["source"], "ai-patrol");
    assert_eq!(media["mediaType"], "图片");
    assert_eq!(media["capturedAt"], "2026-11-20 10:00:00");
    let relative = media["mediaPath"].as_str().expect("相对路径");
    assert!(
        relative.starts_with("media/RD-201/TK-9/pt2/"),
        "落盘路径应带设备/计划/航点：{relative}"
    );
    assert!(
        !relative.contains("..") && !relative.starts_with('/'),
        "mediaPath 必须是上传目录下的相对路径：{relative}"
    );
    assert!(
        upload_dir.join(relative).is_file(),
        "图片应真实落盘：{}",
        upload_dir.join(relative).display()
    );

    // 同一张图重复回传（板端重试 / 客户端重连）：复用同一条记录，不刷重复行
    let again = upload_media(&base, &info, "dogcam_01.jpg", jpeg_bytes(1)).await;
    assert_eq!(again["data"]["created"], json!(false));
    assert_eq!(again["data"]["media"]["id"], json!(id));

    // 软删除后同一张图再传回来：恢复原记录（普通唯一索引会挡住重复插入，
    // 恢复语义也让「删了又被同一个客户端重传」不会 500）
    service.delete(id).await.expect("删除应成功");
    let revived = upload_media(&base, &info, "dogcam_01.jpg", jpeg_bytes(1)).await;
    assert_eq!(revived["data"]["created"], json!(true));
    assert_eq!(
        revived["data"]["media"]["id"],
        json!(id),
        "应恢复同一条记录"
    );
    let record = service.get_by_id(id).await.expect("恢复后应可查询");
    assert_eq!(record.is_deleted, 0, "恢复后 is_deleted 必须清回 0");
    assert_eq!(record.device_code.as_deref(), Some("RD-201"));

    let _ = std::fs::remove_dir_all(&upload_dir);
}

#[tokio::test]
async fn media_upload_rejects_non_image_payload() {
    let mut state = test_state(Some(TOKEN)).await;
    state.upload_dir = std::env::temp_dir().join("zentai-media-reject");
    let base = spawn_zentai(state).await;

    let info = json!({"deviceCode": "RD-201", "planId": "TK-9", "pointIndex": 0});
    // 扩展名对、内容不是 JPG：不信任客户端声明的 mime
    let response = reqwest::Client::new()
        .post(format!("{base}/api/dogInternal/media/upload"))
        .header("x-service-token", TOKEN)
        .multipart(
            reqwest::multipart::Form::new()
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(b"not a jpeg".to_vec())
                        .file_name("frame.jpg")
                        .mime_str("image/jpeg")
                        .expect("mime"),
                )
                .text("mediaInfo", info.to_string()),
        )
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 400, "内容与扩展名不符应拒绝");

    // 不支持的扩展名同样拒绝
    let response = reqwest::Client::new()
        .post(format!("{base}/api/dogInternal/media/upload"))
        .header("x-service-token", TOKEN)
        .multipart(
            reqwest::multipart::Form::new()
                .part(
                    "file",
                    reqwest::multipart::Part::bytes(jpeg_bytes(2))
                        .file_name("clip.mp4")
                        .mime_str("video/mp4")
                        .expect("mime"),
                )
                .text("mediaInfo", info.to_string()),
        )
        .send()
        .await
        .expect("request");
    assert_eq!(response.status(), 400, "媒体只接受 JPG/PNG");
}

#[tokio::test]
async fn media_config_exposes_seeded_capture_settings() {
    let base = spawn_zentai(test_state(Some(TOKEN)).await).await;
    let body = reqwest::Client::new()
        .get(format!("{base}/api/dogInternal/media/config"))
        .header("x-service-token", TOKEN)
        .send()
        .await
        .expect("request")
        .json::<Value>()
        .await
        .expect("json");
    assert_eq!(body["code"], json!(10200), "{body}");
    assert_eq!(body["data"]["pictureDir"], "/media/media-data/picture");
    assert_eq!(body["data"]["frames"], 3);
}

/// 客户端现行方式：一个 zip 传 N 张（`mediaInfo.entries` 逐条目带计划/航点）。
#[tokio::test]
async fn media_zip_upload_stores_every_declared_entry() {
    let mut state = test_state(Some(TOKEN)).await;
    let upload_dir = std::env::temp_dir().join(format!(
        "zentai-media-zip-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&upload_dir);
    state.upload_dir = upload_dir.clone();
    let service = state.dog_media_service.clone();
    let base = spawn_zentai(state).await;

    // 包内 4 个条目：2 个正常、1 个没在 entries 里声明、1 个名字不合法（声明了也不用）
    let zip_bytes = media_zip(&[
        ("0001.jpg", jpeg_bytes(11)),
        ("0002.jpg", jpeg_bytes(12)),
        ("0003.jpg", jpeg_bytes(13)),
        ("../evil.jpg", jpeg_bytes(14)),
    ]);
    let info = json!({
        "deviceCode": "RD-201",
        "source": "ai-patrol",
        "entries": [
            {"name": "0001.jpg", "planId": "TK-9", "pointIndex": 0, "capturedAt": "2026-11-20 10:00:00", "remarks": "dogcam_01.jpg"},
            {"name": "0002.jpg", "planId": "TK-9", "pointIndex": 2, "capturedAt": "2026-11-20 10:00:02", "remarks": "dogcam_02.jpg"},
            {"name": "../evil.jpg", "planId": "TK-9", "pointIndex": 0},
        ],
    });
    let body = upload_media_zip(&base, &info, zip_bytes).await;
    assert_eq!(body["code"], json!(10200), "批量上传应成功：{body}");
    let data = &body["data"];
    assert_eq!(data["total"], 4);
    assert_eq!(data["failed"], 2, "未声明与非法名各算一个失败");
    let results = data["results"].as_array().expect("results 数组");
    let by_name = |name: &str| {
        results
            .iter()
            .find(|item| item["name"] == name)
            .unwrap_or_else(|| panic!("缺少条目 {name}：{results:?}"))
    };
    assert_eq!(by_name("0001.jpg")["ok"], json!(true));
    assert_eq!(by_name("0002.jpg")["ok"], json!(true));
    assert_eq!(
        by_name("0003.jpg")["ok"],
        json!(false),
        "未声明的条目必须拒绝"
    );
    assert!(
        by_name("../evil.jpg")["ok"] == json!(false),
        "包内名不合法必须拒绝"
    );
    let (id1, id2) = (
        by_name("0001.jpg")["mediaId"].as_i64().expect("媒体 id"),
        by_name("0002.jpg")["mediaId"].as_i64().expect("媒体 id"),
    );

    let first = service.get_by_id(id1).await.expect("第一条应可查");
    assert_eq!(first.plan_id.as_deref(), Some("TK-9"));
    assert_eq!(first.point_index, Some(0));
    assert_eq!(first.device_code.as_deref(), Some("RD-201"));
    assert_eq!(first.source.as_deref(), Some("ai-patrol"));
    let second = service.get_by_id(id2).await.expect("第二条应可查");
    assert_eq!(second.point_index, Some(2));

    // 两张图各自落盘（一张一条记录、一个文件）
    let files = std::fs::read_dir(upload_dir.join("media/RD-201/TK-9/pt0"))
        .expect("航点目录应存在")
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("jpg"))
        .count();
    assert!(files >= 1, "pt0 下应至少有一张图");

    // 同一个包重传：全部按 sha256 命中已有记录，不新增行（客户端据此删板端文件）
    let again = upload_media_zip(
        &base,
        &json!({
            "deviceCode": "RD-201",
            "entries": [
                {"name": "0001.jpg", "planId": "TK-9", "pointIndex": 0},
                {"name": "0002.jpg", "planId": "TK-9", "pointIndex": 2},
            ],
        }),
        media_zip(&[("0001.jpg", jpeg_bytes(11)), ("0002.jpg", jpeg_bytes(12))]),
    )
    .await;
    let results = again["data"]["results"].as_array().expect("results");
    assert!(
        results
            .iter()
            .all(|item| item["ok"] == json!(true) && item["created"] == json!(false)),
        "重复包应全部命中已有记录：{results:?}"
    );

    let _ = std::fs::remove_dir_all(&upload_dir);
}

fn media_zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let cursor = std::io::Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in entries {
        use std::io::Write;
        writer.start_file(*name, options).expect("写入条目");
        writer.write_all(bytes).expect("写入内容");
    }
    writer.finish().expect("生成 zip").into_inner()
}

async fn upload_media_zip(base: &str, info: &Value, bytes: Vec<u8>) -> Value {
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes)
                .file_name("media.zip")
                .mime_str("application/zip")
                .expect("mime"),
        )
        .text("mediaInfo", info.to_string());
    let response = reqwest::Client::new()
        .post(format!("{base}/api/dogInternal/media/upload"))
        .header("x-service-token", TOKEN)
        .multipart(form)
        .send()
        .await
        .expect("zip upload request");
    json(response).await
}

/// 「参数配置」页改的两项是板端媒体目录的真源：写图（`ai_start`）与回传
/// （`同步媒体` 指令 / 客户端扫描）都读它，写坏了要回退默认值而不是卡住巡检。
#[tokio::test]
async fn media_capture_config_reads_params_and_falls_back_on_garbage() {
    use x_rust::business::dog::config::domain::UpdateConfigRequest;
    use x_rust::business::dog::media::service::{DEFAULT_MEDIA_PICTURE_DIR, load_capture_config};

    let state = test_state(Some(TOKEN)).await;
    let configs = state
        .dog_config_service
        .list_all()
        .await
        .expect("配置表应可读");
    let by_name = |name: &str| {
        configs
            .iter()
            .find(|config| config.config_name == name)
            .unwrap_or_else(|| panic!("缺少预置配置 {name}"))
            .id
    };
    let dir_id = by_name("media_picture_dir");
    let frames_id = by_name("media_frames");

    // 正常值：原样生效
    state
        .dog_config_service
        .update(
            dir_id,
            UpdateConfigRequest {
                config_name: None,
                config_value: Some("/data/picture".to_string()),
                description: None,
            },
        )
        .await
        .expect("改目录");
    state
        .dog_config_service
        .update(
            frames_id,
            UpdateConfigRequest {
                config_name: None,
                config_value: Some("5".to_string()),
                description: None,
            },
        )
        .await
        .expect("改帧数");
    let config = load_capture_config(&state.dog_config_service).await;
    assert_eq!(config.picture_dir, "/data/picture");
    assert_eq!(config.frames, 5);

    // 写坏（相对路径 / 超出 1~10）：逐项回退，不影响另一个合法值
    state
        .dog_config_service
        .update(
            dir_id,
            UpdateConfigRequest {
                config_name: None,
                config_value: Some("../etc".to_string()),
                description: None,
            },
        )
        .await
        .expect("改坏目录");
    state
        .dog_config_service
        .update(
            frames_id,
            UpdateConfigRequest {
                config_name: None,
                config_value: Some("99".to_string()),
                description: None,
            },
        )
        .await
        .expect("改大帧数");
    let config = load_capture_config(&state.dog_config_service).await;
    assert_eq!(
        config.picture_dir, DEFAULT_MEDIA_PICTURE_DIR,
        "坏路径回退默认"
    );
    assert_eq!(config.frames, 10, "帧数上限 10");
}
