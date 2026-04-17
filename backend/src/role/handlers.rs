use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use crate::role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use crate::user::domain::User;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};

pub async fn create_role_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateRoleRequest>,
) -> Result<Json<ApiResponse<Role>>, AppError> {
    let role = state.role_service.create_role(req).await?;
    Ok(Json(ApiResponse::ok(role)))
}

pub async fn get_role_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Role>>, AppError> {
    let role = state.role_service.get_role(&id).await?;
    Ok(Json(ApiResponse::ok(role)))
}

pub async fn get_all_roles_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<Role>>>, AppError> {
    let roles = state.role_service.get_all_roles().await?;
    Ok(Json(ApiResponse::ok(roles)))
}

pub async fn get_roles_page_handler(
    State(state): State<AppState>,
    Query(req): Query<RolePageQuery>,
) -> Result<Json<ApiResponse<PageResponse<Role>>>, AppError> {
    let page_response = state.role_service.get_roles_page(req).await?;
    Ok(Json(ApiResponse::ok(page_response)))
}

pub async fn update_role_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateRoleRequest>,
) -> Result<Json<ApiResponse<Role>>, AppError> {
    let role = state.role_service.update_role(&id, req).await?;
    Ok(Json(ApiResponse::ok(role)))
}

pub async fn delete_role_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<(StatusCode, ()), AppError> {
    state.role_service.delete_role(&id).await?;
    Ok((StatusCode::NO_CONTENT, ()))
}

pub async fn assign_role_to_user_handler(
    State(state): State<AppState>,
    Path((user_id, role_id)): Path<(String, String)>,
) -> Result<(StatusCode, ()), AppError> {
    state.role_service.assign_role(&user_id, &role_id).await?;
    Ok((StatusCode::CREATED, ()))
}

pub async fn remove_role_from_user_handler(
    State(state): State<AppState>,
    Path((user_id, role_id)): Path<(String, String)>,
) -> Result<(StatusCode, ()), AppError> {
    state.role_service.remove_role(&user_id, &role_id).await?;
    Ok((StatusCode::NO_CONTENT, ()))
}

pub async fn get_user_roles_handler(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
) -> Result<Json<ApiResponse<Vec<Role>>>, AppError> {
    let roles = state.role_service.get_roles_for_user(&user_id).await?;
    Ok(Json(ApiResponse::ok(roles)))
}

pub async fn get_role_users_handler(
    State(state): State<AppState>,
    Path(role_id): Path<String>,
) -> Result<Json<ApiResponse<Vec<User>>>, AppError> {
    let users = state.role_service.get_users_for_role(&role_id).await?;
    Ok(Json(ApiResponse::ok(users)))
}

pub async fn get_roles_nolog_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<Role>>>, AppError> {
    let roles = state.role_service.get_all_roles().await?;
    Ok(Json(ApiResponse::ok(roles)))
}
