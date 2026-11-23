//! 地图模块的单元测试（路径解析 / 栅格解析 / 版本号）。

// 测的就是模块内部的 helper（`resolve_package_path` / `pgm_header` / `bump_version`），
// 走对外的 handler 反而看不到它们。
#[allow(clippy::wildcard_imports)]
use super::*;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("robodog-map-test-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 中台导入的地图：相对路径落在 `<backend_db>/device_uploads/` 下
#[test]
fn resolves_relative_package_under_device_uploads() {
    let dir = temp_dir("relative");
    let maps = dir.join("device_uploads/maps");
    std::fs::create_dir_all(&maps).unwrap();
    let package = maps.join("m.zip");
    std::fs::write(&package, b"PK").unwrap();

    let resolved = resolve_package_path(&dir.join("users.db"), "maps/m.zip")
        .expect("relative package must resolve");
    assert_eq!(resolved, package);
}

/// 设备回传 / 下发刷新后的地图：业务库存的是 ingest 目录里的绝对路径
#[test]
fn resolves_absolute_package_path() {
    let dir = temp_dir("absolute");
    let package = dir.join("stashed.zip");
    std::fs::write(&package, b"PK").unwrap();

    let resolved = resolve_package_path(&dir.join("users.db"), package.to_str().unwrap())
        .expect("absolute package must resolve");
    assert_eq!(resolved, package);
}

#[test]
fn missing_package_resolves_to_none() {
    let dir = temp_dir("missing");
    assert!(resolve_package_path(&dir.join("users.db"), "maps/absent.zip").is_none());
    assert!(resolve_package_path(&dir.join("users.db"), "   ").is_none());
}

#[test]
fn push_request_reads_map_id_and_request_id() {
    let payload: PushMapRequest =
        serde_json::from_str(r#"{"mapId":12,"requestId":"r-1"}"#).expect("push payload must parse");
    assert_eq!(payload.map_id, 12);
    assert_eq!(payload.request_id.as_deref(), Some("r-1"));

    let payload: PushMapRequest = serde_json::from_str(r#"{"mapId":7}"#).unwrap();
    assert_eq!(payload.map_id, 7);
    assert!(payload.request_id.is_none());
}
