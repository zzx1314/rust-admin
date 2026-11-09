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
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, 10500),
        };
        let message = self.to_string();
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %message, "request failed");
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
