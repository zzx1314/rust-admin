use axum::{extract::Request, middleware::Next, response::Response};

use crate::api::AppState;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;

#[derive(Clone, Debug)]
pub struct RequestUser {
    pub user_id: i64,
    pub username: String,
    pub is_admin: bool,
}

impl RequestUser {
    pub fn data_scope(&self) -> DataScope {
        DataScope::new(self.user_id, self.is_admin)
    }
}

pub async fn require_auth(
    state: axum::extract::State<AppState>,
    mut request: Request,
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
            let is_admin = state.auth_service.is_admin(user_id).await.unwrap_or(false);
            let request_user = RequestUser {
                user_id,
                username,
                is_admin,
            };

            // Handlers read the authenticated user from the request extensions;
            // the audit log middleware reads it back from the response.
            request.extensions_mut().insert(request_user.clone());
            let mut response = next.run(request).await;
            response.extensions_mut().insert(request_user);
            Ok(response)
        }
        _ => Err(AppError::Unauthorized(
            "Missing or invalid token".to_string(),
        )),
    }
}
