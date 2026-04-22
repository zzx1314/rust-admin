use axum::{extract::Request, middleware::Next, response::Response};

use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::system::sys_log::domain::CreateSysLogRequest;

pub async fn logging_middleware(
    state: axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let uri = request.uri().path().to_string();
    let ip = request
        .headers()
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let header_operator = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.starts_with("Bearer "))
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|token| state.auth_service.extract_username(token));

    if method == "GET" {
        return next.run(request).await;
    }

    let response = next.run(request).await;
    let status = response.status().as_u16();
    let fail = status >= 400;

    let operator = response
        .extensions()
        .get::<RequestUser>()
        .map(|u| u.username.clone())
        .or(header_operator);

    let log_service = state.sys_log_service.clone();
    let log_req = CreateSysLogRequest {
        tenant: None,
        type_: Some("HTTP".to_string()),
        sub_type: Some(method),
        biz_no: Some(uri.clone()),
        operator,
        action: Some(uri),
        fail: Some(fail),
        extra: Some(format!("status:{}", status)),
        code_variable: None,
        ip,
    };

    tokio::spawn(async move {
        if let Err(e) = log_service.create_log(log_req).await {
            tracing::error!("Failed to create log: {}", e);
        }
    });

    response
}