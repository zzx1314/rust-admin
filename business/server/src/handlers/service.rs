//! 中台 → 业务平台内部接口。
//!
//! 中台设备页的「同步地图」按钮不能直连设备：它调用这里，由业务平台通过
//! 已建立的设备 WebSocket 下发 `同步地图` 指令。接口用共享服务令牌鉴权，
//! 令牌只在两个服务端之间配置，不下发到浏览器或设备。

use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::gateway;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{Value, json};

/// 服务令牌请求头（与中台 `APP_DOG__BUSINESS_API_TOKEN` 对应）
const SERVICE_TOKEN_HEADER: &str = "x-service-token";

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SyncMapsRequest {
    /// 保留字段：manual（中台手动触发）/ auto（设备自动回传）
    #[serde(default)]
    pub mode: Option<String>,
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let expected = state.config.service_token.as_deref().ok_or_else(|| {
        AppError::Internal(
            "业务平台未配置服务令牌（ROBODOG_SERVICE_TOKEN），无法接受中台内部请求".to_string(),
        )
    })?;
    let provided = headers
        .get(SERVICE_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if provided.is_empty() || provided != expected {
        return Err(AppError::Unauthorized("服务令牌无效".to_string()));
    }
    Ok(())
}

/// `POST /api/zentai/devices/{device_id}/sync-maps`
///
/// 中台设备列表点击「同步地图」→ 这里向设备下发 `同步地图` 指令 →
/// 客户端用 4012 拉地图列表、从文件服务下载缺失地图并回传。
pub async fn sync_device_maps(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    payload: Option<Json<SyncMapsRequest>>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    authorize(&state, &headers)?;
    let mode = payload
        .and_then(|Json(request)| request.mode)
        .unwrap_or_else(|| "manual".to_string());

    let device: Option<(i64, Option<i64>, String)> =
        sqlx::query_as("SELECT id, park_id, status FROM devices WHERE device_id = ?1 LIMIT 1")
            .bind(device_id.trim())
            .fetch_optional(&state.db)
            .await?;
    let Some((device_pk, park_id, status)) = device else {
        return Err(AppError::NotFound(format!(
            "设备 {} 不在业务平台台账中",
            device_id.trim()
        )));
    };
    let Some(park_id) = park_id else {
        return Err(AppError::BadRequest(
            "设备尚未绑定园区，无法确定地图归属；请先在业务平台绑定园区".to_string(),
        ));
    };

    let timestamp = now();
    let dispatched = gateway::send_command_to_device(
        &state.device_connections,
        device_id.trim(),
        "同步地图",
        Some(json!({
            "mode": mode,
            "onlyMissing": true,
            "parkId": park_id,
            "requestedAt": timestamp,
        })),
    )
    .await;
    if !dispatched {
        return Err(AppError::BadRequest(format!(
            "设备 {} 当前未连接（状态：{}），无法下发同步指令",
            device_id.trim(),
            status
        )));
    }

    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('map_sync', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(format!("中台触发地图同步：{}", device_id.trim()))
    .bind(format!("模式：{mode}；园区：{park_id}；只同步缺失地图"))
    .bind(device_pk)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;

    Ok(ApiResponse::ok(json!({
        "deviceId": device_id.trim(),
        "parkId": park_id,
        "mode": mode,
        "dispatched": true,
        "requestedAt": timestamp,
    })))
}
