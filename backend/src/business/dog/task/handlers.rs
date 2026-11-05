use crate::api::AppState;
use crate::business::dog::task::domain::{
    CreateTaskRequest, TaskPageQuery, TaskVO, UpdateTaskRequest,
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

pub async fn create_task_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let task = state.dog_task_service.create(req).await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn get_task_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let task = state.dog_task_service.get_by_id(params.id).await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn list_tasks_handler(
    State(state): State<AppState>,
    Query(query): Query<TaskPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<TaskVO>>>, AppError> {
    let result = state.dog_task_service.list_page(query).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_tasks_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<TaskVO>>>, AppError> {
    let tasks = state.dog_task_service.list_all().await?;
    Ok(Json(ApiResponse::ok(
        tasks.into_iter().map(TaskVO::from).collect(),
    )))
}

pub async fn update_task_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateTaskRequest>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let task = state.dog_task_service.update(params.id, req).await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn update_task_status_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateTaskRequest>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let status = req.task_status.unwrap_or_else(|| "pending".to_string());
    let task = state
        .dog_task_service
        .update_status(params.id, &status)
        .await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn delete_task_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_task_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}
