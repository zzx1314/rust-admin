use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::device::domain::{
    AssignUsersRequest, AssignableUserVO, CreateDeviceRequest, DeviceFilter, DevicePageQuery,
    DeviceVO, UnassignUsersRequest, UpdateDeviceRequest,
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

#[derive(serde::Deserialize)]
pub struct DeviceCodeParam {
    pub device_code: String,
}

pub async fn create_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state
        .dog_device_service
        .create(req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

/// 业务平台设备同步入口：按设备编号幂等写入。
pub async fn upsert_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateDeviceRequest>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError("仅管理员可以同步设备台账".to_string()));
    }
    let device = state
        .dog_device_service
        .upsert(req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

pub async fn get_device_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let device = state
        .dog_device_service
        .get_vo(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

pub async fn get_device_by_code_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<DeviceCodeParam>,
) -> Result<Json<ApiResponse<Option<DeviceVO>>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError(
            "仅管理员可以按编号查询设备".to_string(),
        ));
    }
    let device = state
        .dog_device_service
        .get_vo_by_code(&params.device_code)
        .await?;
    Ok(Json(ApiResponse::ok(device)))
}

pub async fn list_devices_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(query): Query<DevicePageQuery>,
) -> Result<Json<ApiResponse<PageResponse<DeviceVO>>>, AppError> {
    let page = state
        .dog_device_service
        .list_page(query, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(page)))
}

pub async fn list_all_devices_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(filter): Query<DeviceFilter>,
) -> Result<Json<ApiResponse<Vec<DeviceVO>>>, AppError> {
    let devices = state
        .dog_device_service
        .list_all(filter, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(devices)))
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
    Ok(Json(ApiResponse::ok(device)))
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

pub async fn assign_users_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<AssignUsersRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError("仅管理员可以分配设备".to_string()));
    }
    let affected = state.dog_device_service.assign_users(req).await?;
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "affected": affected,
    }))))
}

pub async fn unassign_users_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<UnassignUsersRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    if !user.is_admin {
        return Err(AppError::AuthError("仅管理员可以解除设备分配".to_string()));
    }
    let affected = state.dog_device_service.unassign_users(req).await?;
    Ok(Json(ApiResponse::ok(serde_json::json!({
        "affected": affected,
    }))))
}

pub async fn list_assignable_users_handler(
    State(state): State<AppState>,
    Extension(_user): Extension<RequestUser>,
) -> Result<Json<ApiResponse<Vec<AssignableUserVO>>>, AppError> {
    let users = state.dog_device_service.assignable_users().await?;
    Ok(Json(ApiResponse::ok(users)))
}
