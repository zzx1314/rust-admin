use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::map::domain::{CreateMapRequest, MapPageQuery, MapVO, UpdateMapRequest};
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
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
    let map = state
        .dog_map_service
        .create(req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(MapVO::from(map))))
}

pub async fn get_map_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<MapVO>>, AppError> {
    let map = state
        .dog_map_service
        .get_by_id(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(MapVO::from(map))))
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
    let map = state
        .dog_map_service
        .update(params.id, req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(MapVO::from(map))))
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
