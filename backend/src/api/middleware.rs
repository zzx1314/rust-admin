use axum::{extract::Request, middleware::Next, response::Response};

use crate::api::AppState;
use crate::common::error::AppError;

#[derive(Clone, Debug)]
pub struct RequestUser {
    pub user_id: i64,
    pub username: String,
}

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
            let user_id = state.auth_service.validate_token(token).await?;
            let username = state
                .auth_service
                .extract_username(token)
                .unwrap_or_default();
            let mut response = next.run(request).await;
            response.extensions_mut().insert(RequestUser { user_id, username });
            Ok(response)
        }
        _ => Err(AppError::Unauthorized(
            "Missing or invalid token".to_string(),
        )),
    }
}
