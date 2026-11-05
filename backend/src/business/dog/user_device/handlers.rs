use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::user_device::domain::{
    CreateUserDeviceRequest, UserDevicePageQuery, UserDeviceVO,
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
pub struct UserIdParam {
    pub user_id: i64,
}

pub async fn create_binding_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateUserDeviceRequest>,
) -> Result<Json<ApiResponse<UserDeviceVO>>, AppError> {
    let binding = state
        .dog_user_device_service
        .bind(req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(binding)))
}

pub async fn get_binding_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<UserDeviceVO>>, AppError> {
    let binding = state
        .dog_user_device_service
        .get_by_id(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(binding)))
}

pub async fn list_bindings_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(query): Query<UserDevicePageQuery>,
) -> Result<Json<ApiResponse<PageResponse<UserDeviceVO>>>, AppError> {
    let result = state
        .dog_user_device_service
        .list_page(query, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_bindings_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
) -> Result<Json<ApiResponse<Vec<UserDeviceVO>>>, AppError> {
    let bindings = state
        .dog_user_device_service
        .list_all(&user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(bindings)))
}

pub async fn list_bindings_by_user_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<UserIdParam>,
) -> Result<Json<ApiResponse<Vec<UserDeviceVO>>>, AppError> {
    let bindings = state
        .dog_user_device_service
        .list_by_user(params.user_id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(bindings)))
}

pub async fn delete_binding_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .dog_user_device_service
        .delete(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(())))
}
