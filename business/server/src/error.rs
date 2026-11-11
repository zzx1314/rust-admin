use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    /// 上游不可用（中台连不上 / 返回了无法理解的内容）
    ///
    /// 与 `Internal` 分开是为了**登录**这类场景：账号密码错是中台给出的 401，
    /// 中台本身连不上不该报成 500「服务器错误」，否则现场会以为业务平台崩了。
    #[error("{0}")]
    Upstream(String),
    #[error("{0}")]
    Internal(String),
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::NotFound("记录不存在".to_string()),
            other => Self::Internal(other.to_string()),
        }
    }
}

impl From<bcrypt::BcryptError> for AppError {
    fn from(error: bcrypt::BcryptError) -> Self {
        Self::Internal(error.to_string())
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(_: jsonwebtoken::errors::Error) -> Self {
        Self::Unauthorized("登录状态已失效，请重新登录".to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, 10400),
            Self::Unauthorized(_) => (StatusCode::UNAUTHORIZED, 10401),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, 10404),
            Self::Upstream(_) => (StatusCode::SERVICE_UNAVAILABLE, 10503),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, 10500),
        };
        let message = self.to_string();
        match status {
            StatusCode::INTERNAL_SERVER_ERROR => tracing::error!(error = %message, "request failed"),
            // 上游不可用是部署/网络问题，按 warn 记，别刷成 error 噪音
            StatusCode::SERVICE_UNAVAILABLE => tracing::warn!(error = %message, "upstream unavailable"),
            _ => {}
        }
        (
            status,
            Json(json!({
                "code": code,
                "msg": message,
                "data": null
            })),
        )
            .into_response()
    }
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: u16,
    pub msg: String,
    pub data: T,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Json<Self> {
        Json(Self {
            code: 10200,
            msg: "success".to_string(),
            data,
        })
    }
}
