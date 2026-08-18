use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::util::md5_verify;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::common::error::AppError;
use crate::common::traits::{RoleRepository, TokenStore, UserRepository};
use crate::system::sys_user::domain::User;

const ACCESS_TOKEN_TTL_SECS: u64 = 24 * 60 * 60;
const REFRESH_TOKEN_TTL_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: i64,
    username: String,
    exp: u64,
    iat: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse {
    pub id: i64,
    pub username: String,
    pub real_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub org_id: i64,
    pub enable: Option<i32>,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            username: user.username,
            real_name: user.real_name,
            email: user.email,
            phone: user.phone,
            org_id: user.org_id,
            enable: user.enable,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub user: UserResponse,
}

/// Frontend-compatible login response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserLoginVO {
    pub success: bool,
    pub data: UserLoginData,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub struct UserLoginData {
    pub avatar: Option<String>,
    pub username: String,
    pub nickname: Option<String>,
    pub roles: Vec<i64>,
    pub permissions: Vec<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub expires: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
}

impl UserLoginVO {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        access_token: String,
        refresh_token: String,
        expires: String,
        user_id: i64,
        username: String,
        avatar: Option<String>,
        nickname: Option<String>,
        roles: Vec<i64>,
        permissions: Vec<String>,
    ) -> Self {
        Self {
            success: true,
            data: UserLoginData {
                avatar,
                username,
                nickname,
                roles,
                permissions,
                access_token: access_token,
                refresh_token: refresh_token,
                expires,
                user_id: Some(user_id),
            },
        }
    }
}

/// Frontend-compatible refresh token response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenRefreshVO {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

/// Frontend-compatible user info response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfoVO {
    pub success: bool,
    pub data: UserInfoData,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserInfoData {
    pub avatar: String,
    pub username: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub description: String,
}

impl UserInfoVO {
    pub fn from_user(user: &User) -> Self {
        Self {
            success: true,
            data: UserInfoData {
                avatar: user.card.clone().unwrap_or_default(),
                username: user.username.clone(),
                nickname: user.real_name.clone().unwrap_or_default(),
                email: user.email.clone().unwrap_or_default(),
                phone: user.phone.clone().unwrap_or_default(),
                description: user.remarks.clone().unwrap_or_default(),
            },
        }
    }
}

/// Check token response
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckTokenVO {
    pub success: bool,
    pub msg: String,
}

impl CheckTokenVO {
    pub fn valid() -> Self {
        Self {
            success: true,
            msg: "token有效".to_string(),
        }
    }
}

pub struct AuthService {
    user_repo: Arc<dyn UserRepository>,
    token_store: Arc<dyn TokenStore>,
    role_repo: Arc<dyn RoleRepository>,
    jwt_secret: String,
}

impl AuthService {
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        token_store: Arc<dyn TokenStore>,
        role_repo: Arc<dyn RoleRepository>,
        jwt_secret: &str,
    ) -> Self {
        Self {
            user_repo,
            token_store,
            role_repo,
            jwt_secret: jwt_secret.to_string(),
        }
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<LoginResponse, AppError> {
        let user = self
            .user_repo
            .find_by_username(username)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::Unauthorized("Invalid username or password".to_string()))?;

        let password_hash = user
            .password
            .as_ref()
            .ok_or_else(|| AppError::Unauthorized("Invalid username or password".to_string()))?;

        let valid = md5_verify(password, password_hash);

        if !valid {
            return Err(AppError::Unauthorized(
                "Invalid username or password".to_string(),
            ));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let access_exp = now + ACCESS_TOKEN_TTL_SECS;
        let refresh_exp = now + REFRESH_TOKEN_TTL_SECS;

        let user_id = user.id;
        let user_id_str = user_id.to_string();
        let access_claims = Claims {
            sub: user_id,
            username: user.username.clone(),
            exp: access_exp,
            iat: now,
        };

        let refresh_claims = Claims {
            sub: user_id,
            username: user.username.clone(),
            exp: refresh_exp,
            iat: now,
        };

        let encoding_key = EncodingKey::from_secret(self.jwt_secret.as_bytes());

        let access_token = encode(&Header::default(), &access_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        let refresh_token = encode(&Header::default(), &refresh_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        self.token_store
            .set_token(&user_id_str, &access_token, ACCESS_TOKEN_TTL_SECS)
            .await?;

        Ok(LoginResponse {
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            user: user.into(),
        })
    }

    pub async fn login_with_vo(
        &self,
        username: &str,
        password: &str,
    ) -> Result<UserLoginData, AppError> {
        let user = self
            .user_repo
            .find_by_username(username)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::Unauthorized("Invalid username or password".to_string()))?;

        let password_hash = user
            .password
            .as_ref()
            .ok_or_else(|| AppError::Unauthorized("Invalid username or password".to_string()))?;

        let valid = md5_verify(password, password_hash);

        if !valid {
            return Err(AppError::Unauthorized(
                "Invalid username or password".to_string(),
            ));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let access_exp = now + ACCESS_TOKEN_TTL_SECS;
        let refresh_exp = now + REFRESH_TOKEN_TTL_SECS;

        let user_id = user.id;
        let user_id_str = user_id.to_string();
        let access_claims = Claims {
            sub: user_id,
            username: user.username.clone(),
            exp: access_exp,
            iat: now,
        };

        let refresh_claims = Claims {
            sub: user_id,
            username: user.username.clone(),
            exp: refresh_exp,
            iat: now,
        };

        let encoding_key = EncodingKey::from_secret(self.jwt_secret.as_bytes());

        let access_token = encode(&Header::default(), &access_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        let refresh_token = encode(&Header::default(), &refresh_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        self.token_store
            .set_token(&user_id_str, &access_token, ACCESS_TOKEN_TTL_SECS)
            .await?;

        let roles = self.get_user_roles(&user_id).await?;
        let permissions = self.get_user_permissions(&user_id).await?;

        let expires = chrono::DateTime::from_timestamp(access_exp as i64, 0)
            .map(|dt| dt.format("%Y/%m/%d %H:%M:%S").to_string())
            .unwrap_or_default();

        Ok(UserLoginData {
            access_token: access_token,
            refresh_token: refresh_token,
            expires,
            user_id: Some(user_id),
            username: user.username.clone(),
            avatar: None,
            nickname: user.real_name.clone(),
            roles,
            permissions,
        })
    }

    async fn get_user_roles(&self, user_id: &i64) -> Result<Vec<i64>, AppError> {
        let roles = self
            .role_repo
            .find_roles_by_user_id(user_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(roles.into_iter().map(|r| r.id).collect())
    }

    async fn get_user_permissions(&self, user_id: &i64) -> Result<Vec<String>, AppError> {
        let roles = self
            .role_repo
            .find_roles_by_user_id(user_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        let permissions: Vec<String> = roles.iter().filter_map(|r| r.code.clone()).collect();
        Ok(permissions)
    }

    pub async fn logout(&self, user_id: i64) -> Result<(), AppError> {
        self.token_store.delete_token(&user_id.to_string()).await
    }

    pub async fn validate_token(&self, token: &str) -> Result<i64, AppError> {
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AppError::Unauthorized("Invalid token".to_string()))?;

        let user_id = token_data.claims.sub;

        let stored_token = self.token_store.get_token(&user_id.to_string()).await?;

        match stored_token {
            Some(stored) if stored == token => Ok(user_id),
            _ => Err(AppError::Unauthorized(
                "Token expired or revoked".to_string(),
            )),
        }
    }

    pub fn extract_username(&self, token: &str) -> Option<String> {
        decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .ok()
        .map(|data| data.claims.username)
    }

    pub async fn refresh_token(&self, refresh_token: &str) -> Result<LoginResponse, AppError> {
        let token_data = decode::<Claims>(
            refresh_token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AppError::Unauthorized("Invalid refresh token".to_string()))?;

        let user_id = token_data.claims.sub;
        let username = &token_data.claims.username;

        let user = self
            .user_repo
            .find_by_username(username)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::Unauthorized("User not found".to_string()))?;

        if user.id != user_id {
            return Err(AppError::Unauthorized("User ID mismatch".to_string()));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let access_exp = now + ACCESS_TOKEN_TTL_SECS;
        let refresh_exp = now + REFRESH_TOKEN_TTL_SECS;

        let user_id = user.id.clone();
        let access_claims = Claims {
            sub: user_id.clone(),
            username: user.username.clone(),
            exp: access_exp,
            iat: now,
        };

        let refresh_claims = Claims {
            sub: user_id.clone(),
            username: user.username.clone(),
            exp: refresh_exp,
            iat: now,
        };

        let encoding_key = EncodingKey::from_secret(self.jwt_secret.as_bytes());

        let new_access_token = encode(&Header::default(), &access_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        let new_refresh_token = encode(&Header::default(), &refresh_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        self.token_store
            .set_token(
                &user_id.to_string(),
                &new_access_token,
                ACCESS_TOKEN_TTL_SECS,
            )
            .await?;

        Ok(LoginResponse {
            access_token: new_access_token,
            refresh_token: new_refresh_token,
            token_type: "Bearer".to_string(),
            user: user.into(),
        })
    }

    pub async fn refresh_token_with_vo(
        &self,
        refresh_token: &str,
    ) -> Result<TokenRefreshVO, AppError> {
        let token_data = decode::<Claims>(
            refresh_token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AppError::Unauthorized("Invalid refresh token".to_string()))?;

        let user_id = token_data.claims.sub;
        let username = &token_data.claims.username;

        let user = self
            .user_repo
            .find_by_username(username)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::Unauthorized("User not found".to_string()))?;

        if user.id != user_id {
            return Err(AppError::Unauthorized("User ID mismatch".to_string()));
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let access_exp = now + ACCESS_TOKEN_TTL_SECS;
        let refresh_exp = now + REFRESH_TOKEN_TTL_SECS;

        let user_id = user.id.clone();
        let access_claims = Claims {
            sub: user_id.clone(),
            username: user.username.clone(),
            exp: access_exp,
            iat: now,
        };

        let refresh_claims = Claims {
            sub: user_id.clone(),
            username: user.username.clone(),
            exp: refresh_exp,
            iat: now,
        };

        let encoding_key = EncodingKey::from_secret(self.jwt_secret.as_bytes());

        let new_access_token = encode(&Header::default(), &access_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        let new_refresh_token = encode(&Header::default(), &refresh_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        self.token_store
            .set_token(
                &user_id.to_string(),
                &new_access_token,
                ACCESS_TOKEN_TTL_SECS,
            )
            .await?;

        Ok(TokenRefreshVO {
            access_token: new_access_token,
            refresh_token: new_refresh_token,
            expires_in: ACCESS_TOKEN_TTL_SECS as i64,
        })
    }

    pub async fn get_user_info(&self, user_id: i64) -> Result<UserInfoVO, AppError> {
        let user = self
            .user_repo
            .find_by_id(&user_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
        Ok(UserInfoVO::from_user(&user))
    }

    pub async fn check_token(&self, token: &str) -> Result<CheckTokenVO, AppError> {
        let _ = self.validate_token(token).await?;
        Ok(CheckTokenVO::valid())
    }
}
