use crate::api::AppState;
use crate::business::dog::command::domain::{
    CommandPageQuery, CommandVO, CreateCommandRequest, UpdateCommandRequest,
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

pub async fn create_command_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateCommandRequest>,
) -> Result<Json<ApiResponse<CommandVO>>, AppError> {
    let command = state.dog_command_service.create(req).await?;
    Ok(Json(ApiResponse::ok(CommandVO::from(command))))
}

pub async fn get_command_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<CommandVO>>, AppError> {
    let command = state.dog_command_service.get_by_id(params.id).await?;
    Ok(Json(ApiResponse::ok(CommandVO::from(command))))
}

pub async fn list_commands_handler(
    State(state): State<AppState>,
    Query(query): Query<CommandPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<CommandVO>>>, AppError> {
    let result = state.dog_command_service.list_page(query).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_commands_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<CommandVO>>>, AppError> {
    let commands = state.dog_command_service.list_all().await?;
    Ok(Json(ApiResponse::ok(
        commands.into_iter().map(CommandVO::from).collect(),
    )))
}

pub async fn update_command_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateCommandRequest>,
) -> Result<Json<ApiResponse<CommandVO>>, AppError> {
    let command = state.dog_command_service.update(params.id, req).await?;
    Ok(Json(ApiResponse::ok(CommandVO::from(command))))
}

pub async fn update_command_status_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateCommandRequest>,
) -> Result<Json<ApiResponse<CommandVO>>, AppError> {
    let status = req.status.unwrap_or_else(|| "pending".to_string());
    let command = state
        .dog_command_service
        .update_status(params.id, &status, req.return_value)
        .await?;
    Ok(Json(ApiResponse::ok(CommandVO::from(command))))
}

pub async fn delete_command_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_command_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}
