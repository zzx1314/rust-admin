use axum::{extract::Request, middleware::Next, response::Response};

use crate::api::AppState;
use crate::common::error::AppError;

pub async fn require_auth(
    state: axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            state.auth_service.validate_token(token).await?;
            Ok(next.run(request).await)
        }
        _ => Err(AppError::Unauthorized(
            "Missing or invalid token".to_string(),
        )),
    }
}
