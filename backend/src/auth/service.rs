use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::common::util::md5_verify;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::common::error::AppError;
use crate::common::traits::{TokenStore, UserRepository};
use crate::user::domain::User;

const ACCESS_TOKEN_TTL_SECS: u64 = 24 * 60 * 60;
const REFRESH_TOKEN_TTL_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    username: String,
    exp: u64,
    iat: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserResponse {
    pub id: String,
    pub username: String,
    pub real_name: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub org_id: Option<String>,
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

pub struct AuthService {
    user_repo: Arc<dyn UserRepository>,
    token_store: Arc<dyn TokenStore>,
    jwt_secret: String,
}

impl AuthService {
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        token_store: Arc<dyn TokenStore>,
        jwt_secret: &str,
    ) -> Self {
        Self {
            user_repo,
            token_store,
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

        let access_token = encode(&Header::default(), &access_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        let refresh_token = encode(&Header::default(), &refresh_claims, &encoding_key)
            .map_err(|e| AppError::AuthError(format!("Token generation failed: {}", e)))?;

        self.token_store
            .set_token(&user_id, &access_token, ACCESS_TOKEN_TTL_SECS)
            .await?;

        Ok(LoginResponse {
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            user: user.into(),
        })
    }

    pub async fn logout(&self, user_id: &str) -> Result<(), AppError> {
        self.token_store.delete_token(user_id).await
    }

    pub async fn validate_token(&self, token: &str) -> Result<String, AppError> {
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AppError::Unauthorized("Invalid token".to_string()))?;

        let user_id = token_data.claims.sub;

        let stored_token = self.token_store.get_token(&user_id).await?;

        match stored_token {
            Some(stored) if stored == token => Ok(user_id),
            _ => Err(AppError::Unauthorized(
                "Token expired or revoked".to_string(),
            )),
        }
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
            .set_token(&user_id, &new_access_token, ACCESS_TOKEN_TTL_SECS)
            .await?;

        Ok(LoginResponse {
            access_token: new_access_token,
            refresh_token: new_refresh_token,
            token_type: "Bearer".to_string(),
            user: user.into(),
        })
    }
}
