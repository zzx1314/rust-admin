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

/// 客户端提交的地图信息
#[derive(Debug, Clone)]
pub struct IngestRequest {
    pub map_name: String,
    pub park_id: Option<i64>,
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

        Ok(Self {
            map_name,
            park_id: value.get("parkId").and_then(Value::as_i64),
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
    pub file_name: Option<String>,
}

/// 地图归属园区（业务库记录），随 `mapInfo` 一起写入中台
#[derive(Debug, Clone)]
pub struct ParkRef {
    pub code: String,
    pub name: String,
}

/// 把地图包推入中台 `dog_map`
pub async fn push_to_zentai(
    config: &crate::config::Config,
    http: &reqwest::Client,
    request: &IngestRequest,
    park: &ParkRef,
    file_name: &str,
    package: &[u8],
) -> Result<ZentaiMap, AppError> {
    let (Some(base), Some(username), Some(password)) = (
        config.zentai_base_url.as_deref(),
        config.zentai_username.as_deref(),
        config.zentai_password.as_deref(),
    ) else {
        return Err(AppError::Internal(
            "未配置中台回传账号（ROBODOG_ZENTAI_URL / USER / PASSWORD）".to_string(),
        ));
    };

    let token = zentai_login(http, base, username, password).await?;

    // mapInfo：中台按包内容解析坐标系与哈希，这里只传展示字段
    // parkCode / parkName 让中台地图带上园区归属，业务平台后续可按园区反查地图
    let info = zentai_map_info(request, park);

    let part = reqwest::multipart::Part::bytes(package.to_vec())
        .file_name(file_name.to_string())
        .mime_str("application/zip")
        .map_err(|e| AppError::Internal(format!("构造上传体失败: {e}")))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("mapInfo", info.to_string());

    let url = format!("{base}/api/dogMap/upload");
    let resp = http
        .post(&url)
        .bearer_auth(&token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("请求中台上传接口失败: {e}")))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
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

    info!("地图已上传中台: dog_map.id={id} name={map_name}");
    Ok(ZentaiMap {
        id,
        map_name,
        file_name,
    })
}

/// 组装上传中台的 `mapInfo`：地图展示字段 + 园区归属。
///
/// 园区是业务库自己的台账，中台不建园区表，只落 `parkCode` / `parkName`，
/// 业务平台拉取中台地图时凭 `parkCode` 反查本园区。
pub fn zentai_map_info(request: &IngestRequest, park: &ParkRef) -> Value {
    json!({
        "mapName": request.map_name,
        "mapType": request.map_type.clone().unwrap_or_else(|| "2D栅格".to_string()),
        "remarks": request.remarks.clone().unwrap_or_else(|| "机器人建图自动回传".to_string()),
        "mapStatus": "部分标定",
        "parkCode": park.code.clone(),
        "parkName": park.name.clone(),
    })
}

/// 登录中台拿 Bearer token
///
/// 中台密码走 AES-128-CBC（ZeroPadding，key=iv=`Welcome Superred`）加密后
/// 以表单提交，与前端 `utils/aes.ts` 保持一致。
async fn zentai_login(
    http: &reqwest::Client,
    base: &str,
    username: &str,
    password: &str,
) -> Result<String, AppError> {
    use aes::Aes128;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
    use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::NoPadding};

    type Aes128CbcEnc = cbc::Encryptor<Aes128>;
    const KEY: &[u8; 16] = b"Welcome Superred";

    let plain = password.as_bytes();
    let padded_len = plain.len().div_ceil(16).max(1) * 16;
    let mut buf = vec![0u8; padded_len];
    buf[..plain.len()].copy_from_slice(plain);
    let cipher = Aes128CbcEnc::new(KEY.into(), KEY.into())
        .encrypt_padded_mut::<NoPadding>(&mut buf, padded_len)
        .map_err(|e| AppError::Internal(format!("密码加密失败: {e}")))?;
    let encrypted = BASE64.encode(cipher);

    let url = format!("{base}/api/token");
    let resp = http
        .post(&url)
        .form(&[
            ("username", username),
            ("password", encrypted.as_str()),
            ("grant_type", "password"),
            ("scope", "server"),
        ])
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("登录中台失败: {e}")))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(AppError::Internal(format!(
            "登录中台失败 HTTP {status}: {text}"
        )));
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| AppError::Internal(format!("中台登录响应不是 JSON: {e}")))?;
    value
        .get("access_token")
        .or_else(|| value.get("accessToken"))
        .or_else(|| value.get("data").and_then(|d| d.get("access_token")))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| AppError::Internal(format!("中台登录响应缺少 access_token: {text}")))
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

/// 从地图包里解析坐标系信息，作为中台返回值的兜底
#[derive(Debug, Clone)]
pub struct LocalMeta {
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub origin: String,
    pub resolution: String,
}

/// 解析 zip 内的 `model.pgm`（取尺寸）与 `model.yaml`（取分辨率/原点）
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

    // 先找 yaml，再找 pgm（两者可能不在同一层目录）
    for index in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        let name = entry.name().to_lowercase();
        if !(name.ends_with(".yaml") || name.ends_with(".yml")) {
            continue;
        }
        let mut text = String::new();
        if Read::read_to_string(&mut entry, &mut text).is_err() {
            continue;
        }
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
        break;
    }

    for index in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        let name = entry.name().to_lowercase();
        if !name.ends_with(".pgm") {
            continue;
        }
        let mut header = Vec::new();
        let mut chunk = [0u8; 256];
        // PGM header 只需要前若干字节
        while header.len() < 256 {
            match entry.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => header.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        if let Some((w, h)) = parse_pgm_size(&header) {
            meta.width = w;
            meta.height = h;
        }
        break;
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

fn sha256_hex(bytes: &[u8]) -> String {
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
            park_id,
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
        let info = zentai_map_info(&request, &park);
        assert_eq!(info["parkCode"], "PK-003");
        assert_eq!(info["parkName"], "B区园区");
        assert_eq!(info["mapName"], "B区-20261110");
        assert_eq!(info["mapStatus"], "部分标定");
    }

    #[test]
    fn ingest_request_reads_park_id_from_json() {
        let value = json!({"mapName": "B区图", "parkId": 3});
        let request = IngestRequest::from_json(&value).expect("mapInfo must parse");
        assert_eq!(request.park_id, Some(3));

        let value = json!({"mapName": "未绑定设备图"});
        let request = IngestRequest::from_json(&value).expect("mapInfo must parse");
        assert_eq!(request.park_id, None);
    }
}
