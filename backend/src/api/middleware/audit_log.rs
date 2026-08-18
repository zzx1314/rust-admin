use axum::{body::Body, extract::Request, middleware::Next, response::Response};
use http_body_util::BodyExt;
use serde_json::{Value, json};

use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::system::sys_log::domain::CreateSysLogRequest;

const MAX_BODY_LOG_SIZE: usize = 2048;

const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "oldPassword",
    "newPassword",
    "confirmPassword",
    "old_password",
    "new_password",
    "confirm_password",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChangeAction {
    Create,
    Update,
}

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

fn change_action(method: &str, uri: &str) -> Option<ChangeAction> {
    let create_paths = [
        "/api/sysUser",
        "/api/sysRole",
        "/api/sysMenu",
        "/api/sysOrg",
        "/api/sysDict",
        "/api/sysDictItem",
    ];
    if method == "POST" && create_paths.contains(&uri) {
        return Some(ChangeAction::Create);
    }

    if method != "PUT" {
        return None;
    }

    let segments: Vec<&str> = uri.trim_matches('/').split('/').collect();
    if segments.len() == 3
        && segments[0] == "api"
        && matches!(
            segments[1],
            "sysUser" | "sysRole" | "sysMenu" | "sysOrg" | "sysDict" | "sysDictItem"
        )
        && segments[2].parse::<i64>().is_ok()
    {
        return Some(ChangeAction::Update);
    }

    if matches!(
        uri,
        "/api/sysUser/edit" | "/api/sysUser/resetPwd" | "/api/sysUser/enable"
    ) {
        return Some(ChangeAction::Update);
    }

    None
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
        let end = masked
            .char_indices()
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

fn camel_to_snake(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| {
            if ch.is_uppercase() {
                ['_', ch.to_ascii_lowercase()]
                    .into_iter()
                    .collect::<Vec<_>>()
            } else {
                vec![ch]
            }
        })
        .collect()
}

fn value_for_field<'a>(record: Option<&'a Value>, field: &str) -> Option<&'a Value> {
    let object = record?.as_object()?;
    let snake = camel_to_snake(field);
    let candidates = if field == "type" {
        vec![field, snake.as_str(), "type_"]
    } else {
        vec![field, snake.as_str()]
    };
    candidates.into_iter().find_map(|key| object.get(key))
}

fn is_sensitive_field(field: &str) -> bool {
    SENSITIVE_KEYS.contains(&field) || SENSITIVE_KEYS.contains(&camel_to_snake(field).as_str())
}

fn display_value(value: Option<&Value>, sensitive: bool) -> Value {
    if sensitive && value.is_some() {
        Value::String("******".to_string())
    } else {
        value.cloned().unwrap_or(Value::Null)
    }
}

fn build_changes(
    action: ChangeAction,
    before: Option<&Value>,
    after: Option<&Value>,
    submitted: Option<&Value>,
) -> Vec<Value> {
    let Some(fields) = submitted.and_then(Value::as_object) else {
        return Vec::new();
    };

    fields
        .iter()
        .filter_map(|(field, submitted_value)| {
            if submitted_value.is_null() {
                return None;
            }

            let sensitive = is_sensitive_field(field);
            let (old_value, new_value) = match action {
                ChangeAction::Create => (None, Some(submitted_value)),
                ChangeAction::Update => {
                    let new_value = value_for_field(after, field)?;
                    let old_value = value_for_field(before, field);
                    if !sensitive && old_value == Some(new_value) {
                        return None;
                    }
                    (old_value, Some(new_value))
                }
            };

            Some(json!({
                "field": field,
                "old": display_value(old_value, sensitive),
                "new": display_value(new_value, sensitive),
            }))
        })
        .collect()
}

fn format_extra(status: u16, changes: Vec<Value>) -> String {
    if changes.is_empty() {
        return format!("status:{}", status);
    }

    json!({
        "status": status,
        "changes": changes,
    })
    .to_string()
}

async fn load_existing_record(
    state: &AppState,
    uri: &str,
    submitted: Option<&Value>,
) -> Option<Value> {
    let segments: Vec<&str> = uri.trim_matches('/').split('/').collect();
    if segments.len() < 3 || segments[0] != "api" {
        return None;
    }

    let module = segments[1];
    let id = if let Some(id) = segments
        .get(2)
        .and_then(|segment| segment.parse::<i64>().ok())
    {
        id
    } else if module == "sysUser"
        && segments
            .get(2)
            .is_some_and(|segment| matches!(*segment, "edit" | "resetPwd" | "enable"))
    {
        submitted
            .and_then(Value::as_object)
            .and_then(|body| body.get("id").or_else(|| body.get("userId")))
            .and_then(Value::as_i64)?
    } else {
        return None;
    };

    match module {
        "sysUser" => state
            .user_service
            .get_user(&id)
            .await
            .ok()
            .and_then(|record| serde_json::to_value(record).ok()),
        "sysRole" => state
            .role_service
            .get_role(&id)
            .await
            .ok()
            .and_then(|record| serde_json::to_value(record).ok()),
        "sysMenu" => state
            .menu_service
            .get_menu(&id)
            .await
            .ok()
            .and_then(|record| serde_json::to_value(record).ok()),
        "sysOrg" => state
            .org_service
            .get_org(&id)
            .await
            .ok()
            .and_then(|record| serde_json::to_value(record).ok()),
        "sysDict" => state
            .sys_dict_service
            .get_dict(&id)
            .await
            .ok()
            .and_then(|record| serde_json::to_value(record).ok()),
        "sysDictItem" => state
            .sys_dict_item_service
            .get_dict_item(&id)
            .await
            .ok()
            .and_then(|record| serde_json::to_value(record).ok()),
        _ => None,
    }
}

pub async fn audit_log_middleware(
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

    let operation = change_action(&method, &uri);
    let submitted = body_bytes
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
    let before = if operation == Some(ChangeAction::Update) {
        load_existing_record(&state, &uri, submitted.as_ref()).await
    } else {
        None
    };

    let response = next.run(request).await;
    let status = response.status().as_u16();
    let fail = status >= 400;

    let after = if !fail && operation == Some(ChangeAction::Update) {
        load_existing_record(&state, &uri, submitted.as_ref()).await
    } else {
        None
    };
    let changes = match operation {
        Some(action) if !fail => {
            build_changes(action, before.as_ref(), after.as_ref(), submitted.as_ref())
        }
        _ => Vec::new(),
    };

    let operator = response
        .extensions()
        .get::<RequestUser>()
        .map(|u| u.username.clone())
        .or(header_operator);

    let action = match (query.as_ref(), body_bytes.as_ref()) {
        (Some(q), Some(bytes)) => format!(
            "{} {} {}",
            uri,
            mask_sensitive_fields(q),
            format_body_for_log(bytes)
        ),
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
        extra: Some(format_extra(status, changes)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_create_changes_from_submitted_fields() {
        let submitted = json!({"name": "新角色", "password": "secret"});
        let changes = build_changes(ChangeAction::Create, None, None, Some(&submitted));

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0]["old"], Value::Null);
        assert_eq!(changes[0]["new"], "新角色");
        assert_eq!(changes[1]["old"], Value::Null);
        assert_eq!(changes[1]["new"], "******");
    }

    #[test]
    fn build_update_changes_only_keeps_actual_differences() {
        let before = json!({"name": "旧名称", "code": "same"});
        let after = json!({"name": "新名称", "code": "same"});
        let submitted = json!({"name": "新名称", "code": "same"});
        let changes = build_changes(
            ChangeAction::Update,
            Some(&before),
            Some(&after),
            Some(&submitted),
        );

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["field"], "name");
        assert_eq!(changes[0]["old"], "旧名称");
        assert_eq!(changes[0]["new"], "新名称");
    }

    #[test]
    fn camel_case_fields_match_serialized_records() {
        let before = json!({"parent_id": 1, "type_": "menu"});
        let after = json!({"parent_id": 2, "type_": "menu"});
        let submitted = json!({"parentId": 2, "type": "menu"});
        let changes = build_changes(
            ChangeAction::Update,
            Some(&before),
            Some(&after),
            Some(&submitted),
        );

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0]["field"], "parentId");
    }
}
