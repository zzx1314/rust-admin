use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::device::domain::{
    CreateDeviceRequest, DevicePageQuery, DeviceVO, UpdateDeviceRequest,
};
use crate::business::dog::user_device::domain::CreateUserDeviceRequest;
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

pub async fn create_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state.dog_device_service.create(req).await?;
    // A device created by a regular user is attached to them right away,
    // otherwise it would be invisible to its creator until an admin binds it.
    if !user.is_admin {
        state
            .dog_user_device_service
            .bind(
                CreateUserDeviceRequest {
                    user_id: None,
                    device_id: device.id,
                },
                &user.data_scope(),
            )
            .await?;
    }
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

pub async fn get_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state
        .dog_device_service
        .get_by_id(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

pub async fn list_devices_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(query): Query<DevicePageQuery>,
) -> Result<Json<ApiResponse<PageResponse<DeviceVO>>>, AppError> {
    let result = state
        .dog_device_service
        .list_page(query, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_devices_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
) -> Result<Json<ApiResponse<Vec<DeviceVO>>>, AppError> {
    let devices = state
        .dog_device_service
        .list_all(&user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(
        devices.into_iter().map(DeviceVO::from).collect(),
    )))
}

pub async fn update_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state
        .dog_device_service
        .update(params.id, req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

pub async fn update_device_status_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let status = req.status.unwrap_or_else(|| "offline".to_string());
    let device = state
        .dog_device_service
        .update_status(params.id, &status, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

pub async fn delete_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .dog_device_service
        .delete(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(())))
}
