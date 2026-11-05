use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::common::error::AppError;
use axum::{Extension, Json, extract::State};
use serde::Serialize;

#[derive(Serialize)]
pub struct StatisticsResponse<T: Serialize> {
    pub code: u16,
    pub msg: String,
    pub data: T,
}

impl<T: Serialize> StatisticsResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 10200,
            msg: "success".to_string(),
            data,
        }
    }
}

pub async fn statistics_all_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
) -> Result<Json<StatisticsResponse<Vec<serde_json::Value>>>, AppError> {
    let stats = state
        .dog_statistics_service
        .get_all_statistics(&user.data_scope())
        .await
        .map_err(AppError::DatabaseErrorSeaOrm)?;

    let result: Vec<serde_json::Value> = stats
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "value": s.value,
                "percent": s.percent,
                "data": s.data,
                "bgColor": s.bg_color,
                "color": s.color,
                "duration": s.duration,
            })
        })
        .collect();

    Ok(Json(StatisticsResponse::ok(result)))
}

pub async fn statistics_all_type_handler(
    State(state): State<AppState>,
) -> Result<Json<StatisticsResponse<Vec<serde_json::Value>>>, AppError> {
    let stats = state
        .dog_statistics_service
        .get_weekly_statistics()
        .await
        .map_err(AppError::DatabaseErrorSeaOrm)?;

    let result: Vec<serde_json::Value> = stats
        .into_iter()
        .map(|w| {
            serde_json::json!({
                "deviceCount": w.device_count,
                "modelCount": w.model_count,
                "routeCount": w.route_count,
                "taskCount": w.task_count,
            })
        })
        .collect();

    Ok(Json(StatisticsResponse::ok(result)))
}

pub async fn statistics_device_type_handler(
    State(state): State<AppState>,
) -> Result<Json<StatisticsResponse<serde_json::Value>>, AppError> {
    let data = state
        .dog_statistics_service
        .get_device_type_statistics()
        .await
        .map_err(AppError::DatabaseErrorSeaOrm)?;

    Ok(Json(StatisticsResponse::ok(data)))
}
