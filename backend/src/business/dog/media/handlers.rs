use crate::api::AppState;
use crate::business::dog::media::domain::{
    CreateMediaRequest, MediaPageQuery, MediaVO, UpdateMediaRequest,
};
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use axum::{
    Json,
    extract::{Path, Query, State},
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
    let result = state.dog_media_service.list_page(query).await?;
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
