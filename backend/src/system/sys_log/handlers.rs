use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use crate::system::sys_log::domain::{
    CreateSysLogRequest, SysLog, SysLogPageQuery, SysLogVO, UpdateSysLogRequest,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct LogIdParam {
    id: i64,
}

pub async fn create_log_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateSysLogRequest>,
) -> Result<Json<ApiResponse<SysLog>>, AppError> {
    let log = state.sys_log_service.create_log(req).await?;
    Ok(Json(ApiResponse::ok(log)))
}

pub async fn get_log_handler(
    State(state): State<AppState>,
    Path(params): Path<LogIdParam>,
) -> Result<Json<ApiResponse<SysLog>>, AppError> {
    let log = state.sys_log_service.get_log(&params.id).await?;
    Ok(Json(ApiResponse::ok(log)))
}

pub async fn get_all_logs_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<SysLog>>>, AppError> {
    let logs = state.sys_log_service.get_all_logs().await?;
    Ok(Json(ApiResponse::ok(logs)))
}

pub async fn get_logs_page_handler(
    State(state): State<AppState>,
    Query(query): Query<SysLogPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<SysLogVO>>>, AppError> {
    let page_response = state.sys_log_service.get_logs_page(query).await?;
    Ok(Json(ApiResponse::ok(page_response)))
}

pub async fn update_log_handler(
    State(state): State<AppState>,
    Path(params): Path<LogIdParam>,
    Json(req): Json<UpdateSysLogRequest>,
) -> Result<Json<ApiResponse<SysLog>>, AppError> {
    let log = state.sys_log_service.update_log(&params.id, req).await?;
    Ok(Json(ApiResponse::ok(log)))
}

pub async fn delete_log_handler(
    State(state): State<AppState>,
    Path(params): Path<LogIdParam>,
) -> Result<(StatusCode, ()), AppError> {
    state.sys_log_service.delete_log(&params.id).await?;
    Ok((StatusCode::NO_CONTENT, ()))
}