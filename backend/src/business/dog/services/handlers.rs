use crate::api::AppState;
use crate::business::dog::services::domain::{
    CreateServicesRequest, ServicesVO, UpdateServicesRequest,
};
use crate::common::error::{ApiResponse, AppError};
use axum::{
    Json,
    extract::{Path, State},
};

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

pub async fn create_services_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateServicesRequest>,
) -> Result<Json<ApiResponse<ServicesVO>>, AppError> {
    let services = state.dog_services_service.create(req).await?;
    Ok(Json(ApiResponse::ok(ServicesVO::from(services))))
}

pub async fn get_services_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<ServicesVO>>, AppError> {
    let services = state.dog_services_service.get_by_id(params.id).await?;
    Ok(Json(ApiResponse::ok(ServicesVO::from(services))))
}

pub async fn list_all_services_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<ServicesVO>>>, AppError> {
    let services_list = state.dog_services_service.list_all().await?;
    Ok(Json(ApiResponse::ok(
        services_list.into_iter().map(ServicesVO::from).collect(),
    )))
}

pub async fn update_services_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateServicesRequest>,
) -> Result<Json<ApiResponse<ServicesVO>>, AppError> {
    let services = state.dog_services_service.update(params.id, req).await?;
    Ok(Json(ApiResponse::ok(ServicesVO::from(services))))
}

pub async fn delete_services_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_services_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}
