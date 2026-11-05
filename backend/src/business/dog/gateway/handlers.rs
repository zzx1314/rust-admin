use axum::{
    Json,
    extract::{
        Path, State,
        ws::{WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
};

use crate::api::AppState;
use crate::business::dog::device::domain::DeviceVO;
use crate::business::dog::gateway::domain::{
    ConnectionInfo, DEVICE_TOKEN_TTL_SECS, DeviceRegistration, DeviceTokenResponse,
    DisconnectResult, DispatchRequest, DispatchResult,
};
use crate::business::dog::gateway::session;
use crate::common::error::{ApiResponse, AppError};

/// `GET /api/dogGateway/auth/{deviceId}` — issue a device access token.
pub async fn device_auth_handler(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<Json<ApiResponse<DeviceTokenResponse>>, AppError> {
    if device_id.trim().is_empty() {
        return Err(AppError::BadRequest(
            "deviceId must not be empty".to_string(),
        ));
    }
    let token = state.dog_gateway.issue_token(&device_id).await;
    tracing::info!(device_id = %device_id, "device token issued");
    Ok(Json(ApiResponse::ok(DeviceTokenResponse {
        device_id,
        token,
        expires_in: DEVICE_TOKEN_TTL_SECS,
    })))
}

/// `POST /api/dogGateway/register` — register a device using its access token.
pub async fn register_device_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(registration): Json<DeviceRegistration>,
) -> Result<Json<ApiResponse<DeviceVO>>, AppError> {
    let token = device_token_from_headers(&headers)
        .ok_or_else(|| AppError::Unauthorized("missing device token".to_string()))?;

    if !state
        .dog_gateway
        .authorize(&registration.device_id, &token)
        .await
    {
        return Err(AppError::Unauthorized(
            "invalid or expired device token".to_string(),
        ));
    }

    let device = state.dog_gateway.register_device(registration).await?;
    Ok(Json(ApiResponse::ok(DeviceVO::from(device))))
}

/// `GET /api/notice/{deviceId}/{token}` — device WebSocket channel.
///
/// Kept at the original path so existing device clients keep working.
pub async fn device_notice_handler(
    State(state): State<AppState>,
    Path((device_id, token)): Path<(String, String)>,
    websocket: WebSocketUpgrade,
) -> Response {
    if !state.dog_gateway.authorize(&device_id, &token).await {
        tracing::warn!(device_id = %device_id, "device websocket rejected: invalid token");
        return AppError::Unauthorized("invalid or expired device token".to_string())
            .into_response();
    }

    websocket.on_upgrade(move |socket: WebSocket| session::run(state, device_id, socket))
}

/// `POST /api/dogGateway/dispatch` — push an arbitrary payload to a device.
pub async fn dispatch_handler(
    State(state): State<AppState>,
    Json(request): Json<DispatchRequest>,
) -> Result<Json<ApiResponse<DispatchResult>>, AppError> {
    if request.device_id.trim().is_empty() {
        return Err(AppError::BadRequest(
            "deviceId must not be empty".to_string(),
        ));
    }
    let result = state
        .dog_gateway
        .dispatch_raw(&request.device_id, &request.payload)
        .await;
    Ok(Json(ApiResponse::ok(result)))
}

/// `POST /api/dogGateway/disconnect/{deviceId}` — close a device connection.
///
/// The device client reconnects automatically, which makes this useful to force
/// a fresh registration/token (maintenance) and to test reconnect behaviour.
pub async fn disconnect_device_handler(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
) -> Result<Json<ApiResponse<DisconnectResult>>, AppError> {
    let disconnected = state.dog_gateway.disconnect(&device_id).await;
    Ok(Json(ApiResponse::ok(DisconnectResult {
        device_id,
        disconnected,
    })))
}

/// `GET /api/dogGateway/connections` — currently connected devices.
pub async fn connections_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<ConnectionInfo>>>, AppError> {
    let connections = state.dog_gateway.connections().await;
    Ok(Json(ApiResponse::ok(connections)))
}

/// Accept both `Authorization: <token>` (original client) and
/// `Authorization: Bearer <token>`.
fn device_token_from_headers(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .trim();
    if raw.is_empty() {
        return None;
    }
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .unwrap_or(raw)
        .trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}
