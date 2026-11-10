use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

pub const SUCCESS_CODE: u16 = 10200;
pub const FAIL_CODE: u16 = 10400;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub code: u16,
    pub msg: String,
    pub data: T,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: SUCCESS_CODE,
            msg: "success".to_string(),
            data,
        }
    }
}

impl ApiResponse<serde_json::Value> {
    pub fn error(code: u16, msg: String) -> Self {
        Self {
            code,
            msg,
            data: serde_json::Value::Null,
        }
    }
}

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    /// Database failures. The inner error is logged, never returned to the
    /// client (see [`AppError::client_message`]).
    #[error("Database error: {0}")]
    DatabaseError(#[from] sea_orm::DbErr),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Authentication error: {0}")]
    AuthError(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

impl AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::ValidationError(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::DatabaseError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::AuthError(_) => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// `true` when the detail must not leave the server: the message may expose
    /// SQL, file paths or other internals.
    fn is_internal(&self) -> bool {
        matches!(
            self,
            AppError::DatabaseError(_) | AppError::InternalError(_)
        )
    }

    /// Message safe to return to the client.
    ///
    /// Domain errors (validation, conflict, auth…) carry messages that were
    /// written for the user and are passed through. Internal errors are
    /// replaced with a generic message; the detail goes to the log instead.
    fn client_message(&self) -> String {
        if self.is_internal() {
            tracing::error!(error = %self, "internal error");
            INTERNAL_ERROR_MESSAGE.to_string()
        } else {
            self.to_string()
        }
    }
}

/// Generic body returned for internal failures.
const INTERNAL_ERROR_MESSAGE: &str = "服务内部错误，请稍后重试";

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let msg = self.client_message();

        let body = Json(ApiResponse::error(FAIL_CODE, msg));

        (status, body).into_response()
    }
}
