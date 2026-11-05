use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::park::domain::{
    CreateParkRequest, ParkPageQuery, ParkVO, UpdateParkRequest,
};
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

pub async fn create_park_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateParkRequest>,
) -> Result<Json<ApiResponse<ParkVO>>, AppError> {
    let park = state
        .dog_park_service
        .create(req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(ParkVO::from(park))))
}

pub async fn get_park_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<ParkVO>>, AppError> {
    let park = state
        .dog_park_service
        .get_by_id(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(ParkVO::from(park))))
}

pub async fn list_parks_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(query): Query<ParkPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<ParkVO>>>, AppError> {
    let result = state
        .dog_park_service
        .list_page(query, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_parks_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
) -> Result<Json<ApiResponse<Vec<ParkVO>>>, AppError> {
    let parks = state.dog_park_service.list_all(&user.data_scope()).await?;
    Ok(Json(ApiResponse::ok(
        parks.into_iter().map(ParkVO::from).collect(),
    )))
}

pub async fn update_park_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateParkRequest>,
) -> Result<Json<ApiResponse<ParkVO>>, AppError> {
    let park = state
        .dog_park_service
        .update(params.id, req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(ParkVO::from(park))))
}

pub async fn delete_park_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .dog_park_service
        .delete(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(())))
}
