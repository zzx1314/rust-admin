use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::map::domain::{
    CreateMapRequest, MapImageVO, MapPageQuery, MapVO, UpdateMapRequest, UploadMapRequest,
};
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use axum::{
    Extension, Json,
    extract::{Multipart, Path, Query, State},
};

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

pub async fn create_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateMapRequest>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.create(req, &scope).await?;
    let vo = state.dog_map_service.to_vo(map);
    Ok(Json(ApiResponse::ok(vo)))
}

/// `POST /api/dogMap/upload` — stores a `.zip` map package.
///
/// Multipart parts: `file` (the package) and `mapInfo`
/// (`{ "mapName": "园区地图", "remarks": "...", "id": 1 }`, `id` updates an
/// existing record instead of creating a new one).
pub async fn upload_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
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

    let scope = user.data_scope();
    let map = state
        .dog_map_service
        .save_upload(
            info,
            &file_name,
            &bytes,
            &state.upload_dir,
            &scope,
        )
        .await?;
    let vo = state.dog_map_service.to_vo(map);

    Ok(Json(ApiResponse::ok(vo)))
}

pub async fn get_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.get_by_id(params.id, &scope).await?;
    let vo = state.dog_map_service.to_vo(map);
    Ok(Json(ApiResponse::ok(vo)))
}

pub async fn list_maps_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(query): Query<MapPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<MapVO>>>, AppError> {
    let result = state
        .dog_map_service
        .list_page(query, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_maps_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
) -> Result<Json<ApiResponse<Vec<MapVO>>>, AppError> {
    let maps = state.dog_map_service.list_all(&user.data_scope()).await?;
    Ok(Json(ApiResponse::ok(
        maps.into_iter().map(MapVO::from).collect(),
    )))
}

pub async fn update_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateMapRequest>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.update(params.id, req, &scope).await?;
    let vo = state.dog_map_service.to_vo(map);
    Ok(Json(ApiResponse::ok(vo)))
}

/// `GET /api/dogMap/{id}/image` — 把地图包里的 `model.pgm` 转成 PNG 返回，
/// 同时带回 `resolution` / `origin`，供路线规划页做像素↔世界坐标换算。
pub async fn get_map_image_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapImageVO>>, AppError> {
    let image = state
        .dog_map_service
        .load_image(
            params.id,
            &state.upload_dir,
            &user.data_scope(),
        )
        .await?;
    Ok(Json(ApiResponse::ok(image)))
}

pub async fn delete_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .dog_map_service
        .delete(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

/// `POST /api/dogMap/{id}/publish` — 发布为园区当前生效基线，旧版本自动归档。
pub async fn publish_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.publish(params.id, &scope).await?;
    let vo = state.dog_map_service.to_vo(map);
    Ok(Json(ApiResponse::ok(vo)))
}

/// `POST /api/dogMap/{id}/newVersion` — 派生并发布下一个不可变版本。
pub async fn new_map_version_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let scope = user.data_scope();
    let map = state.dog_map_service.new_version(params.id, &scope).await?;
    let vo = state.dog_map_service.to_vo(map);
    Ok(Json(ApiResponse::ok(vo)))
}
