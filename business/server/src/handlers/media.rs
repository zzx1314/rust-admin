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
//! 请求为 multipart：
//! - `file`：一张 JPG/PNG（必填，≤ 8MB）
//! - `mediaInfo`：`{ deviceId, planId, pointIndex, pointName, capturedAt, source, remarks }`
//!
//! 中台按图片 `sha256` 去重，返回 `created=false` 表示这张图之前已经传过 ——
//! 客户端据此照常删除板端文件（重传一次不会多出一行记录）。

use crate::app::AppState;
use crate::error::{ApiResponse, AppError};
use crate::zentai::{self, Auth};
use axum::Json;
use axum::extract::{Multipart, State};
use serde_json::Value;
use tracing::{info, warn};

/// 单张图片上限：与中台 `MEDIA_IMAGE_MAX_BYTES` 保持一致。
const MEDIA_IMAGE_MAX_BYTES: usize = 8 * 1024 * 1024;

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
        return Err(AppError::BadRequest("上传的图片为空".to_string()));
    }
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
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|error| AppError::Internal(format!("构造 HTTP 客户端失败: {error}")))?;

    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
        .file_name(file_name.to_string())
        .mime_str(if file_name.to_ascii_lowercase().ends_with(".png") {
            "image/png"
        } else {
            "image/jpeg"
        })
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
