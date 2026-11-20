//! `POST /api/media/ingest` — 设备客户端回传板端 AI 存证图片。
//!
//! 链路（与建图回传同构，中台凭证只在业务服务端）：
//!
//! ```text
//! 板端 /media/media-data/picture/<planId>/pt<n>/*.jpg
//!   └─ 中台设备页「同步媒体」→ 业务平台 → 设备 WS（command=同步媒体）
//!        └─ 客户端读图 → POST /api/media/ingest（本接口）
//!             └─ 转发中台 POST /api/dogInternal/media/upload → dog_media
//!                  └─ 中台「媒体资源」页展示（GET /api/dogMedia/{id}/image）
//! ```
//!
//! 请求为 multipart，`file` 两种形态都支持：
//!
//! - **压缩包（客户端现行方式）**：`file = media.zip`（≤ 64MB），
//!   `mediaInfo = { deviceCode, source, entries: [{ name, planId, pointIndex, capturedAt, remarks }] }`；
//! - **单张（老客户端 / 手工联调）**：`file = 一张 JPG/PNG`（≤ 8MB），
//!   `mediaInfo = { deviceCode, planId, pointIndex, capturedAt, source, remarks }`。
//!
//! 业务平台只做「验大小 + 补设备/园区 + 原样转发」，解包与逐张入库在中台
//! （`/api/dogInternal/media/upload`）：中台按 `sha256` 去重并回 `results[]`，
//! 客户端只删 `ok` 的条目（`created=false` 也算成功）。

use crate::app::AppState;
use crate::error::{ApiResponse, AppError};
use crate::zentai::{self, Auth};
use axum::Json;
use axum::extract::{Multipart, State};
use serde_json::Value;
use tracing::{info, warn};

/// 单张图片上限：与中台 `MEDIA_IMAGE_MAX_BYTES` 保持一致。
const MEDIA_IMAGE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// 压缩包上限：客户端按「每包 8 张 / 8MB」打包，这里留足余量挡住误传大包。
const MEDIA_ZIP_MAX_BYTES: usize = 64 * 1024 * 1024;

pub async fn ingest_media(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let mut info_json: Option<Value> = None;
    let mut file: Option<(String, Vec<u8>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::BadRequest(format!("无效的上传请求: {error}")))?
    {
        match field.name() {
            Some("file") => {
                let name = field.file_name().unwrap_or("frame.jpg").to_string();
                let data = field
                    .bytes()
                    .await
                    .map_err(|error| AppError::BadRequest(format!("读取图片失败: {error}")))?;
                file = Some((name, data.to_vec()));
            }
            Some("mediaInfo") => {
                let text = field
                    .text()
                    .await
                    .map_err(|error| AppError::BadRequest(format!("读取图片信息失败: {error}")))?;
                info_json =
                    Some(serde_json::from_str(&text).map_err(|error| {
                        AppError::BadRequest(format!("图片信息格式错误: {error}"))
                    })?);
            }
            _ => {}
        }
    }

    let mut info = info_json.ok_or_else(|| AppError::BadRequest("缺少 mediaInfo".to_string()))?;
    let (file_name, bytes) =
        file.ok_or_else(|| AppError::BadRequest("请上传图片文件".to_string()))?;

    if bytes.is_empty() {
        return Err(AppError::BadRequest("上传的媒体为空".to_string()));
    }
    // 先看文件头再判形态：zip（压缩包）走批量，JPG/PNG 走单张（老客户端）
    let is_zip = bytes.starts_with(b"PK\x03\x04");
    if is_zip {
        if bytes.len() > MEDIA_ZIP_MAX_BYTES {
            return Err(AppError::BadRequest("压缩包超过 64MB 上限".to_string()));
        }
        let entries = info
            .get("entries")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if entries == 0 {
            return Err(AppError::BadRequest(
                "压缩包回传缺少 mediaInfo.entries（每张图的计划/航点信息）".to_string(),
            ));
        }
    } else {
        if bytes.len() > MEDIA_IMAGE_MAX_BYTES {
            return Err(AppError::BadRequest("图片超过 8MB 上限".to_string()));
        }
        // 只放 JPG/PNG；扩展名先挡一层，文件头再复核（客户端不可信）
        let lower = file_name.to_ascii_lowercase();
        let is_png = lower.ends_with(".png");
        if !is_png && !lower.ends_with(".jpg") && !lower.ends_with(".jpeg") {
            return Err(AppError::BadRequest("媒体回传仅支持 JPG/PNG".to_string()));
        }
        if is_png {
            if !bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
                return Err(AppError::BadRequest("文件内容不是有效的 PNG".to_string()));
            }
        } else if !bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Err(AppError::BadRequest("文件内容不是有效的 JPG".to_string()));
        }
    }

    // 设备编号/来源以客户端上报为准，业务平台不替它猜（中台只做展示与去重）。
    // 中台内部接口按 `deviceCode` 落库（与 `dog_map.device_code` 同名），
    // 客户端/老版本可能只给 `deviceId`，这里统一补齐两份。
    let device_code = info
        .get("deviceCode")
        .or_else(|| info.get("deviceId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if let Some(code) = &device_code {
        info["deviceCode"] = Value::String(code.clone());
        info["deviceId"] = Value::String(code.clone());
    } else {
        return Err(AppError::BadRequest(
            "mediaInfo 缺少设备编号（deviceCode）".to_string(),
        ));
    }
    // 园区归属：设备在业务平台绑了园区就顺手带上，中台媒体资源里能直接看出是哪片园区
    // （客户端不配园区，避免板端与平台口径不一致）
    if info.get("parkCode").map(Value::is_null).unwrap_or(true)
        && let Some(device_id) = device_code.as_deref()
    {
        let park: Option<(String,)> = sqlx::query_as(
            "SELECT p.park_code FROM devices d JOIN parks p ON p.id = d.park_id \
             WHERE d.device_id = ?1",
        )
        .bind(device_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        if let Some((park_code,)) = park {
            info["parkCode"] = Value::String(park_code);
        }
    }

    let value = forward_to_zentai(&state, &file_name, &bytes, &info).await?;
    if is_zip {
        // 批量：中台回 { total, failed, results: [{ name, ok, created, mediaId, error }] }
        let total = value
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let failed = value
            .get("failed")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let device = info
            .get("deviceCode")
            .and_then(Value::as_str)
            .unwrap_or("-");
        if failed == 0 {
            info!(device, total, file = %file_name, "媒体压缩包已入库");
        } else {
            warn!(device, total, failed, file = %file_name, "媒体压缩包部分条目入库失败");
        }
    } else {
        let created = value
            .get("created")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if created {
            let name = value
                .get("media")
                .and_then(|media| media.get("mediaName"))
                .and_then(Value::as_str)
                .unwrap_or("媒体");
            info!(media = %name, device = %info.get("deviceId").and_then(|value| value.as_str()).unwrap_or("-"),
                   "图片已回传中台媒体资源");
        } else {
            info!(file = %file_name, "图片已存在（sha256 去重），未新增媒体记录");
        }
    }
    Ok(ApiResponse::ok(value))
}

/// 转发中台 `/api/dogInternal/media/upload`（multipart：file + mediaInfo）。
async fn forward_to_zentai(
    state: &AppState,
    file_name: &str,
    bytes: &[u8],
    info: &Value,
) -> Result<Value, AppError> {
    let base = zentai::zentai_base(&state.config)?;
    let auth = Auth::resolve(&state.config).await?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|error| AppError::Internal(format!("构造 HTTP 客户端失败: {error}")))?;

    let mime = if file_name.to_ascii_lowercase().ends_with(".zip") {
        "application/zip"
    } else if file_name.to_ascii_lowercase().ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    };
    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
        .file_name(file_name.to_string())
        .mime_str(mime)
        .map_err(|error| AppError::Internal(format!("构造上传体失败: {error}")))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("mediaInfo", info.to_string());

    let url = if auth.is_service() {
        format!("{base}/api/dogInternal/media/upload")
    } else {
        // 中台没配共享令牌时退化为服务账号登录公开接口（与地图回传同样兜底）
        format!("{base}/api/dogMedia/upload")
    };
    let resp = auth
        .apply(http.post(&url).multipart(form))
        .send()
        .await
        .map_err(|error| AppError::Internal(format!("请求中台上传接口失败: {error}")))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        warn!(%status, "中台媒体上传失败: {text}");
        return Err(AppError::Internal(format!(
            "中台上传失败 HTTP {status}: {text}"
        )));
    }
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| AppError::Internal(format!("中台响应解析失败: {error}")))?;
    Ok(value.get("data").cloned().unwrap_or(value))
}
