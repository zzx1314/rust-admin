use crate::api::AppState;
use crate::business::dog::config::domain::{
    ConfigPageQuery, ConfigVO, CreateConfigRequest, UpdateConfigRequest,
};
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use axum::{
    Json,
    extract::{Path, Query, State},
};

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

pub async fn create_config_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateConfigRequest>,
) -> Result<Json<ApiResponse<ConfigVO>>, AppError> {
    let config = state.dog_config_service.create(req).await?;
    Ok(Json(ApiResponse::ok(ConfigVO::from(config))))
}

pub async fn get_config_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<ConfigVO>>, AppError> {
    let config = state.dog_config_service.get_by_id(params.id).await?;
    Ok(Json(ApiResponse::ok(ConfigVO::from(config))))
}

pub async fn list_configs_handler(
    State(state): State<AppState>,
    Query(query): Query<ConfigPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<ConfigVO>>>, AppError> {
    let result = state.dog_config_service.list_page(query).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_configs_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<ConfigVO>>>, AppError> {
    let configs = state.dog_config_service.list_all().await?;
    Ok(Json(ApiResponse::ok(
        configs.into_iter().map(ConfigVO::from).collect(),
    )))
}

pub async fn update_config_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateConfigRequest>,
) -> Result<Json<ApiResponse<ConfigVO>>, AppError> {
    let config = state.dog_config_service.update(params.id, req).await?;
    Ok(Json(ApiResponse::ok(ConfigVO::from(config))))
}

pub async fn delete_config_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_config_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}
