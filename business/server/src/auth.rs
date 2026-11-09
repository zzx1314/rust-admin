use crate::app::AppState;
use crate::error::{ApiResponse, AppError};
use crate::models::{LoginRequest, LoginResponse, UserInfo};
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum::{Extension, Json};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claims {
    pub sub: i64,
    pub username: String,
    pub role: String,
    pub exp: usize,
}

#[derive(Debug, FromRow)]
struct UserRow {
    id: i64,
    username: String,
    password_hash: String,
    display_name: Option<String>,
    role: String,
}

pub async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, AppError> {
    let user = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, display_name, role FROM users WHERE username = ?1",
    )
    .bind(payload.username.trim())
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::Unauthorized("用户名或密码错误".to_string()))?;

    let valid = bcrypt::verify(&payload.password, &user.password_hash)?;
    if !valid {
        return Err(AppError::Unauthorized("用户名或密码错误".to_string()));
    }

    let expires = Utc::now() + Duration::hours(state.config.jwt_ttl_hours);
    let claims = Claims {
        sub: user.id,
        username: user.username.clone(),
        role: user.role.clone(),
        exp: expires.timestamp() as usize,
    };
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )?;

    Ok(ApiResponse::ok(LoginResponse {
        token,
        user: UserInfo {
            id: user.id,
            username: user.username,
            display_name: user.display_name,
            role: user.role,
        },
    }))
}

pub async fn me_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<UserInfo>>, AppError> {
    let user = sqlx::query_as::<_, UserRow>(
        "SELECT id, username, password_hash, display_name, role FROM users WHERE id = ?1",
    )
    .bind(claims.sub)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::Unauthorized("用户不存在".to_string()))?;

    Ok(ApiResponse::ok(UserInfo {
        id: user.id,
        username: user.username,
        display_name: user.display_name,
        role: user.role,
    }))
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("请先登录".to_string()))?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or_else(|| AppError::Unauthorized("登录凭证格式错误".to_string()))?;

    let decoded = decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &Validation::default(),
    )?;

    request.extensions_mut().insert(decoded.claims);
    Ok(next.run(request).await)
}
