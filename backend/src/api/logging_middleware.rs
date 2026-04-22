use axum::{body::Body, extract::Request, middleware::Next, response::Response};
use http_body_util::BodyExt;

use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::system::sys_log::domain::CreateSysLogRequest;

const MAX_BODY_LOG_SIZE: usize = 2048;

const SENSITIVE_KEYS: &[&str] = &[
    "password", "oldPassword", "newPassword", "confirmPassword",
    "old_password", "new_password", "confirm_password",
];

fn extract_biz_type(uri: &str) -> String {
    let mut segments = uri.split('/');
    segments.next();
    let prefix = segments.next().unwrap_or("");
    let module = segments.next().unwrap_or("");
    if prefix == "api" && !module.is_empty() {
        match module {
            "sysUser" => "USER",
            "sysRole" => "ROLE",
            "sysMenu" => "MENU",
            "sysOrg" => "ORG",
            "sysAuth" => "AUTH",
            "sysDict" => "DICT",
            "sysDictItem" => "DICT_ITEM",
            "sysLog" => "LOG",
            "token" => "AUTH",
            _ => "OTHER",
        }
        .to_string()
    } else {
        "OTHER".to_string()
    }
}

fn mask_sensitive_fields(body: &str) -> String {
    if let Ok(mut map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(body) {
        for key in SENSITIVE_KEYS {
            if let Some(v) = map.get_mut(*key) {
                *v = serde_json::Value::String("******".to_string());
            }
        }
        serde_json::to_string(&map).unwrap_or_else(|_| body.to_string())
    } else {
        mask_form_encoded(body)
    }
}

fn mask_form_encoded(body: &str) -> String {
    let mut result = body.to_string();
    for key in SENSITIVE_KEYS {
        let key_eq = format!("{}=", key);
        let amp_key = format!("&{}=", key);
        let mut search_from = 0;
        while search_from < result.len() {
            let val_start = if search_from == 0 && result.starts_with(&key_eq) {
                key_eq.len()
            } else if let Some(pos) = result[search_from..].find(&amp_key) {
                search_from + pos + amp_key.len()
            } else {
                break;
            };
            let val_end = result[val_start..]
                .find('&')
                .map(|i| val_start + i)
                .unwrap_or(result.len());
            result.replace_range(val_start..val_end, "******");
            search_from = val_start + 6;
        }
    }
    result
}

fn format_body_for_log(bytes: &[u8]) -> String {
    let body_str = std::str::from_utf8(bytes).unwrap_or("<binary>");
    let masked = mask_sensitive_fields(body_str);
    if masked.len() > MAX_BODY_LOG_SIZE {
        let end = masked.char_indices()
            .take_while(|(i, _)| *i < MAX_BODY_LOG_SIZE)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}...(truncated)", &masked[..end])
    } else {
        masked
    }
}

fn should_skip_body(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.starts_with("multipart/form-data"))
        .unwrap_or(false)
}

pub async fn logging_middleware(
    state: axum::extract::State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let uri = request.uri().path().to_string();
    let query = request.uri().query().map(String::from);
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

    let skip_body = should_skip_body(request.headers());
    let body_bytes = if skip_body {
        None
    } else {
        match request.body_mut().collect().await {
            Ok(collected) => Some(collected.to_bytes()),
            Err(_) => None,
        }
    };

    if let Some(ref bytes) = body_bytes {
        *request.body_mut() = Body::from(bytes.clone());
    }

    let response = next.run(request).await;
    let status = response.status().as_u16();
    let fail = status >= 400;

    let operator = response
        .extensions()
        .get::<RequestUser>()
        .map(|u| u.username.clone())
        .or(header_operator);

    let action = match (query.as_ref(), body_bytes.as_ref()) {
        (Some(q), Some(bytes)) => format!("{} {} {}", uri, mask_sensitive_fields(q), format_body_for_log(bytes)),
        (Some(q), None) => format!("{} {}", uri, mask_sensitive_fields(q)),
        (None, Some(bytes)) => format!("{} {}", uri, format_body_for_log(bytes)),
        (None, None) => uri.clone(),
    };

    let biz_type = extract_biz_type(&uri);
    let log_service = state.sys_log_service.clone();
    let log_req = CreateSysLogRequest {
        tenant: None,
        type_: Some(biz_type),
        sub_type: Some(method),
        biz_no: Some(uri),
        operator,
        action: Some(action),
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