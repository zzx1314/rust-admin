use crate::api::AppState;
use crate::business::dog::device::domain::{
    CreateDeviceRequest, DevicePageQuery, DeviceVO, UpdateDeviceRequest,
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

pub async fn create_device_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state.dog_device_service.create(req).await?;
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

pub async fn get_device_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state.dog_device_service.get_by_id(params.id).await?;
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

pub async fn list_devices_handler(
    State(state): State<AppState>,
    Query(query): Query<DevicePageQuery>,
) -> Result<Json<ApiResponse<PageResponse<DeviceVO>>>, AppError> {
    let result = state.dog_device_service.list_page(query).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_devices_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<DeviceVO>>>, AppError> {
    let devices = state.dog_device_service.list_all().await?;
    Ok(Json(ApiResponse::ok(
        devices.into_iter().map(DeviceVO::from).collect(),
    )))
}

pub async fn update_device_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state.dog_device_service.update(params.id, req).await?;
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

pub async fn update_device_status_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let status = req.status.unwrap_or_else(|| "offline".to_string());
    let device = state
        .dog_device_service
        .update_status(params.id, &status)
        .await?;
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

pub async fn delete_device_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_device_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}
