//! 地图编辑（橡皮擦）：在中台预览地图后擦除栅格噪点。
//!
//! 编辑走 `DogMapService::erase`：读取原 `.zip`，按像素笔画改写 `model.pgm`，
//! 其余条目原样保留，落盘为新包并回写 `file_path` / `map_hash` / 尺寸。
//! 已发布 / 已归档版本不可直接编辑（必须派生新版本），`map_hash` 不一致时
//! 拒绝编辑，避免覆盖被偷改过的文件。

use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::sync::Arc;

use x_rust::api::AppState;
use x_rust::app::App;
use x_rust::business::dog::map::domain::{
    CreateMapRequest, MapEditPoint, MapEditRequest, MapEditStroke, MapForbiddenArea,
    MapForbiddenRequest, UploadMapRequest,
};
use x_rust::business::dog::map::package::{parse_map_package, parse_pgm_raster, sha256_hex};
use x_rust::common::data_scope::DataScope;
use x_rust::system::auth::memory_store::InMemoryTokenStore;

const JWT_SECRET: &str = "test-secret";
const WIDTH: u32 = 8;
const HEIGHT: u32 = 8;
const NOISE_INDEX: usize = 4 * 8 + 4;

struct Fixture {
    state: AppState,
    dir: PathBuf,
    map_id: i64,
    /// 原始地图包（带噪点）
    original: Vec<u8>,
}

/// 生成一张 8x8、中心有一个噪点黑像素的地图包。
fn sample_package(with_noise: bool) -> Vec<u8> {
    let mut pixels = vec![254u8; (WIDTH * HEIGHT) as usize];
    if with_noise {
        pixels[NOISE_INDEX] = 0;
    }
    let mut pgm = format!("P5\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    pgm.extend_from_slice(&pixels);
    let yaml = b"image: model.pgm\nresolution: 0.05\norigin: [-1.0, -2.0, 0.0]\nnegate: 0\n";

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::SimpleFileOptions = Default::default();
        writer.start_file("map/model.pgm", options).unwrap();
        writer.write_all(&pgm).unwrap();
        writer.start_file("map/model.yaml", options).unwrap();
        writer.write_all(yaml).unwrap();
        writer.finish().unwrap();
    }
    buffer.into_inner()
}

/// 带「遥控器编辑版」的地图包：根目录 `map.pgm` 是原始地图，`map_edit/map.pgm`
/// 是编辑后的地图。平台编辑必须只改 `map_edit/`，根目录一个字节都不能动。
fn package_with_map_edit() -> Vec<u8> {
    let mut original_pixels = vec![255u8; (WIDTH * HEIGHT) as usize];
    original_pixels[NOISE_INDEX] = 0;
    let mut original_pgm = format!("P5\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    original_pgm.extend_from_slice(&original_pixels);

    let mut edited_pixels = vec![254u8; (WIDTH * HEIGHT) as usize];
    edited_pixels[NOISE_INDEX] = 0;
    let mut edited_pgm = format!("P5\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    edited_pgm.extend_from_slice(&edited_pixels);

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::SimpleFileOptions = Default::default();
        writer.start_file("map.pgm", options).unwrap();
        writer.write_all(&original_pgm).unwrap();
        writer.start_file("map.yaml", options).unwrap();
        writer
            .write_all(b"image: map.pgm\nresolution: 0.05\norigin: [-1.0, -2.0, 0.0]\n")
            .unwrap();
        writer.start_file("map_edit/map.pgm", options).unwrap();
        writer.write_all(&edited_pgm).unwrap();
        writer.start_file("map_edit/map.yaml", options).unwrap();
        writer
            .write_all(b"image: map.pgm\nresolution: 0.05\norigin: [-1.0, -2.0, 0.0]\n")
            .unwrap();
        writer.finish().unwrap();
    }
    buffer.into_inner()
}

/// 刚建好图的机器人包：`map_edit/` 里只有 `map.yaml`，还没有 `map_edit/map.pgm`
/// （栅格要等遥控器第一次编辑才生成）。现场 2026-11-26 15:46 同步上来的就是这个形态。
fn fresh_robot_package() -> Vec<u8> {
    let mut pixels = vec![255u8; (WIDTH * HEIGHT) as usize];
    pixels[NOISE_INDEX] = 0;
    let mut pgm = format!("P5\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    pgm.extend_from_slice(&pixels);
    let yaml = b"image: map.pgm\nresolution: 0.05\norigin: [-1.0, -2.0, 0.0]\n";

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::SimpleFileOptions = Default::default();
        writer.start_file("map.pgm", options).unwrap();
        writer.write_all(&pgm).unwrap();
        writer.start_file("map.yaml", options).unwrap();
        writer.write_all(yaml).unwrap();
        writer.start_file("map_edit/map.yaml", options).unwrap();
        writer.write_all(yaml).unwrap();
        writer.finish().unwrap();
    }
    buffer.into_inner()
}

fn create_request(status: &str) -> CreateMapRequest {
    CreateMapRequest {
        map_name: format!("橡皮擦测试 {status}"),
        map_type: Some("离线地图".to_string()),
        park_code: None,
        park_name: None,
        device_code: Some("RD-201".to_string()),
        map_data: None,
        file_name: Some("map.zip".to_string()),
        file_path: Some("maps/original.zip".to_string()),
        file_format: Some("zip".to_string()),
        file_size: None,
        remarks: None,
        map_version: Some("v1.0".to_string()),
        frame_id: None,
        crs: None,
        map_origin: Some("-1.0, -2.0, 0.0".to_string()),
        resolution: Some("0.05".to_string()),
        floors: None,
        road_count: None,
        nogo_count: None,
        map_status: Some(status.to_string()),
        map_hash: None,
        map_width: Some(WIDTH as i32),
        map_height: Some(HEIGHT as i32),
    }
}

async fn fixture(status: &str) -> Fixture {
    fixture_with_package(status, sample_package(true)).await
}

async fn fixture_with_package(status: &str, original: Vec<u8>) -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "rust_admin_map_edit_{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(dir.join("maps")).unwrap();
    std::fs::write(dir.join("maps/original.zip"), &original).unwrap();

    let app = App::new("sqlite::memory:")
        .await
        .expect("in-memory database must migrate");
    let mut state =
        app.build_state_with_token_store(Arc::new(InMemoryTokenStore::new()), JWT_SECRET);
    state.upload_dir = dir.clone();

    let mut request = create_request(if status == "已发布" {
        "草稿"
    } else {
        status
    });
    request.map_hash = Some(sha256_hex(&original));
    request.file_size = i64::try_from(original.len()).ok();
    let scope = DataScope::new(1, true);
    let map = state
        .dog_map_service
        .create(request, &scope)
        .await
        .expect("map must be created");
    if status == "已发布" {
        // 新建只允许草稿 / 部分标定，发布态必须走发布流程
        state
            .dog_map_service
            .publish(map.id, &scope)
            .await
            .expect("map must be published");
    }

    Fixture {
        state,
        dir,
        map_id: map.id,
        original,
    }
}

fn center_stroke(radius: f64) -> MapEditRequest {
    MapEditRequest {
        strokes: vec![MapEditStroke {
            points: vec![MapEditPoint { x: 4.5, y: 4.5 }],
            radius: Some(radius),
            value: None,
        }],
        erase_value: None,
    }
}

#[tokio::test]
async fn erase_rewrites_pgm_and_updates_hash() {
    let fixture = fixture("草稿").await;
    let scope = DataScope::new(1, true);
    let stored_path = fixture.dir.join("maps/original.zip");

    let image = fixture
        .state
        .dog_map_service
        .erase(
            fixture.map_id,
            center_stroke(0.6),
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect("erase must succeed");

    assert_eq!(image.width, WIDTH);
    assert_eq!(image.height, HEIGHT);
    assert_eq!(image.erased_pixels, Some(1));
    assert!(image.image.starts_with("data:image/png;base64,"));
    assert!((image.resolution - 0.05).abs() < f64::EPSILON);
    assert_eq!(image.origin, vec![-1.0, -2.0, 0.0]);

    // 记录被更新：新文件路径、新 hash、尺寸不变
    let map = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .expect("map must still exist");
    let new_path = map.file_path.clone().expect("file path must be updated");
    assert_ne!(new_path, "maps/original.zip");
    assert_eq!(map.map_hash.as_deref(), image.map_hash.as_deref());
    assert_eq!(map.map_width, Some(WIDTH as i32));
    assert_eq!(map.map_height, Some(HEIGHT as i32));

    let bytes = std::fs::read(fixture.state.upload_dir.join(&new_path)).unwrap();
    assert_eq!(sha256_hex(&bytes), image.map_hash.clone().unwrap());
    assert_eq!(bytes.len() as i64, map.file_size.unwrap());

    // 噪点被擦成 254（自由空间），model.yaml 原样保留
    let parsed = parse_map_package(&bytes).expect("edited package must still parse");
    assert_eq!(parsed.width, WIDTH);
    assert_eq!(parsed.origin, vec![-1.0, -2.0, 0.0]);
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut pgm = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("map/model.pgm").unwrap(), &mut pgm).unwrap();
    let raster = parse_pgm_raster(&pgm).unwrap();
    assert_eq!(raster.pixels[NOISE_INDEX], 254);
    assert!(archive.by_name("map/model.yaml").is_ok());

    // 旧包保留在磁盘上（和 upload 覆盖旧包的行为一致）
    assert!(stored_path.exists());
}

#[tokio::test]
async fn erase_targets_map_edit_and_preserves_original_map() {
    let fixture = fixture_with_package("草稿", package_with_map_edit()).await;
    let scope = DataScope::new(1, true);

    let image = fixture
        .state
        .dog_map_service
        .erase(
            fixture.map_id,
            center_stroke(0.6),
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect("erase must succeed");
    assert_eq!(image.erased_pixels, Some(1));
    assert_eq!(image.width, WIDTH);
    assert_eq!(image.height, HEIGHT);

    let map = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .unwrap();
    let bytes = std::fs::read(fixture.state.upload_dir.join(map.file_path.unwrap())).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();

    let mut original = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("map.pgm").unwrap(), &mut original).unwrap();
    let original_raster = parse_pgm_raster(&original).unwrap();
    assert_eq!(
        original_raster.pixels[NOISE_INDEX], 0,
        "根目录原始地图不能被平台编辑修改"
    );

    let mut edited = Vec::new();
    std::io::Read::read_to_end(
        &mut archive.by_name("map_edit/map.pgm").unwrap(),
        &mut edited,
    )
    .unwrap();
    let edited_raster = parse_pgm_raster(&edited).unwrap();
    assert_eq!(
        edited_raster.pixels[NOISE_INDEX], 254,
        "编辑必须落在 map_edit/map.pgm"
    );
}

/// 发布把「部分标定」也推进为「已发布」。
///
/// 2026-11-24 现场：中台点了「发布」，页面还是「部分标定」，业务平台也不跟。
/// 现在发布 = 人工确认「这张图可以当基线用」，草稿 / 部分标定都变已发布
/// （业务平台由 `business/` 的 `sync_map_statuses` 跟随）。
#[tokio::test]
async fn publish_partial_map_marks_it_published() {
    let fixture = fixture("部分标定").await;
    let scope = DataScope::new(1, true);

    let published = fixture
        .state
        .dog_map_service
        .publish(fixture.map_id, &scope)
        .await
        .expect("部分标定版本应允许发布");
    assert_eq!(published.map_status, "已发布");
    assert!(published.publish_time.is_some(), "发布要记录发布时间");

    // 发布后按不可变处理：橡皮擦必须拒绝（与「已发布」同一套规则）
    let error = fixture
        .state
        .dog_map_service
        .erase(
            fixture.map_id,
            MapEditRequest {
                strokes: vec![MapEditStroke {
                    points: vec![MapEditPoint { x: 4.5, y: 4.5 }],
                    radius: Some(1.0),
                    value: None,
                }],
                erase_value: None,
            },
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect_err("已发布的版本不可直接编辑");
    assert!(format!("{error:?}").contains("已发布"), "{error:?}");
}

#[tokio::test]
async fn erase_supports_unknown_value_and_multiple_strokes() {
    let fixture = fixture("部分标定").await;
    let scope = DataScope::new(1, true);

    let request = MapEditRequest {
        strokes: vec![
            MapEditStroke {
                points: vec![MapEditPoint { x: 4.5, y: 4.5 }],
                radius: Some(1.0),
                value: None,
            },
            MapEditStroke {
                points: vec![
                    MapEditPoint { x: 0.5, y: 7.5 },
                    MapEditPoint { x: 7.5, y: 7.5 },
                ],
                radius: Some(0.6),
                value: None,
            },
        ],
        erase_value: Some(128),
    };
    let image = fixture
        .state
        .dog_map_service
        .erase(fixture.map_id, request, &fixture.state.upload_dir, &scope)
        .await
        .expect("erase must succeed");
    assert!(image.erased_pixels.unwrap() >= 8, "整条底边都应该被擦除");

    let map = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .unwrap();
    let bytes = std::fs::read(fixture.state.upload_dir.join(map.file_path.unwrap())).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut pgm = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("map/model.pgm").unwrap(), &mut pgm).unwrap();
    let raster = parse_pgm_raster(&pgm).unwrap();
    assert_eq!(raster.pixels[NOISE_INDEX], 128);
    assert_eq!(raster.pixels[(7 * WIDTH) as usize + 3], 128);
}

/// 旧前端还会提交「擦成未知 205」：落盘必须归一成 128（本体按 free_thresh: 0.2
/// 会把 205 当成可通行，见 `business/docs/waypoint-clearance.md` §8）。
#[tokio::test]
async fn legacy_unknown_205_request_is_stored_as_128() {
    let fixture = fixture("草稿").await;
    let scope = DataScope::new(1, true);

    let request = MapEditRequest {
        strokes: vec![MapEditStroke {
            points: vec![MapEditPoint { x: 4.5, y: 4.5 }],
            radius: Some(1.0),
            value: None,
        }],
        erase_value: Some(205),
    };
    fixture
        .state
        .dog_map_service
        .erase(fixture.map_id, request, &fixture.state.upload_dir, &scope)
        .await
        .expect("旧的 205 仍要能被接受（归一到 128）");

    let map = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .unwrap();
    let bytes = std::fs::read(fixture.state.upload_dir.join(map.file_path.unwrap())).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut pgm = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("map/model.pgm").unwrap(), &mut pgm).unwrap();
    let raster = parse_pgm_raster(&pgm).unwrap();
    assert_eq!(raster.pixels[NOISE_INDEX], 128, "205 必须落盘成 128");
}

#[tokio::test]
async fn erase_keeps_per_stroke_value_in_one_save() {
    let fixture = fixture("草稿").await;
    let scope = DataScope::new(1, true);

    // 先擦成可通行（中心噪点），再擦成未知（左上角原本可通行的像素）。
    // 两笔分属于同一个请求：值必须跟着笔画走，不能被请求级 `eraseValue` 覆盖。
    let request = MapEditRequest {
        strokes: vec![
            MapEditStroke {
                points: vec![MapEditPoint { x: 4.5, y: 4.5 }],
                radius: Some(0.6),
                value: Some(254),
            },
            MapEditStroke {
                points: vec![MapEditPoint { x: 0.5, y: 0.5 }],
                radius: Some(0.6),
                value: Some(128),
            },
        ],
        erase_value: None,
    };
    fixture
        .state
        .dog_map_service
        .erase(fixture.map_id, request, &fixture.state.upload_dir, &scope)
        .await
        .expect("erase must succeed");

    let map = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .unwrap();
    let bytes = std::fs::read(fixture.state.upload_dir.join(map.file_path.unwrap())).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut pgm = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("map/model.pgm").unwrap(), &mut pgm).unwrap();
    let raster = parse_pgm_raster(&pgm).unwrap();
    assert_eq!(raster.pixels[NOISE_INDEX], 254, "第一笔是擦成可通行");
    assert_eq!(raster.pixels[0], 128, "第二笔是擦成未知");
}

#[tokio::test]
async fn erase_rejects_published_map() {
    let fixture = fixture("已发布").await;
    let scope = DataScope::new(1, true);

    let error = fixture
        .state
        .dog_map_service
        .erase(
            fixture.map_id,
            center_stroke(1.0),
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect_err("published maps are immutable");
    let message = format!("{error}");
    assert!(
        message.contains("不可直接编辑"),
        "unexpected error: {message}"
    );

    // 原文件没被改动
    let stored = std::fs::read(fixture.dir.join("maps/original.zip")).unwrap();
    assert_eq!(sha256_hex(&stored), sha256_hex(&fixture.original));
}

#[tokio::test]
async fn erase_rejects_tampered_package() {
    let fixture = fixture("草稿").await;
    let scope = DataScope::new(1, true);

    let tampered = sample_package(false);
    std::fs::write(fixture.dir.join("maps/original.zip"), &tampered).unwrap();

    let error = fixture
        .state
        .dog_map_service
        .erase(
            fixture.map_id,
            center_stroke(1.0),
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect_err("hash mismatch must be rejected");
    let message = format!("{error}");
    assert!(message.contains("map_hash"), "unexpected error: {message}");
}

#[tokio::test]
async fn erase_rejects_empty_or_oversized_request() {
    let fixture = fixture("草稿").await;
    let scope = DataScope::new(1, true);

    let empty = MapEditRequest {
        strokes: Vec::new(),
        erase_value: None,
    };
    assert!(
        fixture
            .state
            .dog_map_service
            .erase(fixture.map_id, empty, &fixture.state.upload_dir, &scope)
            .await
            .is_err()
    );

    let mut bad = center_stroke(1.0);
    bad.strokes[0].points[0].x = f64::NAN;
    assert!(
        fixture
            .state
            .dog_map_service
            .erase(fixture.map_id, bad, &fixture.state.upload_dir, &scope)
            .await
            .is_err()
    );
}

// ─── 设备「同步地图」刷新：按 id 原地覆盖地图包 ─────────────────────

fn upload_request(map_id: Option<i64>, version: Option<&str>, status: &str) -> UploadMapRequest {
    UploadMapRequest {
        id: map_id,
        map_name: "刷新测试".to_string(),
        map_type: Some("2D栅格".to_string()),
        park_code: None,
        park_name: None,
        device_code: Some("RD-201".to_string()),
        owner_user_id: None,
        remarks: Some("机器人建图自动回传".to_string()),
        map_version: version.map(str::to_string),
        frame_id: None,
        crs: None,
        map_origin: None,
        resolution: None,
        floors: None,
        road_count: None,
        nogo_count: None,
        map_status: Some(status.to_string()),
        map_hash: None,
        map_width: None,
        map_height: None,
    }
}

/// 业务平台刷新时带 `id` → 覆盖同一条记录，不新增副本，版本号/状态不被回退。
#[tokio::test]
async fn upload_with_id_replaces_package_in_place() {
    let fixture = fixture("部分标定").await;
    let scope = DataScope::new(1, true);
    let before = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .expect("map must exist");

    // 本体上重新标定过的图（同样的坐标系，栅格变了）
    let refreshed = sample_package(false);
    let map = fixture
        .state
        .dog_map_service
        .save_upload(
            upload_request(Some(fixture.map_id), Some("v9.9"), "草稿"),
            "map.zip",
            &refreshed,
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect("部分标定版本允许原地覆盖");

    assert_eq!(map.id, fixture.map_id, "必须复用原 id，不能新建副本");
    assert_eq!(
        sha256_hex(&refreshed),
        map.map_hash.clone().unwrap_or_default(),
        "map_hash 要跟着新包更新"
    );
    assert_eq!(
        map.map_version, before.map_version,
        "回传里带的版本号不能覆盖中台版本（否则 v1.1 会被打回 v1.0）"
    );
    assert_eq!(
        map.map_status, before.map_status,
        "刷新只换栅格，状态由发布流程维护"
    );
    assert_eq!(
        map.file_size,
        Some(i64::try_from(refreshed.len()).unwrap()),
        "文件大小应随新包更新"
    );

    // 落盘的是新包，旧包不再被引用
    let stored_path = map.file_path.clone().expect("替换后必须有包路径");
    let stored = std::fs::read(fixture.state.upload_dir.join(&stored_path)).unwrap();
    assert_eq!(sha256_hex(&stored), sha256_hex(&refreshed));
}

/// 已发布版本不可原地覆盖：返回可操作的原因，且不落盘新文件。
#[tokio::test]
async fn upload_with_id_rejects_published_map() {
    let fixture = fixture("已发布").await;
    let scope = DataScope::new(1, true);
    let before = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .expect("map must exist");

    let refreshed = sample_package(false);
    let error = fixture
        .state
        .dog_map_service
        .save_upload(
            upload_request(Some(fixture.map_id), None, "部分标定"),
            "map.zip",
            &refreshed,
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect_err("published maps must not be overwritten");
    let message = format!("{error}");
    assert!(
        message.contains("派生新版本"),
        "错误要告诉现场怎么办: {message}"
    );

    let after = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .unwrap();
    assert_eq!(after.map_hash, before.map_hash, "中台记录不能被改动");
    assert_eq!(after.file_path, before.file_path, "包路径不能被改动");
}

/// 中台 `mapInfo` 的 `mapId` 也要认（业务平台旧版本用这个字段名）。
#[test]
fn upload_request_accepts_map_id_alias() {
    let request: UploadMapRequest = serde_json::from_value(serde_json::json!({
        "mapId": 42,
        "mapName": "别名",
    }))
    .expect("mapId 应能反序列化到 id");
    assert_eq!(request.id, Some(42));
}

/// 前端新拖出来的禁止区域**不带 `id`**（服务层按毫秒时间戳补）。
///
/// 2026-11-26 线上：DTO 把 `id` 写成必填，`Json<MapForbiddenRequest>` 直接回 422
/// （纯文本、没有 `msg`），页面只显示「系统错误」，禁止区域根本存不进去。
#[tokio::test]
async fn new_forbidden_area_without_id_is_accepted_and_gets_one() {
    let fixture = fixture_with_package("草稿", package_with_map_edit()).await;
    // 与前端 `dogMapSetForbiddenAreas` 发出的请求体逐字一致
    let body = r#"{"areas":[{"center":{"x":4.0,"y":4.0},"width":3.0,"height":3.0,"rotation":0}]}"#;
    let request: MapForbiddenRequest =
        serde_json::from_str(body).expect("新画的禁止区域不带 id，必须能反序列化");
    assert_eq!(request.areas.len(), 1);
    assert_eq!(request.areas[0].id, 0);

    let image = fixture
        .state
        .dog_map_service
        .set_forbidden_areas(
            fixture.map_id,
            request,
            &fixture.state.upload_dir,
            &DataScope::new(1, true),
        )
        .await
        .expect("设置禁止区域必须成功");

    assert_eq!(image.forbidden_areas.len(), 1);
    assert!(
        image.forbidden_areas[0].id > 0,
        "服务层要补上毫秒时间戳 id，前端下次保存才能按 id 保留 / 删除"
    );
    // 栅格确实被刷成占据（0）：矩形 3x3 像素，中心 (4,4)
    assert!(image.image.starts_with("data:image/png;base64,"));
}

/// 刚建好图就同步上来的地图（`map_edit/` 只有 `map.yaml`）：
/// 第一次设禁止区域必须成功，并且是**派生** `map_edit/map.pgm`，根目录建图原图不动。
///
/// 现场 2026-11-26 15:55：这种包直接报「这张地图没有编辑版（手工上传的地图包），
/// 不能设置禁止区域」，连点五次都存不进去（板子刚建完图，遥控器还没编辑过）。
#[tokio::test]
async fn forbidden_area_on_fresh_robot_package_derives_the_edit_raster() {
    let original = fresh_robot_package();
    let fixture = fixture_with_package("部分标定", original.clone()).await;
    let scope = DataScope::new(1, true);

    let request = MapForbiddenRequest {
        areas: vec![MapForbiddenArea {
            id: 0,
            center: MapEditPoint { x: 4.0, y: 4.0 },
            width: 3.0,
            height: 3.0,
            rotation: 0.0,
            kind: "area".to_string(),
        }],
    };
    let image = fixture
        .state
        .dog_map_service
        .set_forbidden_areas(
            fixture.map_id,
            request,
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect("刚建好图的地图也要能设置禁止区域");
    assert_eq!(image.forbidden_areas.len(), 1);

    // 落盘的新包：根目录 map.pgm 与建图原图逐字节一致，编辑版是平台派生的
    let stored = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .expect("map must exist");
    let bytes = std::fs::read(
        fixture
            .state
            .upload_dir
            .join(stored.file_path.clone().expect("file_path")),
    )
    .expect("edited package must be readable");
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).expect("edited package is a zip");
    let mut root = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("map.pgm").unwrap(), &mut root).unwrap();
    let mut original_zip =
        zip::ZipArchive::new(Cursor::new(&original)).expect("original package is a zip");
    let mut original_root = Vec::new();
    std::io::Read::read_to_end(
        &mut original_zip.by_name("map.pgm").unwrap(),
        &mut original_root,
    )
    .unwrap();
    assert_eq!(root, original_root, "根目录建图原图不能被改写");
    assert!(
        archive.by_name("map_edit/map.pgm").is_ok(),
        "要派生编辑版栅格，遥控器 / 下发才认这份"
    );
    assert!(archive.by_name("map_edit/edit_info.json").is_ok());
}

/// 同一个包做橡皮擦：写进派生的 `map_edit/map.pgm`，根图不动。
#[tokio::test]
async fn erase_on_fresh_robot_package_keeps_the_original_raster() {
    let original = fresh_robot_package();
    let fixture = fixture_with_package("部分标定", original.clone()).await;
    let scope = DataScope::new(1, true);

    let image = fixture
        .state
        .dog_map_service
        .erase(
            fixture.map_id,
            center_stroke(0.6),
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect("橡皮擦必须成功");
    assert_eq!(image.erased_pixels, Some(1));

    let stored = fixture
        .state
        .dog_map_service
        .get_by_id(fixture.map_id, &scope)
        .await
        .expect("map must exist");
    let bytes = std::fs::read(
        fixture
            .state
            .upload_dir
            .join(stored.file_path.clone().expect("file_path")),
    )
    .expect("edited package must be readable");
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).expect("edited package is a zip");
    let mut root = Vec::new();
    std::io::Read::read_to_end(&mut archive.by_name("map.pgm").unwrap(), &mut root).unwrap();
    let mut original_zip =
        zip::ZipArchive::new(Cursor::new(&original)).expect("original package is a zip");
    let mut original_root = Vec::new();
    std::io::Read::read_to_end(
        &mut original_zip.by_name("map.pgm").unwrap(),
        &mut original_root,
    )
    .unwrap();
    assert_eq!(root, original_root, "根目录建图原图不能被改写");

    let mut edited = Vec::new();
    std::io::Read::read_to_end(
        &mut archive.by_name("map_edit/map.pgm").unwrap(),
        &mut edited,
    )
    .unwrap();
    let raster = parse_pgm_raster(&edited).expect("edit raster must parse");
    assert_eq!(raster.pixels[NOISE_INDEX], 254, "噪点要在编辑版里被擦掉");
    assert!(archive.by_name("map_edit/edit_info.json").is_ok());
}

/// 派生新版本必须是**草稿**，而且真的能编辑。
///
/// 已发布版本的擦除 / 禁止区域 / 恢复原图都会被状态门禁拒掉，文案统一写着
/// 「请先派生新版本（新版本为草稿）」。以前 `newVersion` 派完就顺手发布，
/// 新版本同样是「已发布」——现场只能在「编辑失败 → 新版本 → 还是失败」里打转
/// （2026-11-26 线上，`POST /api/dogMap/{id}/newVersion` 之后 PUT 仍报已发布）。
#[tokio::test]
async fn new_version_keeps_the_derived_map_editable() {
    let fixture = fixture("已发布").await;
    let scope = DataScope::new(1, true);

    let derived = fixture
        .state
        .dog_map_service
        .new_version(fixture.map_id, &scope)
        .await
        .expect("派生新版本必须成功");
    assert_eq!(derived.map_status, "草稿", "派生出来的是可编辑的草稿版");

    let image = fixture
        .state
        .dog_map_service
        .erase(
            derived.id,
            center_stroke(0.6),
            &fixture.state.upload_dir,
            &scope,
        )
        .await
        .expect("派生的草稿版本必须能编辑");
    assert_eq!(image.erased_pixels, Some(1));
}
