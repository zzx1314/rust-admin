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
use crate::common::data_scope::DataScope;
use crate::common::error::{ApiResponse, AppError};

/// 与 `/api/dogMap/upload` 保持一致的地图包上限。
const MAP_UPLOAD_LIMIT: usize = 256 * 1024 * 1024;

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
        .route(
            "/dogInternal/maps/upload",
            post(upload_map_handler).layer(DefaultBodyLimit::max(MAP_UPLOAD_LIMIT)),
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
    let vo = state.dog_map_service.to_vo(map);
    Ok(Json(ApiResponse::ok(
        serde_json::to_value(vo).unwrap_or_default(),
    )))
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
