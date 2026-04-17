use crate::api::AppState;
use crate::auth::service::{CheckTokenVO, TokenRefreshVO, UserInfoVO, UserLoginData};
use crate::common::error::AppError;
use crate::common::util::decrypt_password;
use axum::{Form, Json, extract::Path, extract::State, http::StatusCode};
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
) -> Result<Json<UserLoginData>, AppError> {
    let password = decrypt_password(&req.password)
        .map_err(|e| AppError::BadRequest(format!("Password decryption failed: {}", e)))?;

    let response = state
        .auth_service
        .login_with_vo(&req.username, &password)
        .await?;
    Ok(Json(response))
}

#[derive(Debug, serde::Serialize)]
pub struct LogoutResponse {
    pub success: bool,
    pub msg: String,
}

#[derive(Debug, serde::Serialize)]
pub struct LogoutApiResponse {
    pub code: u16,
    pub msg: String,
    pub data: LogoutResponse,
}

pub async fn logout_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    auth: TypedHeader<Authorization<Bearer>>,
) -> Result<(StatusCode, CookieJar, Json<LogoutApiResponse>), AppError> {
    let token = auth.token();
    let user_id = state.auth_service.validate_token(token).await?;
    state.auth_service.logout(&user_id).await?;

    let jar = jar.remove(
        axum_extra::extract::cookie::Cookie::build(("auth_token", ""))
            .path("/")
            .http_only(true),
    );

    let response = LogoutApiResponse {
        code: 10200,
        msg: "success".to_string(),
        data: LogoutResponse {
            success: true,
            msg: "退出成功".to_string(),
        },
    };

    Ok((StatusCode::OK, jar, Json(response)))
}

pub async fn me_handler(
    State(state): State<AppState>,
    auth: TypedHeader<Authorization<Bearer>>,
) -> Result<Json<UserInfoVO>, AppError> {
    let token = auth.token();
    let user_id = state.auth_service.validate_token(token).await?;
    let user_info = state.auth_service.get_user_info(&user_id).await?;
    Ok(Json(user_info))
}

pub async fn refresh_handler(
    State(state): State<AppState>,
    Path(refresh_token): Path<String>,
) -> Result<Json<TokenRefreshVO>, AppError> {
    let response = state
        .auth_service
        .refresh_token_with_vo(&refresh_token)
        .await?;
    Ok(Json(response))
}

pub async fn check_token_handler(
    State(state): State<AppState>,
    auth: TypedHeader<Authorization<Bearer>>,
) -> Result<Json<CheckTokenVO>, AppError> {
    let token = auth.token();
    let result = state.auth_service.check_token(token).await?;
    Ok(Json(result))
}
