//! 机器人建图地图回传（A 方案）。
//!
//! 设备客户端建图完成后，从本体取到 `.zip` 地图包，通过
//! `POST /api/map-versions/ingest` 交给本服务。本服务负责两跳：
//!
//! ```text
//! 1. 转发中台：POST {ROBODOG_ZENTAI_URL}/api/dogMap/upload
//!      → 中台解析 PGM/YAML、落盘、写入 dog_map，返回 dog_map.id
//! 2. 导入业务库：复用 import_backend_map 的逻辑
//!      → 写入 map_versions，可选绑定为园区基线
//! ```
//!
//! 中台账号只在服务端配置，不下发到每台机器狗。

use crate::db::now;
use crate::error::AppError;
use serde_json::{Value, json};
use std::path::Path;
use tracing::{info, warn};

/// 回传模式。
///
/// - [`IngestMode::Incremental`]：默认。`sourceRef` 命中且中台那张图还活着就跳过，
///   **只补平台没有的图**，绝不覆盖已有记录。设备定时轮询走这条。
/// - [`IngestMode::Refresh`]：中台/控制台显式触发「同步地图」时用，要求「全部更新到本体最新」，
///   因此不按 `sourceRef` 跳过；包内容与业务库副本完全一致时仍然跳过（省一次中台上传），
///   内容有变化则覆盖中台同一条 `dog_map`（`mapId` 原地替换，不新增副本）。
///   中台状态为「已发布 / 部分标定 / 已归档」的版本不可变，中台会拒绝覆盖，
///   这类图在同步报告里记为失败并给出原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestMode {
    Incremental,
    Refresh,
}

impl IngestMode {
    pub fn parse(raw: Option<&str>) -> Result<Self, AppError> {
        match raw.map(str::trim).filter(|value| !value.is_empty()) {
            None | Some("incremental") | Some("auto") | Some("onlyMissing") => {
                Ok(Self::Incremental)
            }
            Some("refresh") | Some("manual") | Some("full") => Ok(Self::Refresh),
            Some(other) => Err(AppError::BadRequest(format!(
                "未知的同步模式 {other}（支持 incremental / refresh）"
            ))),
        }
    }

    pub fn is_refresh(self) -> bool {
        matches!(self, Self::Refresh)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incremental => "incremental",
            Self::Refresh => "refresh",
        }
    }
}

/// 客户端提交的地图信息
#[derive(Debug, Clone)]
pub struct IngestRequest {
    pub map_name: String,
    /// 回传模式（`mode` 字段，缺省增量）
    pub mode: IngestMode,
    pub park_id: Option<i64>,
    /// 采集该地图的设备编号（业务库 `devices.device_id`）
    pub device_id: Option<String>,
    /// 导入后是否绑定为园区基线
    pub bind: bool,
    pub map_type: Option<String>,
    pub remarks: Option<String>,
    /// 来源标记，默认 `robot-mapping`
    pub source: Option<String>,
    /// 来源引用（本体侧地图路径），用于去重
    pub source_ref: Option<String>,
}

impl IngestRequest {
    pub fn from_json(value: &Value) -> Result<Self, AppError> {
        let map_name = value
            .get("mapName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::BadRequest("缺少 mapName".to_string()))?
            .to_string();

        let mode = IngestMode::parse(value.get("mode").and_then(Value::as_str))?;

        Ok(Self {
            map_name,
            mode,
            park_id: value.get("parkId").and_then(Value::as_i64),
            device_id: value
                .get("deviceId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            bind: value.get("bind").and_then(Value::as_bool).unwrap_or(false),
            map_type: value
                .get("mapType")
                .and_then(Value::as_str)
                .map(str::to_string),
            remarks: value
                .get("remarks")
                .and_then(Value::as_str)
                .map(str::to_string),
            source: value
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_string),
            source_ref: value
                .get("sourceRef")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// 中台返回的地图记录（只取回传需要的字段）
#[derive(Debug, Clone)]
pub struct ZentaiMap {
    pub id: i64,
    pub map_name: String,
    /// 中台记录上的版本号（`v1.0` / `v1.1`）。
    ///
    /// 中台为「已发布」地图新建版本时会在这里返回新版本号，业务平台要跟着用它，
    /// 否则业务库里一张 `v1.1` 的图会被写成 `v1.0`，版本号与中台对不上。
    pub map_version: Option<String>,
    pub file_name: Option<String>,
}

/// 地图归属园区（业务库记录），随 `mapInfo` 一起写入中台
#[derive(Debug, Clone)]
pub struct ParkRef {
    pub code: String,
    pub name: String,
}

/// 把地图包推入中台 `dog_map`
///
/// `park` 可以为空：地图先跟设备走，设备未绑定园区时中台的 `parkCode` /
/// `parkName` 留空，园区在中台地图管理里后续补登。
// 参数是「一次上传需要知道的事实」（谁、哪张图、哪个园区、覆盖哪条中台记录），
// 打包成结构体只会多一层搬运，保留平铺签名。
#[allow(clippy::too_many_arguments)]
pub async fn push_to_zentai(
    config: &crate::config::Config,
    http: &reqwest::Client,
    request: &IngestRequest,
    park: Option<&ParkRef>,
    file_name: &str,
    package: &[u8],
    owner_user_id: Option<i64>,
    // 刷新模式下要原地覆盖的中台 `dog_map.id`；`None` = 新建一条
    replace_id: Option<i64>,
) -> Result<ZentaiMap, AppError> {
    let base = crate::zentai::zentai_base(config)?;
    // 优先共享服务令牌（中台内部接口），否则退回服务账号登录
    let auth = crate::zentai::Auth::resolve(config).await?;

    // mapInfo：中台按包内容解析坐标系与哈希，这里只传展示字段
    // parkCode / parkName 让中台地图带上园区归属，业务平台后续可按园区反查地图
    let info = zentai_map_info(request, park, owner_user_id, replace_id);

    let part = reqwest::multipart::Part::bytes(package.to_vec())
        .file_name(file_name.to_string())
        .mime_str("application/zip")
        .map_err(|e| AppError::Internal(format!("构造上传体失败: {e}")))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("mapInfo", info.to_string());

    let url = if auth.is_service() {
        format!("{base}/api/dogInternal/maps/upload")
    } else {
        format!("{base}/api/dogMap/upload")
    };
    let resp = auth
        .apply(http.post(&url).multipart(form))
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("请求中台上传接口失败: {e}")))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // 刷新覆盖中台不可变版本（已发布 / 部分标定 / 已归档）时，把中台的原话带出去，
        // 现场才知道该「派生新版本」而不是反复点同步。
        if replace_id.is_some() {
            return Err(AppError::BadRequest(format!(
                "中台拒绝覆盖地图 #{}（HTTP {status}）：{}",
                replace_id.unwrap_or_default(),
                zentai_error_message(&text)
            )));
        }
        return Err(AppError::Internal(format!(
            "中台上传失败 HTTP {status}: {text}"
        )));
    }

    let value: Value = serde_json::from_str(&text)
        .map_err(|e| AppError::Internal(format!("中台响应不是 JSON: {e}")))
        .inspect_err(|_| warn!("中台响应原文: {text}"))?;

    let data = value.get("data").cloned().unwrap_or(Value::Null);
    let id = data
        .get("id")
        .and_then(Value::as_i64)
        .ok_or_else(|| AppError::Internal(format!("中台响应缺少 data.id: {text}")))?;
    let map_name = data
        .get("mapName")
        .and_then(Value::as_str)
        .unwrap_or(&request.map_name)
        .to_string();
    let file_name = data
        .get("fileName")
        .and_then(Value::as_str)
        .map(str::to_string);
    let map_version = data
        .get("mapVersion")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    info!("地图已上传中台: dog_map.id={id} name={map_name}");
    Ok(ZentaiMap {
        id,
        map_name,
        map_version,
        file_name,
    })
}

/// 机器人回传地图的基线版本。
///
/// 中台 `dog_map.map_version` 与业务库 `map_versions.version` 用同一个值，
/// 两库不会因为一处改字而漂移。
pub const ROBOT_MAP_VERSION: &str = "v1.0";

/// 组装上传中台的 `mapInfo`：地图展示字段 + 园区归属 + 上报设备。
///
/// 园区是业务库自己的台账，中台不建园区表，只落 `parkCode` / `parkName`，
/// 业务平台拉取中台地图时凭 `parkCode` 反查本园区。设备还没绑定园区时这两个
/// 字段为 `null`，中台地图就停在「未关联园区」，不阻断地图回传。
/// `deviceCode` 记录采集这张地图的设备编号，中台地图列表据此显示来源设备。
/// `mapVersion` 必须显式带上：不带时中台旧版本会落成 NULL，地图列表的「基线版本」
/// 列就是空的。
///
/// `id` 只在刷新（`mode=refresh`）时带：中台按这个 `dog_map.id` 原地覆盖同一条记录，
/// 避免每同步一次就在地图列表里多一份副本。字段名用中台 `UploadMapRequest` 的原生
/// `id`（不是 `mapId`）：旧版中台也认这个字段，不会因为字段名不匹配而静默新建副本。
/// 中台对「已发布 / 已归档」的版本会拒绝覆盖，错误原样带回业务平台。
pub fn zentai_map_info(
    request: &IngestRequest,
    park: Option<&ParkRef>,
    owner_user_id: Option<i64>,
    replace_id: Option<i64>,
) -> Value {
    json!({
        "mapName": request.map_name,
        "mapType": request.map_type.clone().unwrap_or_else(|| "2D栅格".to_string()),
        "remarks": request.remarks.clone().unwrap_or_else(|| "机器人建图自动回传".to_string()),
        "mapStatus": "部分标定",
        "mapVersion": ROBOT_MAP_VERSION,
        "parkCode": park.map(|park| park.code.clone()),
        "parkName": park.map(|park| park.name.clone()),
        "deviceCode": request.device_id.clone(),
        "ownerUserId": owner_user_id,
        // 刷新时中台按这个 id 原地覆盖（字段名与 `UploadMapRequest.id` 对齐）
        "id": replace_id,
    })
}

/// 从错误响应里抽出人话（中台多数返回 `{"code":..,"msg":".."}`）。
///
/// 拿不到就回退成截断后的原文：宁可多显示几个字符，也不要丢掉「为什么拒绝」。
fn zentai_error_message(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "（中台未返回错误信息）".to_string();
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && let Some(message) = value
            .get("msg")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|message| !message.is_empty())
    {
        return message.to_string();
    }
    let mut clipped: String = trimmed.chars().take(300).collect();
    if trimmed.chars().count() > 300 {
        clipped.push('…');
    }
    clipped
}

/// 地图包的**内容指纹**（与 zip 打包时间无关）。
///
/// 回传的 zip 是客户端现场打包的，每次压缩的时间戳都不一样，所以 package 级
/// 哈希不能用来判断「本体上的图变没变」。这里按文件名排序后把条目内容一起哈希：
/// 同样的 `model.pgm` + `model.yaml` 总是得到同一个指纹，客户端重打包多少次都一样。
pub fn package_fingerprint(package: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::io::Read;

    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(package)) else {
        // 解析不了就退回整包哈希，至少不会把 `None` 当成「没变化」
        return format!("raw:{}", sha256_hex(package));
    };
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        entries.push((name, bytes));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = DefaultHasher::new();
    for (name, bytes) in entries {
        name.hash(&mut hasher);
        bytes.len().hash(&mut hasher);
        bytes.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// 把上传的地图包落盘暂存（便于排障与重放）
pub async fn stash_package(
    ingest_dir: &Path,
    file_name: &str,
    package: &[u8],
) -> Result<std::path::PathBuf, AppError> {
    tokio::fs::create_dir_all(ingest_dir)
        .await
        .map_err(|e| AppError::Internal(format!("创建暂存目录失败: {e}")))?;
    let stamp = now().replace([' ', ':', '-'], "");
    let safe_name: String = file_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let path = ingest_dir.join(format!("{stamp}-{safe_name}"));
    tokio::fs::write(&path, package)
        .await
        .map_err(|e| AppError::Internal(format!("暂存地图包失败: {e}")))?;
    Ok(path)
}

/// 地图包条目是否位于 `map_edit/` 目录（遥控器编辑后的地图）。
///
/// 机器人本体同时保留两份栅格：根目录 `map.pgm`（建图原始地图）与
/// `map_edit/map.pgm`（遥控器编辑后的地图）。平台一律以 `map_edit/` 为准，
/// 否则预览、尺寸、下发都会拿回原始地图。
pub fn is_map_edit_path(name: &str) -> bool {
    name.split('/')
        .any(|segment| segment.eq_ignore_ascii_case("map_edit"))
}

/// 从 zip 条目名里选择地图文件：`map_edit/` 下的同扩展名条目优先，其次其他目录。
///
/// 只按扩展名判断，`map.yaml` / `model.yaml` / `*.yml` 都适用；返回的字符串
/// 借用自入参，调用方可以直接拿去 `ZipArchive::by_name`。
pub fn preferred_map_entry<'a, I>(names: I, extensions: &[&str]) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut fallback: Option<&'a str> = None;
    for name in names {
        let lower = name.to_ascii_lowercase();
        if !extensions
            .iter()
            .any(|extension| lower.ends_with(extension))
        {
            continue;
        }
        if is_map_edit_path(&lower) {
            return Some(name);
        }
        if fallback.is_none() {
            fallback = Some(name);
        }
    }
    fallback
}

/// 从地图包里解析坐标系信息，作为中台返回值的兜底
#[derive(Debug, Clone)]
pub struct LocalMeta {
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub origin: String,
    pub resolution: String,
}

/// 解析 zip 内的栅格（取尺寸）与 yaml（取分辨率/原点）
///
/// 优先使用 `map_edit/` 下的编辑版；没有编辑版时才退回 `model.pgm` / `map.pgm`。
pub fn parse_local_meta(package: &[u8]) -> LocalMeta {
    use std::io::{Cursor, Read};

    let mut meta = LocalMeta {
        sha256: sha256_hex(package),
        width: 0,
        height: 0,
        origin: "0.0, 0.0, 0.0".to_string(),
        resolution: "0.05".to_string(),
    };

    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(package)) else {
        return meta;
    };

    let mut names: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if !entry.is_dir() {
            names.push(entry.name().to_string());
        }
    }

    // 先找 yaml，再找 pgm（两者可能不在同一层目录）
    if let Some(name) = preferred_map_entry(names.iter().map(String::as_str), &[".yaml", ".yml"]) {
        let mut text = String::new();
        if let Ok(mut entry) = archive.by_name(name)
            && Read::read_to_string(&mut entry, &mut text).is_ok()
        {
            for line in text.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("resolution:") {
                    let value = rest.trim();
                    if !value.is_empty() {
                        meta.resolution = value.to_string();
                    }
                } else if let Some(rest) = line.strip_prefix("origin:") {
                    let cleaned = rest.trim().trim_start_matches('[').trim_end_matches(']');
                    if !cleaned.is_empty() {
                        meta.origin = cleaned.to_string();
                    }
                }
            }
        }
    }

    if let Some(name) = preferred_map_entry(names.iter().map(String::as_str), &[".pgm"]) {
        let mut header = Vec::new();
        let mut chunk = [0u8; 256];
        // PGM header 只需要前若干字节
        if let Ok(mut entry) = archive.by_name(name) {
            while header.len() < 256 {
                match entry.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => header.extend_from_slice(&chunk[..n]),
                    Err(_) => break,
                }
            }
        }
        if let Some((w, h)) = parse_pgm_size(&header) {
            meta.width = w;
            meta.height = h;
        }
    }

    meta
}

/// 从 PGM 头里读宽高（跳过注释行）
fn parse_pgm_size(header: &[u8]) -> Option<(u32, u32)> {
    let text = String::from_utf8_lossy(header);
    let mut tokens = text
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .flat_map(str::split_whitespace);
    let magic = tokens.next()?;
    if magic != "P5" && magic != "P2" {
        return None;
    }
    let width = tokens.next()?.parse().ok()?;
    let height = tokens.next()?.parse().ok()?;
    Some((width, height))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    // 复用中台同款实现：直接用 zip 包内容做指纹
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_binary_pgm_header() {
        let header = b"P5\n1024 768\n255\n";
        assert_eq!(parse_pgm_size(header), Some((1024, 768)));
    }

    #[test]
    fn parses_ascii_pgm_header() {
        let header = b"P2\n800 600\n255\n";
        assert_eq!(parse_pgm_size(header), Some((800, 600)));
    }

    #[test]
    fn skips_comment_lines() {
        let header = b"P5\n# CREATOR: cartographer\n# another comment\n640 480\n255\n";
        assert_eq!(parse_pgm_size(header), Some((640, 480)));
    }

    #[test]
    fn rejects_unknown_magic() {
        assert_eq!(parse_pgm_size(b"P6\n640 480\n255\n"), None);
        assert_eq!(parse_pgm_size(b"garbage"), None);
    }

    #[test]
    fn rejects_truncated_header_without_dimensions() {
        assert_eq!(parse_pgm_size(b"P5\n"), None);
        assert_eq!(parse_pgm_size(b"P5\n640\n"), None);
    }

    fn sample_request(park_id: Option<i64>) -> IngestRequest {
        IngestRequest {
            map_name: "B区-20261110".to_string(),
            mode: IngestMode::Incremental,
            park_id,
            device_id: Some("RD-001".to_string()),
            bind: true,
            map_type: None,
            remarks: None,
            source: None,
            source_ref: None,
        }
    }

    #[test]
    fn map_info_carries_park_code_and_name() {
        let request = sample_request(Some(3));
        let park = ParkRef {
            code: "PK-003".to_string(),
            name: "B区园区".to_string(),
        };
        let info = zentai_map_info(&request, Some(&park), Some(7), None);
        assert_eq!(info["parkCode"], "PK-003");
        assert_eq!(info["parkName"], "B区园区");
        assert_eq!(info["mapVersion"], ROBOT_MAP_VERSION);
        assert_eq!(info["deviceCode"], "RD-001");
        assert_eq!(info["mapName"], "B区-20261110");
        assert_eq!(info["mapStatus"], "部分标定");
        assert_eq!(info["ownerUserId"], 7);
    }

    #[test]
    fn map_info_without_park_leaves_park_fields_null() {
        // 设备未绑定园区也允许回传：中台 dog_map 停在「未关联园区」
        let request = sample_request(None);
        let info = zentai_map_info(&request, None, None, None);
        assert_eq!(info["parkCode"], Value::Null);
        assert_eq!(info["parkName"], Value::Null);
        assert_eq!(info["deviceCode"], "RD-001");
    }

    #[test]
    fn ingest_request_reads_park_id_and_device_id_from_json() {
        let value = json!({"mapName": "B区图", "parkId": 3, "deviceId": "RD-001"});
        let request = IngestRequest::from_json(&value).expect("mapInfo must parse");
        assert_eq!(request.park_id, Some(3));
        assert_eq!(request.device_id.as_deref(), Some("RD-001"));
        assert_eq!(request.mode, IngestMode::Incremental);

        let value = json!({"mapName": "未绑定设备图"});
        let request = IngestRequest::from_json(&value).expect("mapInfo must parse");
        assert_eq!(request.park_id, None);
        assert_eq!(request.mode, IngestMode::Incremental);
    }

    #[test]
    fn ingest_mode_parses_and_defaults_to_incremental() {
        assert_eq!(IngestMode::parse(None).unwrap(), IngestMode::Incremental);
        assert_eq!(
            IngestMode::parse(Some("refresh")).unwrap(),
            IngestMode::Refresh
        );
        assert_eq!(
            IngestMode::parse(Some("auto")).unwrap(),
            IngestMode::Incremental
        );
        assert!(IngestMode::parse(Some("whatever")).is_err());
    }

    #[test]
    fn map_info_carries_replace_id_only_when_asked() {
        let request = sample_request(None);
        assert_eq!(
            zentai_map_info(&request, None, None, None)["id"],
            Value::Null
        );
        assert_eq!(
            zentai_map_info(&request, None, None, Some(42))["id"],
            json!(42)
        );
    }

    #[test]
    fn package_fingerprint_ignores_zip_timestamps() {
        fn zip_with(mtime: zip::DateTime) -> Vec<u8> {
            use std::io::Write;
            let mut buffer = std::io::Cursor::new(Vec::new());
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default().last_modified_time(mtime);
            writer.start_file("model.pgm", options).unwrap();
            writer.write_all(b"P5\n4 4\n255\n\x00\x01\x02\x03").unwrap();
            writer.start_file("model.yaml", options).unwrap();
            writer.write_all(b"resolution: 0.05\n").unwrap();
            writer.finish().unwrap();
            buffer.into_inner()
        }

        let early = zip_with(zip::DateTime::from_date_and_time(2024, 1, 1, 0, 0, 0).unwrap());
        let late = zip_with(zip::DateTime::from_date_and_time(2026, 11, 10, 16, 15, 6).unwrap());
        assert_ne!(early, late, "打包时间不同，整包字节必然不同");
        assert_eq!(
            package_fingerprint(&early),
            package_fingerprint(&late),
            "内容指纹不能受 zip 时间戳影响"
        );

        let mut changed = early.clone();
        changed.extend_from_slice(b"trailing");
        assert_eq!(package_fingerprint(&early), package_fingerprint(&changed));
    }

    #[test]
    fn prefers_map_edit_entries_and_ignores_backup_dirs() {
        let names = [
            "map.pgm",
            "map.yaml",
            "map_edit/map.pgm",
            "map_edit/map.yaml",
            "map_edit.bak/map.pgm",
        ];
        assert_eq!(
            preferred_map_entry(names.iter().copied(), &[".pgm"]),
            Some("map_edit/map.pgm")
        );
        assert_eq!(
            preferred_map_entry(names.iter().copied(), &[".yaml", ".yml"]),
            Some("map_edit/map.yaml")
        );
        assert!(is_map_edit_path("map_edit/map.pgm"));
        assert!(!is_map_edit_path("map_edit.bak/map.pgm"));

        // 没有编辑版时退回第一个匹配项，保持旧行为
        let flat = ["model.pgm", "model.yaml"];
        assert_eq!(
            preferred_map_entry(flat.iter().copied(), &[".pgm"]),
            Some("model.pgm")
        );
    }

    #[test]
    fn local_meta_prefers_map_edit() {
        use std::io::Write;

        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("map.pgm", options).unwrap();
            writer
                .write_all(b"P5\n4 3\n255\n\0\0\0\0\0\0\0\0\0\0\0\0")
                .unwrap();
            writer.start_file("map.yaml", options).unwrap();
            writer
                .write_all(b"resolution: 0.05\norigin: [-8.7, -17.7, 0.0]\n")
                .unwrap();
            writer.start_file("map_edit/map.pgm", options).unwrap();
            writer.write_all(b"P5\n2 1\n255\n\0\xfe").unwrap();
            writer.start_file("map_edit/map.yaml", options).unwrap();
            writer
                .write_all(b"resolution: 0.1\norigin: [1, 2, 0]\n")
                .unwrap();
            writer.finish().unwrap();
        }

        let meta = parse_local_meta(&buffer.into_inner());
        assert_eq!(meta.width, 2);
        assert_eq!(meta.height, 1);
        assert_eq!(meta.resolution, "0.1");
        assert_eq!(meta.origin, "1, 2, 0");
    }
}
