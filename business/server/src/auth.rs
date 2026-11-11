use crate::app::AppState;
use crate::error::{ApiResponse, AppError};
use crate::models::{LoginRequest, LoginResponse, UserInfo};
use crate::zentai;
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
    /// 中台账号 id；本地服务账号为空
    #[serde(default)]
    pub zentai_user_id: Option<i64>,
    #[serde(default)]
    pub zentai_username: Option<String>,
    /// 中台管理员（角色 id = 1）在业务平台拥有全局视角
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Debug, Clone, FromRow)]
struct UserRow {
    id: i64,
    username: String,
    password_hash: String,
    display_name: Option<String>,
    role: String,
    zentai_user_id: Option<i64>,
    zentai_username: Option<String>,
}

const USER_COLUMNS: &str = "id, username, password_hash, display_name, role,
    zentai_user_id, zentai_username";

pub async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, AppError> {
    let username = payload.username.trim();
    if username.is_empty() || payload.password.is_empty() {
        return Err(AppError::Unauthorized("用户名或密码错误".to_string()));
    }

    let local = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE username = ?1"
    ))
    .bind(username)
    .fetch_optional(&state.db)
    .await?;

    // 1) 本地账号（含设备客户端服务账号）：本地密码优先，保持后端离线可用。
    if let Some(user) = local.as_ref() {
        let valid = bcrypt::verify(&payload.password, &user.password_hash).unwrap_or(false);
        if valid {
            return issue_token(&state, user.clone());
        }
    }

    // 2) 中台账号：向中台校验密码，成功后落地/更新本地账号。
    if state.config.zentai_ready() {
        let http = reqwest::Client::new();
        match zentai::login(&state.config, &http, username, &payload.password).await {
            Ok(session) => {
                let user = upsert_zentai_user(&state, &session).await?;
                return issue_token(&state, user);
            }
            Err(error @ AppError::Unauthorized(_)) => {
                // 本地密码已失败、中台也拒绝，统一返回账号或密码错误
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }

    Err(AppError::Unauthorized("用户名或密码错误".to_string()))
}

fn issue_token(
    state: &AppState,
    user: UserRow,
) -> Result<Json<ApiResponse<LoginResponse>>, AppError> {
    let is_admin = user.role == "admin";
    let expires = Utc::now() + Duration::hours(state.config.jwt_ttl_hours);
    let claims = Claims {
        sub: user.id,
        username: user.username.clone(),
        role: user.role.clone(),
        exp: expires.timestamp() as usize,
        zentai_user_id: user.zentai_user_id,
        zentai_username: user.zentai_username.clone(),
        is_admin,
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
            zentai_user_id: user.zentai_user_id,
            zentai_username: user.zentai_username,
            is_admin,
        },
    }))
}

/// 中台账号首次登录时落地本地用户，后续登录同步昵称/角色。
async fn upsert_zentai_user(
    state: &AppState,
    session: &zentai::ZentaiSession,
) -> Result<UserRow, AppError> {
    let existing: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM users WHERE zentai_user_id = ?1 OR username = ?2 LIMIT 1")
            .bind(session.user_id)
            .bind(&session.username)
            .fetch_optional(&state.db)
            .await?;

    let role = if session.is_admin() {
        "admin"
    } else {
        "operator"
    };
    let roles = serde_json::to_string(&session.roles).unwrap_or_else(|_| "[]".to_string());
    let display_name = session.nickname.clone();

    match existing {
        Some((id,)) => {
            sqlx::query(
                "UPDATE users SET username = ?1, zentai_user_id = ?2, zentai_username = ?3,
                    zentai_roles = ?4, display_name = COALESCE(?5, display_name),
                    role = CASE WHEN role = 'admin' THEN 'admin' ELSE ?6 END
                 WHERE id = ?7",
            )
            .bind(&session.username)
            .bind(session.user_id)
            .bind(&session.username)
            .bind(&roles)
            .bind(&display_name)
            .bind(role)
            .bind(id)
            .execute(&state.db)
            .await?;
        }
        None => {
            let password_hash = bcrypt::hash("zentai-managed-account", bcrypt::DEFAULT_COST)?;
            sqlx::query(
                "INSERT INTO users (username, password_hash, display_name, role, created_at,
                    zentai_user_id, zentai_username, zentai_roles)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )
            .bind(&session.username)
            .bind(password_hash)
            .bind(&display_name)
            .bind(role)
            .bind(crate::db::now())
            .bind(session.user_id)
            .bind(&session.username)
            .bind(&roles)
            .execute(&state.db)
            .await?;
        }
    }

    sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE zentai_user_id = ?1"
    ))
    .bind(session.user_id)
    .fetch_one(&state.db)
    .await
    .map_err(AppError::from)
}

pub async fn me_handler(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<UserInfo>>, AppError> {
    let user =
        sqlx::query_as::<_, UserRow>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1"))
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::Unauthorized("用户不存在".to_string()))?;

    Ok(ApiResponse::ok(UserInfo {
        id: user.id,
        username: user.username,
        display_name: user.display_name,
        role: user.role,
        zentai_user_id: user.zentai_user_id,
        zentai_username: user.zentai_username,
        is_admin: claims.is_admin,
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

/// 当前业务账号在中台可见的设备编号。
///
/// `None` 表示不限制（本地服务账号 / 中台管理员 / 未配置中台）。
pub async fn allowed_device_codes(
    state: &AppState,
    claims: &Claims,
) -> Result<Option<std::collections::HashSet<String>>, AppError> {
    let Some(zentai_user_id) = claims.zentai_user_id else {
        return Ok(None);
    };
    if claims.is_admin || !state.config.zentai_ready() {
        return Ok(None);
    }
    let assignments = zentai::assignment_map(&state.config).await?;
    Ok(Some(
        assignments
            .into_iter()
            .filter(|(_, users)| users.contains(&zentai_user_id))
            .map(|(device_id, _)| device_id)
            .collect(),
    ))
}

/// 校验当前业务账号是否可以操作某台设备。
pub async fn ensure_device_access(
    state: &AppState,
    claims: &Claims,
    device_id: &str,
) -> Result<(), AppError> {
    let allowed = allowed_device_codes(state, claims).await?;
    match allowed {
        None => Ok(()),
        Some(codes) if codes.contains(device_id) => Ok(()),
        Some(_) => Err(AppError::NotFound(
            "设备不存在或不在你的管辖范围内".to_string(),
        )),
    }
}
