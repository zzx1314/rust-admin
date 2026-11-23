//! 中台内部接口：给业务平台用的「服务身份」通道。
//!
//! 业务平台同步设备台账、查询分配关系、回传机器人地图时，原本要拿一个中台
//! 管理员账号登录换 JWT。那样有两个持续成本：口令以明文躺在业务平台配置里，
//! 且 10 分钟会话过期 / Redis 抖动都会让回传失败。
//!
//! 这里把这三件事收敛成共享服务令牌（`x-service-token`）鉴权的内部接口：
//!
//! ```text
//! GET    /api/dogInternal/devices/assignments   设备台账 + 分配关系（批量）
//! POST   /api/dogInternal/devices/upsert        设备台账幂等同步
//! DELETE /api/dogInternal/devices/{device_code} 删除设备台账
//! POST   /api/dogInternal/maps/exists           查询地图是否仍存在（业务平台判重）
//! POST   /api/dogInternal/maps/upload           地图包回传（multipart）
//! GET    /api/dogInternal/media/config          板端媒体目录 / 固定帧数（参数配置页）
//! POST   /api/dogInternal/media/upload          航点 AI 存证图片回传（multipart：单张或 zip）
//! POST   /api/dogInternal/media/sync-status     「同步媒体」进度 / 结论回报（写指令审计）
//! ```
//!
//! 与公开接口的关系：复用同一批 service，只是身份从「JWT 用户 + 行级权限」
//! 换成「管理员范围 + 显式 `ownerUserId` / `creatorUserId`」。公开接口一行没改。

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::HeaderMap;
use axum::routing::{delete, get, post};
use serde::Deserialize;
use serde_json::json;

use crate::api::AppState;
use crate::business::dog::device::control_channel::SERVICE_TOKEN_HEADER;
use crate::business::dog::device::domain::{CreateDeviceRequest, DeviceFilter};
use crate::business::dog::map::domain::UploadMapRequest;
use crate::business::dog::media::domain::{UploadMediaBatch, UploadMediaEntry, UploadMediaRequest};
use crate::common::data_scope::DataScope;
use crate::common::error::{ApiResponse, AppError};
use std::collections::HashMap;
use std::io::{Cursor, Read};

/// 与 `/api/dogMap/upload` 保持一致的地图包上限。
const MAP_UPLOAD_LIMIT: usize = 256 * 1024 * 1024;
/// 媒体压缩包上限（与业务平台 `MEDIA_ZIP_MAX_BYTES` 同值）。
const MEDIA_ZIP_MAX_BYTES: usize = 64 * 1024 * 1024;
/// 一个压缩包里最多接受多少张：挡住「把整个目录打成包」的误用。
const MEDIA_BATCH_MAX_ENTRIES: usize = 100;

/// 内部接口的数据范围：管理员范围（不做归属过滤）。
///
/// `creator_user_id` 为设备台账的创建者；0 表示「由业务平台同步」，不是真实用户。
fn internal_scope(creator_user_id: Option<i64>) -> DataScope {
    DataScope::new(creator_user_id.unwrap_or(0), true)
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let expected = state
        .business_api_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(
                "未配置业务平台服务令牌（APP_DOG__BUSINESS_API_TOKEN），内部接口不可用".to_string(),
            )
        })?;
    let provided = headers
        .get(SERVICE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if provided.trim() != expected {
        return Err(AppError::Unauthorized("服务令牌无效".to_string()));
    }
    Ok(())
}

/// 内部路由：挂在 `/api` 下，不走 `require_auth`。
pub fn internal_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/dogInternal/devices/assignments",
            get(list_assignments_handler),
        )
        .route("/dogInternal/devices/upsert", post(upsert_device_handler))
        .route(
            "/dogInternal/devices/{device_code}",
            delete(delete_device_handler),
        )
        .route("/dogInternal/maps/exists", post(map_exists_handler))
        .route("/dogInternal/media/config", get(media_config_handler))
        .route(
            "/dogInternal/media/sync-status",
            post(media_sync_status_handler),
        )
        .route(
            "/dogInternal/maps/upload",
            post(upload_map_handler).layer(DefaultBodyLimit::max(MAP_UPLOAD_LIMIT)),
        )
        .route(
            "/dogInternal/media/upload",
            post(upload_media_handler).layer(DefaultBodyLimit::max(
                crate::business::dog::media::service::MEDIA_IMAGE_MAX_BYTES + 64 * 1024,
            )),
        )
}

/// `GET /api/dogInternal/devices/assignments`
///
/// 返回结构刻意与 `/api/dogDevice/getAll` 一致（`DeviceVO` 数组），
/// 业务平台的解析逻辑不用分叉。
pub async fn list_assignments_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    authorize(&state, &headers)?;
    let devices = state
        .dog_device_service
        .list_all(DeviceFilter::default(), &internal_scope(None))
        .await?;
    Ok(Json(ApiResponse::ok(
        serde_json::to_value(devices).unwrap_or_default(),
    )))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InternalDeviceUpsertRequest {
    #[serde(flatten)]
    pub device: CreateDeviceRequest,
    /// 台账创建者；业务平台通常不传（记为 0 = 平台同步）
    #[serde(default)]
    pub creator_user_id: Option<i64>,
}

/// `POST /api/dogInternal/devices/upsert` — 按设备编号幂等写入台账。
pub async fn upsert_device_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<InternalDeviceUpsertRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    authorize(&state, &headers)?;
    let device = state
        .dog_device_service
        .upsert(req.device, &internal_scope(req.creator_user_id))
        .await?;
    Ok(Json(ApiResponse::ok(
        serde_json::to_value(device).unwrap_or_default(),
    )))
}

/// `DELETE /api/dogInternal/devices/{device_code}` — 删除台账（幂等）。
pub async fn delete_device_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_code): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    authorize(&state, &headers)?;
    let Some(device) = state
        .dog_device_service
        .get_vo_by_code(&device_code)
        .await?
    else {
        // 已经不存在：业务平台的重试不该报错
        return Ok(Json(ApiResponse::ok(json!({"deleted": false}))));
    };
    state
        .dog_device_service
        .delete(device.id, &internal_scope(None))
        .await?;
    Ok(Json(ApiResponse::ok(json!({"deleted": true}))))
}

/// `POST /api/dogInternal/maps/exists` — 中台哪些地图记录还活着。
///
/// 业务平台用它修正「业务库已导入、中台已删除」的判定：这类地图不能算已同步，
/// 否则设备端永远不会重传，中台列表里也看不到。只返回 id，不泄漏地图字段。
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MapExistsRequest {
    #[serde(default)]
    pub ids: Vec<i64>,
}

pub async fn map_exists_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MapExistsRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    authorize(&state, &headers)?;
    let existing = state.dog_map_service.alive_ids(&req.ids).await?;
    Ok(Json(ApiResponse::ok(json!({ "existing": existing }))))
}

/// `POST /api/dogInternal/maps/upload` — 地图包回传（multipart：file + mapInfo）。
///
/// `mapInfo.ownerUserId` 指定地图归属用户（设备被分配的中台用户）；
/// 未指定时记在 `user_id = 0`（平台同步）。
pub async fn upload_map_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    authorize(&state, &headers)?;

    let mut info: Option<UploadMapRequest> = None;
    let mut file: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::BadRequest(format!("无效的上传请求: {error}")))?
    {
        match field.name() {
            Some("file") => {
                let name = field.file_name().unwrap_or("map.zip").to_string();
                let data = field
                    .bytes()
                    .await
                    .map_err(|error| AppError::BadRequest(format!("读取上传文件失败: {error}")))?;
                file = Some((name, data.to_vec()));
            }
            Some("mapInfo") => {
                let text = field
                    .text()
                    .await
                    .map_err(|error| AppError::BadRequest(format!("读取地图信息失败: {error}")))?;
                info =
                    Some(serde_json::from_str(&text).map_err(|error| {
                        AppError::BadRequest(format!("地图信息格式错误: {error}"))
                    })?);
            }
            _ => {}
        }
    }

    let info = info.ok_or_else(|| AppError::BadRequest("缺少 mapInfo".to_string()))?;
    let (file_name, bytes) =
        file.ok_or_else(|| AppError::BadRequest("请上传地图包文件".to_string()))?;

    let map = state
        .dog_map_service
        .save_upload(
            info,
            &file_name,
            &bytes,
            &state.upload_dir,
            &internal_scope(None),
        )
        .await?;
    let vo = state.dog_map_service.to_vo(map).await?;
    Ok(Json(ApiResponse::ok(
        serde_json::to_value(vo).unwrap_or_default(),
    )))
}

/// `POST /api/dogInternal/media/sync-status` — 「同步媒体」的进度与最终结论回报。
///
/// 链路：设备客户端（`command_progress` / `command_result`）→ 业务平台 →
/// 本接口 → `dog_command_audit`。为什么要绕这一圈：中台只负责下发，「扫了几张、
/// 传了几张、板端目录清没清」只有板端知道；不回写的话设备页只能看到
/// 「已下发」，现场无法判断同步有没有完成。
///
/// `requestId` 必须与中台下发时的一致（中台把它放在指令 `params.requestId`），
/// 且设备编号 / 指令名必须对得上——内部令牌虽然是服务级，也不该能改别的设备。
pub async fn media_sync_status_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MediaSyncStatusRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    authorize(&state, &headers)?;
    let request_id = req.request_id.trim();
    if request_id.is_empty() {
        return Err(AppError::BadRequest("缺少 requestId".to_string()));
    }
    let Some(record) = state.dog_device_service.find_command(request_id).await? else {
        return Err(AppError::BadRequest(format!(
            "找不到 requestId={request_id} 的指令审计（中台可能已清理）"
        )));
    };
    if !req.device_code.trim().is_empty() && record.device_code != req.device_code.trim() {
        return Err(AppError::BadRequest("设备编号与审计记录不一致".to_string()));
    }
    if record.action != "同步媒体" {
        return Err(AppError::BadRequest(format!(
            "该 requestId 属于指令「{}」，不接受媒体同步状态回报",
            record.action
        )));
    }

    let status = match req.status.trim() {
        // 进度：仍停在 pending，只刷新 message
        "running" | "pending" | "" => None,
        "succeeded" | "ok" => Some(crate::business::dog::device::audit::STATUS_SUCCEEDED),
        "failed" | "error" => Some(crate::business::dog::device::audit::STATUS_FAILED),
        other => {
            return Err(AppError::BadRequest(format!("未知状态：{other}")));
        }
    };
    let result = req.detail.as_ref().map(|detail| detail.to_string());
    let rows = state
        .dog_device_service
        .record_device_report(request_id, status, result.as_deref(), Some(&req.message))
        .await?;
    Ok(Json(ApiResponse::ok(json!({
        "requestId": request_id,
        "status": status.unwrap_or("pending"),
        "updated": rows,
    }))))
}

/// 业务平台转来的「同步媒体」回报。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSyncStatusRequest {
    #[serde(default)]
    pub device_code: String,
    pub request_id: String,
    /// running / succeeded / failed
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message: String,
    /// 客户端上报的结构化细节（扫几张 / 传几张 / 目录是否清空）
    #[serde(default)]
    pub detail: Option<serde_json::Value>,
}

/// `GET /api/dogInternal/media/config` — 板端媒体目录与每航点固定帧数。
///
/// 业务平台组装 `ai_start` 时取一次（缓存 60s）随指令下发：板端写图的目录与
/// 「同步媒体」读图的目录必须是同一份配置，否则现场改了配置就会传空。
pub async fn media_config_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    authorize(&state, &headers)?;
    let config =
        crate::business::dog::media::service::load_capture_config(&state.dog_config_service).await;
    Ok(Json(ApiResponse::ok(json!(config))))
}

/// `POST /api/dogInternal/media/upload` — 航点 AI 存证图片回传。
///
/// 两种载荷（业务平台原样转发，客户端决定用哪种）：
///
/// 1. **压缩包（现行）**：`file` = `media.zip`（Stored 打包，≤64MB），
///    `mediaInfo = { deviceCode, parkCode, source, entries: [{ name, planId, pointIndex, capturedAt, remarks }] }`。
///    逐条目解包 → **一张一条** `dog_media` → 回 `results[]`，客户端只删 `ok` 的；
/// 2. **单张（老客户端）**：`file` = JPG/PNG，`mediaInfo` 是单张的描述。
///
/// 两种都按图片 `sha256` 去重（`created=false` = 之前传过，也算成功）。
/// 条目名必须在 `entries` 里显式声明过（白名单），这同时挡掉 zip 里塞
/// `../` 或任意文件的玩法。
pub async fn upload_media_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    authorize(&state, &headers)?;

    let mut info_text: Option<String> = None;
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
                    .map_err(|error| AppError::BadRequest(format!("读取上传文件失败: {error}")))?;
                file = Some((name, data.to_vec()));
            }
            Some("mediaInfo") => {
                let text = field
                    .text()
                    .await
                    .map_err(|error| AppError::BadRequest(format!("读取图片信息失败: {error}")))?;
                info_text = Some(text);
            }
            _ => {}
        }
    }

    let info_text = info_text.ok_or_else(|| AppError::BadRequest("缺少 mediaInfo".to_string()))?;
    let (file_name, bytes) =
        file.ok_or_else(|| AppError::BadRequest("请上传媒体文件".to_string()))?;

    // zip 魔数：`PK\x03\x04`
    if bytes.starts_with(b"PK\x03\x04") {
        let batch: UploadMediaBatch = serde_json::from_str(&info_text)
            .map_err(|error| AppError::BadRequest(format!("批量图片信息格式错误: {error}")))?;
        let value = save_media_batch(&state, batch, &bytes).await?;
        return Ok(Json(ApiResponse::ok(value)));
    }

    let info: UploadMediaRequest = serde_json::from_str(&info_text)
        .map_err(|error| AppError::BadRequest(format!("图片信息格式错误: {error}")))?;
    let (media, created) = state
        .dog_media_service
        .save_upload(info, &file_name, &bytes, &state.upload_dir)
        .await?;
    let vo = crate::business::dog::media::domain::MediaVO::from(media);
    Ok(Json(ApiResponse::ok(json!({
        "created": created,
        "media": serde_json::to_value(vo).unwrap_or_default(),
    }))))
}

/// 解包并逐张入库，返回 `{ total, failed, results[] }`（客户端按 `results` 删板端文件）。
async fn save_media_batch(
    state: &AppState,
    batch: UploadMediaBatch,
    zip_bytes: &[u8],
) -> Result<serde_json::Value, AppError> {
    if zip_bytes.len() > MEDIA_ZIP_MAX_BYTES {
        return Err(AppError::BadRequest("媒体压缩包超过 64MB 上限".to_string()));
    }
    if batch.entries.is_empty() {
        return Err(AppError::BadRequest(
            "mediaInfo.entries 为空：压缩包里每张图都要有计划和航点信息".to_string(),
        ));
    }
    // 只认「元数据里声明过 + 包内名合法」的条目：白名单同时也挡住了 zip 里的路径穿越
    let by_name: HashMap<&str, &UploadMediaEntry> = batch
        .entries
        .iter()
        .filter(|entry| is_safe_media_name(&entry.name))
        .map(|entry| (entry.name.as_str(), entry))
        .collect();

    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|error| AppError::BadRequest(format!("媒体压缩包无效: {error}")))?;
    if archive.len() > MEDIA_BATCH_MAX_ENTRIES {
        return Err(AppError::BadRequest(format!(
            "一次最多 {MEDIA_BATCH_MAX_ENTRIES} 张（收到 {} 张）",
            archive.len()
        )));
    }

    let mut results = Vec::with_capacity(archive.len());
    let mut created_any = false;
    let mut failed = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| AppError::BadRequest(format!("读取压缩包条目失败: {error}")))?;
        let name = entry.name().to_string();
        let Some(meta) = by_name.get(name.as_str()).copied() else {
            failed += 1;
            results.push(json!({
                "name": name,
                "ok": false,
                "error": "条目未在 mediaInfo.entries 声明（或包内名不合法）",
            }));
            continue;
        };
        // 单张上限按「解压后」算：`take` 多读 1 字节就能判超限，不把大文件读进内存
        let mut bytes = Vec::new();
        let limit = (crate::business::dog::media::service::MEDIA_IMAGE_MAX_BYTES + 1) as u64;
        if let Err(error) = entry.by_ref().take(limit).read_to_end(&mut bytes) {
            failed += 1;
            results
                .push(json!({ "name": name, "ok": false, "error": format!("读取失败: {error}") }));
            continue;
        }
        if bytes.len() > crate::business::dog::media::service::MEDIA_IMAGE_MAX_BYTES {
            failed += 1;
            results.push(json!({
                "name": name,
                "ok": false,
                "error": format!("单张超过 {}MB 上限", crate::business::dog::media::service::MEDIA_IMAGE_MAX_BYTES / 1024 / 1024),
            }));
            continue;
        }
        let request = UploadMediaRequest {
            device_code: batch.device_code.clone(),
            park_code: batch.park_code.clone(),
            plan_id: meta.plan_id.clone(),
            plan_name: meta.plan_name.clone(),
            point_index: meta.point_index,
            point_name: meta.point_name.clone(),
            captured_at: meta.captured_at.clone(),
            source: meta.source.clone().or_else(|| batch.source.clone()),
            remarks: meta.remarks.clone(),
            sha256: None,
            media_name: meta.media_name.clone(),
        };
        match state
            .dog_media_service
            .save_upload(request, &name, &bytes, &state.upload_dir)
            .await
        {
            Ok((media, created)) => {
                created_any |= created;
                results.push(json!({
                    "name": name,
                    "ok": true,
                    "created": created,
                    "mediaId": media.id,
                }));
            }
            Err(error) => {
                failed += 1;
                results.push(json!({
                    "name": name,
                    "ok": false,
                    "error": error.to_string(),
                }));
            }
        }
    }

    Ok(json!({
        "created": created_any,
        "total": results.len(),
        "failed": failed,
        "results": results,
    }))
}

/// 包内名白名单：`0001.jpg` 这类纯 ASCII 名字（不超过 64 字符）。
///
/// 客户端打的就是这个格式；不认子目录、不认中文、不认 `..` —— 走 zip 的路由
/// 只该出现这种名字，出现别的说明不是我们的客户端。
fn is_safe_media_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 || name.contains('/') || name.contains('\\') {
        return false;
    }
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    if stem.is_empty()
        || !stem
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return false;
    }
    matches!(ext.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_request_keeps_user_ids() {
        let request: InternalDeviceUpsertRequest = serde_json::from_value(serde_json::json!({
            "deviceId": "RD-200",
            "userIds": [5, 7],
        }))
        .expect("deserialize");
        assert_eq!(request.device.user_ids, Some(vec![5, 7]));
        assert_eq!(request.creator_user_id, None);
    }
}
