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

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error("Database error: {0}")]
    DatabaseErrorSeaOrm(#[from] sea_orm::DbErr),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Authentication error: {0}")]
    AuthError(String),

    #[error("Bad request: {0}")]
    BadRequest(String),
}

impl AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::ValidationError(_) => StatusCode::UNPROCESSABLE_ENTITY,
            AppError::DatabaseError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::DatabaseErrorSeaOrm(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::AuthError(_) => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn error_code(&self) -> u16 {
        match self {
            AppError::NotFound(_) => FAIL_CODE,
            AppError::ValidationError(_) => FAIL_CODE,
            AppError::DatabaseError(_) => FAIL_CODE,
            AppError::DatabaseErrorSeaOrm(_) => FAIL_CODE,
            AppError::Unauthorized(_) => FAIL_CODE,
            AppError::AuthError(_) => FAIL_CODE,
            AppError::BadRequest(_) => FAIL_CODE,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        let code = self.error_code();
        let msg = self.to_string();

        let body = Json(ApiResponse::error(code, msg));

        (status, body).into_response()
    }
}
