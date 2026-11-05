use crate::api::AppState;
use crate::api::middleware::RequestUser;
use crate::business::dog::task::domain::{
    CreateTaskRequest, TaskPageQuery, TaskStatusVO, TaskVO, UpdateTaskRequest,
};
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

pub async fn create_task_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Json(req): Json<CreateTaskRequest>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let task = state
        .dog_task_service
        .create(req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn get_task_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let task = state
        .dog_task_service
        .get_by_id(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn list_tasks_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Query(query): Query<TaskPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<TaskVO>>>, AppError> {
    let result = state
        .dog_task_service
        .list_page(query, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_tasks_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
) -> Result<Json<ApiResponse<Vec<TaskVO>>>, AppError> {
    let tasks = state.dog_task_service.list_all(&user.data_scope()).await?;
    Ok(Json(ApiResponse::ok(
        tasks.into_iter().map(TaskVO::from).collect(),
    )))
}

pub async fn update_task_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateTaskRequest>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let task = state
        .dog_task_service
        .update(params.id, req, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn update_task_status_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateTaskRequest>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let status = req.task_status.unwrap_or_else(|| "pending".to_string());
    let task = state
        .dog_task_service
        .update_status(params.id, &status, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn delete_task_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .dog_task_service
        .delete(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

pub async fn start_task_handler(
    State(state): State<AppState>,
    Extension(user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<TaskVO>>, AppError> {
    let task = state
        .dog_task_service
        .start_task(params.id, &user.data_scope())
        .await?;
    Ok(Json(ApiResponse::ok(TaskVO::from(task))))
}

pub async fn get_task_status_handler(
    State(state): State<AppState>,
    Extension(_user): Extension<RequestUser>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<TaskStatusVO>>, AppError> {
    let status = state
        .dog_task_service
        .get_task_status(params.id)
        .await?;
    Ok(Json(ApiResponse::ok(status)))
}
