use std::sync::Arc;

use crate::common::password;
use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::common::error::AppError;
use crate::common::traits::{RoleRepository, TokenStore, UserRepository};
use crate::system::sys_dict_item::service::SysDictItemService;
use crate::system::sys_user::domain::{UpdateUserRequest, User};

const REFRESH_TOKEN_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// Distinguishes the two token kinds so that a refresh token can never be
/// replayed as an access token (and vice versa).
const TOKEN_TYPE_ACCESS: &str = "access";
const TOKEN_TYPE_REFRESH: &str = "refresh";

/// `enable` value that marks an account as usable.
const ENABLED: i32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: i64,
    username: String,
    exp: u64,
    iat: u64,
    /// `"access"` or `"refresh"`; see [`TOKEN_TYPE_ACCESS`].
    #[serde(default = "default_token_type")]
    typ: String,
    #[serde(default)]
    jti: Option<String>,
}

/// Tokens issued before `typ` existed are treated as access tokens, which is
/// what they effectively were: the stored access token was the only one
/// `validate_token` would accept.
fn default_token_type() -> String {
    TOKEN_TYPE_ACCESS.to_string()
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
    /// Whether the password has exceeded the configured validity period.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_expired: Option<bool>,
    /// The moment the current password becomes invalid (Beijing time, UTC+8).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_expire_date: Option<String>,
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
                access_token,
                refresh_token,
                expires,
                user_id: Some(user_id),
                password_expired: None,
                password_expire_date: None,
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
    dict_item_service: Arc<SysDictItemService>,
}

impl AuthService {
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        token_store: Arc<dyn TokenStore>,
        role_repo: Arc<dyn RoleRepository>,
        jwt_secret: &str,
        dict_item_service: Arc<SysDictItemService>,
    ) -> Self {
        Self {
            user_repo,
            token_store,
            role_repo,
            jwt_secret: jwt_secret.to_string(),
            dict_item_service,
        }
    }

    /// Read the security policy once and parse every field we need.
    ///
    /// Previously each getter called `get_safe_policy()` separately, so a single
    /// login issued 4-6 identical dictionary queries.
    async fn security_policy(&self) -> SecurityPolicy {
        let Ok(policy) = self.dict_item_service.get_safe_policy().await else {
            return SecurityPolicy::default();
        };

        SecurityPolicy {
            max_try_count: policy
                .get("sysLoginMaxTryCount")
                .and_then(|value| parse_leading_number(value, "次"))
                .unwrap_or(5),
            max_lock_time_mins: policy
                .get("sysLoginMaxLockTime")
                .and_then(|value| parse_leading_number(value, "分钟"))
                .unwrap_or(5),
            session_timeout_secs: policy
                .get("sysOvertime")
                .and_then(|value| parse_timeout_secs(value))
                .unwrap_or(24 * 60 * 60),
            pass_change_days: policy
                .get("sysPassChange")
                .and_then(|value| parse_leading_number(value, "天"))
                .filter(|days| *days > 0),
        }
    }

    /// Determine whether the user's password has expired and, if a validity
    /// period is configured, when it expires.
    fn password_expiry(policy: &SecurityPolicy, user: &User) -> (bool, Option<String>) {
        let Some(days) = policy.pass_change_days else {
            return (false, None);
        };
        let Some(updated) = user.pass_update_time else {
            return (false, None);
        };
        let expire_at = updated + chrono::Duration::days(days);
        let expired = Utc::now() >= expire_at;
        (
            expired,
            Some(crate::common::util::format_datetime(expire_at)),
        )
    }

    /// Authenticate a user and issue a token pair.
    ///
    /// This is the single implementation behind both
    /// [`AuthService::login`] and [`AuthService::login_with_vo`]; those only
    /// reshape the result, so the credential / lockout / token logic lives here
    /// exactly once.
    async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<AuthenticatedSession, AppError> {
        let user = self
            .user_repo
            .find_by_username(username)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(invalid_credentials)?;

        let policy = self.security_policy().await;

        self.release_expired_lock(&user, &policy).await?;

        let password_hash = user.password.as_deref().unwrap_or_default();
        let valid = password::verify(password, password_hash);

        tracing::debug!(
            username = %username,
            user_id = user.id,
            enable = ?user.enable,
            is_deleted = user.is_deleted,
            password_matches = valid,
            "Login credential check"
        );

        if !valid {
            return self.record_failed_attempt(&user, &policy).await;
        }

        // Reject disabled / deleted accounts only after the password check so
        // that the response does not reveal whether the account exists.
        if user.is_deleted != 0 {
            return Err(invalid_credentials());
        }
        if user.enable.is_some_and(|enable| enable != ENABLED) {
            return Err(AppError::AuthError("账户已被禁用".to_string()));
        }

        self.upgrade_password_hash_if_needed(&user, password, password_hash)
            .await;

        let request = UpdateUserRequest {
            try_count: Some(0),
            lock_flag: Some(1),
            last_login_time: Some(Utc::now()),
            ..Default::default()
        };
        self.user_repo
            .update(&user.id, &request)
            .await
            .map_err(AppError::DatabaseError)?;

        self.issue_session(user, &policy).await
    }

    /// Clear the lock when the configured lockout window has elapsed.
    async fn release_expired_lock(
        &self,
        user: &User,
        policy: &SecurityPolicy,
    ) -> Result<(), AppError> {
        // `lock_flag == Some(0)` means "locked"; any other value is unlocked.
        if user.lock_flag != Some(0) {
            return Ok(());
        }
        let Some(lock_time) = user.lock_time else {
            return Ok(());
        };

        let locked_for = Utc::now() - lock_time;
        if locked_for < chrono::Duration::minutes(policy.max_lock_time_mins) {
            let remaining = policy.max_lock_time_mins - locked_for.num_minutes();
            return Err(AppError::Unauthorized(format!(
                "账户已被锁定，请{}分钟后再试",
                remaining.max(1)
            )));
        }

        let request = UpdateUserRequest {
            try_count: Some(0),
            lock_flag: Some(1),
            ..Default::default()
        };
        self.user_repo
            .update(&user.id, &request)
            .await
            .map_err(AppError::DatabaseError)?;
        Ok(())
    }

    /// Bump the failure counter and lock the account when the limit is hit.
    async fn record_failed_attempt(
        &self,
        user: &User,
        policy: &SecurityPolicy,
    ) -> Result<AuthenticatedSession, AppError> {
        let current_count = user.try_count.unwrap_or(0) + 1;

        if current_count >= policy.max_try_count {
            let request = UpdateUserRequest {
                try_count: Some(current_count),
                lock_flag: Some(0),
                lock_time: Some(Utc::now()),
                ..Default::default()
            };
            // A failure to persist the counter must not silently disable the
            // lockout, so this one is propagated instead of discarded.
            self.user_repo
                .update(&user.id, &request)
                .await
                .map_err(AppError::DatabaseError)?;

            tracing::warn!(
                user_id = user.id,
                attempts = current_count,
                "Account locked after repeated failed logins"
            );
            return Err(AppError::Unauthorized(format!(
                "密码错误{}次，账户已被锁定，请{}分钟后再试",
                current_count, policy.max_lock_time_mins
            )));
        }

        let request = UpdateUserRequest {
            try_count: Some(current_count),
            ..Default::default()
        };
        self.user_repo
            .update(&user.id, &request)
            .await
            .map_err(AppError::DatabaseError)?;

        Err(AppError::Unauthorized(format!(
            "用户名或密码错误，还可重试{}次",
            policy.max_try_count - current_count
        )))
    }

    /// Replace a legacy MD5 hash with bcrypt right after a successful login.
    ///
    /// Failures are logged and ignored: the user is already authenticated, and
    /// a failed upgrade only means the next login retries it.
    async fn upgrade_password_hash_if_needed(
        &self,
        user: &User,
        password: &str,
        stored_hash: &str,
    ) {
        if !crate::common::password::needs_rehash(stored_hash) {
            return;
        }
        let Ok(hashed) = crate::common::password::hash(password) else {
            tracing::warn!(user_id = user.id, "failed to rehash password");
            return;
        };

        let request = UpdateUserRequest {
            password: Some(hashed),
            ..Default::default()
        };
        match self.user_repo.update(&user.id, &request).await {
            Ok(_) => tracing::info!(
                user_id = user.id,
                "password hash upgraded from legacy MD5 to bcrypt"
            ),
            Err(error) => tracing::warn!(
                user_id = user.id,
                %error,
                "failed to persist upgraded password hash"
            ),
        }
    }

    /// Build the token pair and persist it, then hand back the session.
    async fn issue_session(
        &self,
        user: User,
        policy: &SecurityPolicy,
    ) -> Result<AuthenticatedSession, AppError> {
        let now = Utc::now().timestamp().max(0) as u64;
        let session_timeout = policy.session_timeout_secs;
        let access_exp = now + session_timeout;
        let refresh_exp = now + REFRESH_TOKEN_TTL_SECS;

        let encoding_key = EncodingKey::from_secret(self.jwt_secret.as_bytes());
        let user_id = user.id;
        let user_id_str = user_id.to_string();

        let access_token = self.encode_token(
            Claims {
                sub: user_id,
                username: user.username.clone(),
                exp: access_exp,
                iat: now,
                typ: TOKEN_TYPE_ACCESS.to_string(),
                jti: None,
            },
            &encoding_key,
        )?;

        let refresh_token = self.encode_token(
            Claims {
                sub: user_id,
                username: user.username.clone(),
                exp: refresh_exp,
                iat: now,
                typ: TOKEN_TYPE_REFRESH.to_string(),
                jti: Some(uuid::Uuid::new_v4().to_string()),
            },
            &encoding_key,
        )?;

        self.token_store
            .set_token(&user_id_str, &access_token, session_timeout)
            .await?;
        self.token_store
            .set_refresh_token(&user_id_str, &refresh_token, REFRESH_TOKEN_TTL_SECS)
            .await?;

        Ok(AuthenticatedSession {
            access_token,
            refresh_token,
            access_exp,
            session_timeout,
            user,
            policy: policy.clone(),
        })
    }

    fn encode_token(&self, claims: Claims, key: &EncodingKey) -> Result<String, AppError> {
        encode(&Header::default(), &claims, key)
            .map_err(|error| AppError::InternalError(format!("token generation failed: {error}")))
    }

    /// Decode and validate a token of the expected kind.
    fn decode_token(&self, token: &str, expected_type: &str) -> Result<Claims, AppError> {
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AppError::Unauthorized("Invalid token".to_string()))?;

        if data.claims.typ != expected_type {
            return Err(AppError::Unauthorized("Invalid token".to_string()));
        }
        Ok(data.claims)
    }

    pub async fn login(&self, username: &str, password: &str) -> Result<LoginResponse, AppError> {
        let session = self.authenticate(username, password).await?;
        Ok(LoginResponse {
            access_token: session.access_token,
            refresh_token: session.refresh_token,
            token_type: "Bearer".to_string(),
            user: session.user.into(),
        })
    }

    pub async fn login_with_vo(
        &self,
        username: &str,
        password: &str,
    ) -> Result<UserLoginData, AppError> {
        let session = self.authenticate(username, password).await?;
        self.build_login_data(session).await
    }

    async fn build_login_data(
        &self,
        session: AuthenticatedSession,
    ) -> Result<UserLoginData, AppError> {
        let user_id = session.user.id;
        let roles = self.get_user_roles(&user_id).await?;
        let permissions = self.get_user_permissions(&user_id).await?;

        let (password_expired, password_expire_date) =
            Self::password_expiry(&session.policy, &session.user);

        Ok(UserLoginData {
            access_token: session.access_token,
            refresh_token: session.refresh_token,
            expires: crate::common::util::format_datetime(
                chrono::DateTime::from_timestamp(session.access_exp as i64, 0)
                    .unwrap_or_else(Utc::now),
            ),
            user_id: Some(user_id),
            username: session.user.username.clone(),
            avatar: None,
            nickname: session.user.real_name.clone(),
            roles,
            permissions,
            password_expired: Some(password_expired),
            password_expire_date,
        })
    }

    async fn get_user_roles(&self, user_id: &i64) -> Result<Vec<i64>, AppError> {
        let roles = self
            .role_repo
            .find_roles_by_user_id(user_id)
            .await
            .map_err(AppError::DatabaseError)?;
        Ok(roles.into_iter().map(|r| r.id).collect())
    }

    async fn get_user_permissions(&self, user_id: &i64) -> Result<Vec<String>, AppError> {
        let roles = self
            .role_repo
            .find_roles_by_user_id(user_id)
            .await
            .map_err(AppError::DatabaseError)?;
        let permissions: Vec<String> = roles.iter().filter_map(|r| r.code.clone()).collect();
        Ok(permissions)
    }
    pub async fn logout(&self, user_id: i64) -> Result<(), AppError> {
        self.token_store.delete_token(&user_id.to_string()).await?;
        self.token_store
            .delete_refresh_token(&user_id.to_string())
            .await
    }

    pub async fn validate_token(&self, token: &str) -> Result<i64, AppError> {
        // Rejects refresh tokens used as access tokens.
        let claims = self.decode_token(token, TOKEN_TYPE_ACCESS)?;
        let user_id = claims.sub;

        let stored_token = self.token_store.get_token(&user_id.to_string()).await?;

        match stored_token {
            Some(stored) if constant_time_eq(stored.as_bytes(), token.as_bytes()) => Ok(user_id),
            _ => Err(AppError::Unauthorized(
                "Token expired or revoked".to_string(),
            )),
        }
    }

    pub fn extract_username(&self, token: &str) -> Option<String> {
        // Signature-only decode: the caller already ran `validate_token`.
        decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .ok()
        .map(|data| data.claims.username)
    }

    /// Validate the refresh token, re-check the account state and rotate both
    /// tokens.
    ///
    /// Shared by [`AuthService::refresh_token`] and
    /// [`AuthService::refresh_token_with_vo`].
    async fn rotate_session(&self, refresh_token: &str) -> Result<AuthenticatedSession, AppError> {
        let claims = self.decode_token(refresh_token, TOKEN_TYPE_REFRESH)?;
        let user_id = claims.sub;

        let user = self
            .user_repo
            .find_by_username(&claims.username)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::Unauthorized("User not found".to_string()))?;

        if user.id != user_id {
            return Err(AppError::Unauthorized("User ID mismatch".to_string()));
        }

        // A refresh token outlives the access token by days, so the account
        // state has to be re-checked here; otherwise a disabled or deleted
        // user could keep minting access tokens until it expired.
        if user.is_deleted != 0 {
            return Err(AppError::Unauthorized("User not found".to_string()));
        }
        if user.enable.is_some_and(|enable| enable != ENABLED) {
            return Err(AppError::AuthError("账户已被禁用".to_string()));
        }
        if user.lock_flag == Some(0) {
            return Err(AppError::Unauthorized("账户已被锁定".to_string()));
        }

        let stored = self
            .token_store
            .get_refresh_token(&user_id.to_string())
            .await?;
        match stored {
            Some(stored) if constant_time_eq(stored.as_bytes(), refresh_token.as_bytes()) => {}
            _ => {
                return Err(AppError::Unauthorized("Invalid refresh token".to_string()));
            }
        }

        let policy = self.security_policy().await;
        self.issue_session(user, &policy).await
    }

    pub async fn refresh_token(&self, refresh_token: &str) -> Result<LoginResponse, AppError> {
        let session = self.rotate_session(refresh_token).await?;
        Ok(LoginResponse {
            access_token: session.access_token,
            refresh_token: session.refresh_token,
            token_type: "Bearer".to_string(),
            user: session.user.into(),
        })
    }

    pub async fn refresh_token_with_vo(
        &self,
        refresh_token: &str,
    ) -> Result<TokenRefreshVO, AppError> {
        let session = self.rotate_session(refresh_token).await?;
        Ok(TokenRefreshVO {
            access_token: session.access_token,
            refresh_token: session.refresh_token,
            expires_in: session.session_timeout as i64,
        })
    }

    pub async fn get_user_info(&self, user_id: i64) -> Result<UserInfoVO, AppError> {
        let user = self
            .user_repo
            .find_by_id(&user_id)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
        Ok(UserInfoVO::from_user(&user))
    }

    /// Returns `true` when the user holds the built-in administrator role.
    pub async fn is_admin(&self, user_id: i64) -> Result<bool, AppError> {
        let roles = self
            .role_repo
            .find_roles_by_user_id(&user_id)
            .await
            .map_err(AppError::DatabaseError)?;
        Ok(roles.iter().any(|role| {
            role.code
                .as_deref()
                .is_some_and(|code| code == crate::system::sys_role::domain::ADMIN_ROLE_CODE)
        }))
    }

    pub async fn check_token(&self, token: &str) -> Result<CheckTokenVO, AppError> {
        let _ = self.validate_token(token).await?;
        Ok(CheckTokenVO::valid())
    }
}

/// Security policy read from `sys_dict_item`, parsed once per login.
#[derive(Debug, Clone)]
struct SecurityPolicy {
    max_try_count: i32,
    max_lock_time_mins: i64,
    session_timeout_secs: u64,
    /// `None` means passwords never expire.
    pass_change_days: Option<i64>,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            max_try_count: 5,
            max_lock_time_mins: 5,
            session_timeout_secs: 24 * 60 * 60,
            pass_change_days: None,
        }
    }
}

/// Result of a successful credential check, before being shaped into a
/// `LoginResponse` or a `UserLoginData`.
struct AuthenticatedSession {
    access_token: String,
    refresh_token: String,
    access_exp: u64,
    session_timeout: u64,
    user: User,
    policy: SecurityPolicy,
}

/// Parse "5", "5次", " 5 次 " into `5`.
///
/// Dictionary values are written by operators and frequently carry a unit
/// suffix or stray whitespace; a strict `parse()` would silently fall back to
/// the default.
fn parse_leading_number<T: std::str::FromStr>(value: &str, unit: &str) -> Option<T> {
    let trimmed = value.trim();
    let digits = trimmed.strip_suffix(unit).unwrap_or(trimmed).trim();
    digits.parse().ok()
}

/// Parse a timeout in seconds.
///
/// Accepts `"3600"` / `"3600秒"` as seconds and `"60分钟"` as minutes. Values
/// that parse to zero or overflow are treated as absent so the caller falls
/// back to its default.
fn parse_timeout_secs(value: &str) -> Option<u64> {
    if value.trim().ends_with("分钟") {
        return parse_leading_number::<u64>(value, "分钟")
            .and_then(|mins| mins.checked_mul(60))
            .filter(|secs| *secs > 0);
    }

    parse_leading_number::<u64>(value, "秒").filter(|secs| *secs > 0)
}

fn invalid_credentials() -> AppError {
    AppError::Unauthorized("用户名或密码错误".to_string())
}

/// Length-agnostic constant-time comparison, used for token equality so that
/// the check does not leak a prefix match via timing.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}
