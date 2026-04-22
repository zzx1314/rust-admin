use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use crate::system::sys_user::domain::{CreateUserRequest, UpdateUserRequest, User, UserPageQuery, UserVO};
use crate::system::sys_user::service::PasswordUpdateRequest;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct UserIdParam {
    id: i64,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleIdQuery {
    pub role_id: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct UsersByRoleResponse {
    pub success: bool,
    pub data: Vec<UserInfoSimple>,
}

#[derive(Debug, serde::Serialize)]
pub struct UserInfoSimple {
    pub id: i64,
    pub username: String,
    pub real_name: Option<String>,
    pub phone: Option<String>,
}

pub async fn get_users_by_role_handler(
    State(state): State<AppState>,
    Query(query): Query<RoleIdQuery>,
) -> Result<Json<UsersByRoleResponse>, AppError> {
    let users = state.user_service.get_users_by_role(&query.role_id).await?;
    let data: Vec<UserInfoSimple> = users
        .into_iter()
        .map(|u| UserInfoSimple {
            id: u.id,
            username: u.username,
            real_name: u.real_name,
            phone: u.phone,
        })
        .collect();
    Ok(Json(UsersByRoleResponse {
        success: true,
        data,
    }))
}

pub async fn edit_password_handler(
    State(state): State<AppState>,
    Json(req): Json<PasswordUpdateRequest>,
) -> Result<Json<crate::system::sys_user::service::PasswordUpdateResponse>, AppError> {
    let user_id = req
        .user_id
        .ok_or_else(|| AppError::BadRequest("user_id is required".to_string()))?;
    let response = state
        .user_service
        .update_password(&user_id, req.old_password.as_deref(), &req.password)
        .await?;
    Ok(Json(response))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetPwdRequest {
    pub id: i64,
    pub password: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableRequest {
    pub id: i64,
    pub enable: i32,
}

#[derive(Debug, serde::Serialize)]
pub struct OperationResponse {
    pub success: bool,
    pub msg: String,
}

pub async fn reset_user_password_handler(
    State(state): State<AppState>,
    Json(req): Json<ResetPwdRequest>,
) -> Result<Json<OperationResponse>, AppError> {
    let default_password = "123456";
    state
        .user_service
        .reset_password(&req.id, default_password)
        .await?;
    Ok(Json(OperationResponse {
        success: true,
        msg: "密码重置成功".to_string(),
    }))
}

pub async fn toggle_user_enable_handler(
    State(state): State<AppState>,
    Json(req): Json<EnableRequest>,
) -> Result<Json<OperationResponse>, AppError> {
    state
        .user_service
        .toggle_enable(&req.id, req.enable)
        .await?;
    Ok(Json(OperationResponse {
        success: true,
        msg: if req.enable == 1 {
            "启用成功"
        } else {
            "禁用成功"
        }
        .to_string(),
    }))
}
