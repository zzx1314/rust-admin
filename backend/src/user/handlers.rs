use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use crate::user::domain::{CreateUserRequest, UpdateUserRequest, User, UserPageQuery, UserVO};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct UserIdParam {
    id: String,
}

pub async fn create_user_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<ApiResponse<User>>, AppError> {
    let user = state.user_service.create_user(req).await?;
    Ok(Json(ApiResponse::ok(user)))
}

pub async fn get_user_handler(
    State(state): State<AppState>,
    Path(params): Path<UserIdParam>,
) -> Result<Json<ApiResponse<User>>, AppError> {
    let user = state.user_service.get_user(&params.id).await?;
    Ok(Json(ApiResponse::ok(user)))
}

pub async fn get_all_users_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<User>>>, AppError> {
    let users = state.user_service.get_all_users().await?;
    Ok(Json(ApiResponse::ok(users)))
}

pub async fn get_users_page_handler(
    State(state): State<AppState>,
    Query(query): Query<UserPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<UserVO>>>, AppError> {
    let page_response = state.user_service.get_users_page(query).await?;
    Ok(Json(ApiResponse::ok(page_response)))
}

pub async fn update_user_handler(
    State(state): State<AppState>,
    Path(params): Path<UserIdParam>,
    Json(req): Json<UpdateUserRequest>,
) -> Result<Json<ApiResponse<User>>, AppError> {
    let user = state.user_service.update_user(&params.id, req).await?;
    Ok(Json(ApiResponse::ok(user)))
}

pub async fn delete_user_handler(
    State(state): State<AppState>,
    Path(params): Path<UserIdParam>,
) -> Result<(StatusCode, ()), AppError> {
    state.user_service.delete_user(&params.id).await?;
    Ok((StatusCode::NO_CONTENT, ()))
}
