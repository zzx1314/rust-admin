use crate::api::AppState;
use crate::auth::service::{LoginResponse, UserResponse};
use crate::common::error::{ApiResponse, AppError};
use crate::common::util::decrypt_password;
use axum::{Form, Json, extract::State, http::StatusCode};
use axum_extra::TypedHeader;
use axum_extra::extract::CookieJar;
use axum_extra::headers::Authorization;
use axum_extra::headers::authorization::Bearer;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub grant_type: String,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
    pub grant_type: String,
}

pub async fn login_handler(
    State(state): State<AppState>,
    Form(req): Form<LoginRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, AppError> {
    let password = decrypt_password(&req.password)
        .map_err(|e| AppError::BadRequest(format!("Password decryption failed: {}", e)))?;

    let response = state.auth_service.login(&req.username, &password).await?;
    Ok(Json(ApiResponse::ok(response)))
}

pub async fn logout_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    auth: TypedHeader<Authorization<Bearer>>,
) -> Result<(StatusCode, CookieJar), AppError> {
    let token = auth.token();
    let user_id = state.auth_service.validate_token(token).await?;
    state.auth_service.logout(&user_id).await?;

    let jar = jar.remove(
        axum_extra::extract::cookie::Cookie::build(("auth_token", ""))
            .path("/")
            .http_only(true),
    );

    Ok((StatusCode::NO_CONTENT, jar))
}

pub async fn me_handler(
    State(state): State<AppState>,
    auth: TypedHeader<Authorization<Bearer>>,
) -> Result<Json<ApiResponse<UserResponse>>, AppError> {
    let token = auth.token();
    let user_id = state.auth_service.validate_token(token).await?;
    let user = state.user_service.get_user(&user_id).await?;
    Ok(Json(ApiResponse::ok(user.into())))
}

pub async fn refresh_handler(
    State(state): State<AppState>,
    Form(req): Form<RefreshRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, AppError> {
    let response = state.auth_service.refresh_token(&req.refresh_token).await?;
    Ok(Json(ApiResponse::ok(response)))
}
