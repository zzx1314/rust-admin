use crate::api::AppState;
use crate::business::dog::media::domain::{
    CreateMediaRequest, ExportMediaQuery, MediaImageVO, MediaPageQuery, MediaVO, UpdateMediaRequest,
};
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, header},
    response::Response,
};

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

pub async fn create_media_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateMediaRequest>,
) -> Result<Json<ApiResponse<MediaVO>>, AppError> {
    let media = state.dog_media_service.create(req).await?;
    Ok(Json(ApiResponse::ok(MediaVO::from(media))))
}

pub async fn get_media_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MediaVO>>, AppError> {
    let media = state.dog_media_service.get_by_id(params.id).await?;
    Ok(Json(ApiResponse::ok(MediaVO::from(media))))
}

pub async fn list_media_handler(
    State(state): State<AppState>,
    Query(query): Query<MediaPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<MediaVO>>>, AppError> {
    let result = state
        .dog_media_service
        .list_page(query.normalized())
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_media_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<MediaVO>>>, AppError> {
    let media_list = state.dog_media_service.list_all().await?;
    Ok(Json(ApiResponse::ok(
        media_list.into_iter().map(MediaVO::from).collect(),
    )))
}

pub async fn update_media_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateMediaRequest>,
) -> Result<Json<ApiResponse<MediaVO>>, AppError> {
    let media = state.dog_media_service.update(params.id, req).await?;
    Ok(Json(ApiResponse::ok(MediaVO::from(media))))
}

pub async fn delete_media_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_media_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}

/// `GET /api/dogMedia/{id}/image` — 单张预览（JSON + base64 data URL）。
///
/// 列表里逐张看一眼用（图糊没糊、框画偏没偏）；批量下载走 `/export`。
pub async fn get_media_image_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MediaImageVO>>, AppError> {
    let image = state
        .dog_media_service
        .load_image(params.id, &state.upload_dir)
        .await?;
    Ok(Json(ApiResponse::ok(image)))
}

/// `GET /api/dogMedia/{id}/file` — 下载单张图片（原始字节，`attachment`）。
///
/// 这些图是训练数据，页面只做下载、不做逐张预览；手工登记的记录没有落盘文件，
/// 返回 404。
pub async fn download_media_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Response, AppError> {
    let (bytes, file_name, content_type) = state
        .dog_media_service
        .load_file(params.id, &state.upload_dir)
        .await?;
    Ok(attachment(bytes, &file_name, &content_type))
}

/// `GET /api/dogMedia/export` — 批量下载：按勾选 `ids` 或筛选条件打包成 zip。
///
/// 包内是原图 + `manifest.json`（每张图的计划/航点/采集时间等元数据），
/// 直接喂训练流程；单次上限 200 张 / 100MB，再多请按计划或时间分批。
pub async fn export_media_handler(
    State(state): State<AppState>,
    Query(query): Query<ExportMediaQuery>,
) -> Result<Response, AppError> {
    let export = state
        .dog_media_service
        .export_zip(query, &state.upload_dir)
        .await?;
    if export.count == 0 {
        return Err(AppError::NotFound(format!(
            "符合条件的 {} 条记录都没有落盘文件（手工登记的行没有图片）",
            export.skipped
        )));
    }
    Ok(attachment(
        export.bytes,
        &export.file_name,
        "application/zip",
    ))
}

/// 附件响应：`Content-Disposition` 只用 ASCII 文件名（中文名会被浏览器截断）。
fn attachment(bytes: Vec<u8>, file_name: &str, content_type: &str) -> Response {
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    let disposition = format!("attachment; filename=\"{file_name}\"");
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    response
}
