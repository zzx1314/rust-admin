//! 中台 → 业务平台内部接口（控制面）。
//!
//! 中台不直连设备，也不持有设备 WebSocket 连接：所有「对设备做动作」的请求
//! 都在这里统一转发给业务平台，由业务平台的设备网关落地。集中到一处的收益：
//! 超时可控（连接复用）、错误语义一致（4xx 原样透出给运维、5xx 收敛成内部错误）、
//! 幂等键与操作人随请求透传（业务平台侧才能留痕）。

use crate::api::AppState;
use crate::common::error::{AppError, SUCCESS_CODE};
use serde_json::Value;

/// 共享服务令牌请求头，值与业务平台 `ROBODOG_SERVICE_TOKEN` 一致。
pub const SERVICE_TOKEN_HEADER: &str = "x-service-token";
/// 操作人（中台账号 id）：共享令牌本身没有身份，服务端之间靠它留痕。
pub const OPERATOR_HEADER: &str = "x-operator-id";
/// 幂等键：同一次中台操作重试时保持不变。
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// 业务平台设备内部接口路径，设备编号按 URL 段编码。
pub fn device_path(device_code: &str, suffix: &str) -> String {
    format!(
        "/api/zentai/devices/{}/{}",
        urlencoding::encode(device_code.trim()),
        suffix
    )
}

fn endpoint(state: &AppState) -> Result<(&str, &str), AppError> {
    let base = state
        .business_api_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(
                "未配置业务平台地址（[dog] business_api_url 或 APP_DOG__BUSINESS_API_URL）"
                    .to_string(),
            )
        })?;
    let token = state
        .business_api_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(
                "未配置业务平台服务令牌（APP_DOG__BUSINESS_API_TOKEN）".to_string(),
            )
        })?;
    Ok((base, token))
}

/// `POST` 业务平台内部接口，返回响应里的 `data`。
pub async fn post(
    state: &AppState,
    path: &str,
    request_id: &str,
    operator_id: i64,
    body: Value,
) -> Result<Value, AppError> {
    let (base, token) = endpoint(state)?;
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let request = state
        .business_http
        .post(&url)
        .header(SERVICE_TOKEN_HEADER, token)
        .header(REQUEST_ID_HEADER, request_id)
        .header(OPERATOR_HEADER, operator_id.to_string())
        .json(&body);
    send(request, &url).await
}

/// `GET` 业务平台内部接口，返回响应里的 `data`。
pub async fn get(state: &AppState, path: &str) -> Result<Value, AppError> {
    let (base, token) = endpoint(state)?;
    let url = format!("{}{}", base.trim_end_matches('/'), path);
    let request = state
        .business_http
        .get(&url)
        .header(SERVICE_TOKEN_HEADER, token);
    send(request, &url).await
}

async fn send(request: reqwest::RequestBuilder, url: &str) -> Result<Value, AppError> {
    let response = request
        .send()
        .await
        .map_err(|error| AppError::InternalError(format!("调用业务平台失败（{url}）: {error}")))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();

    if status.is_success() {
        let value: Value = serde_json::from_str(&text).map_err(|error| {
            AppError::InternalError(format!("业务平台响应不是 JSON（{url}）: {error}"))
        })?;
        // 业务平台统一 `{ code, msg, data }`；非成功码说明是业务拒绝
        if let Some(code) = value.get("code").and_then(Value::as_u64)
            && code != u64::from(SUCCESS_CODE)
        {
            return Err(AppError::BadRequest(
                message_of(&value).unwrap_or_else(|| "业务平台返回失败".to_string()),
            ));
        }
        return Ok(value.get("data").cloned().unwrap_or(Value::Null));
    }

    let message = message_of(&serde_json::from_str::<Value>(&text).unwrap_or(Value::Null))
        .unwrap_or_else(|| text.trim().to_string());
    if status.is_client_error() {
        // 设备离线 / 未绑园区 / 任务冲突 / 指令非法：业务平台的消息本身就是给运维看的
        return Err(AppError::BadRequest(if message.is_empty() {
            format!("业务平台拒绝请求（HTTP {status}）")
        } else {
            message
        }));
    }
    Err(AppError::InternalError(format!(
        "业务平台异常（HTTP {status}）：{message}"
    )))
}

fn message_of(value: &Value) -> Option<String> {
    value
        .get("msg")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|msg| !msg.is_empty())
        .map(str::to_string)
}
