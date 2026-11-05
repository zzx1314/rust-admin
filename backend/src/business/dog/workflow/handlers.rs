use crate::api::AppState;
use crate::business::dog::workflow::domain::{
    CreateWorkflowRequest, UpdateWorkflowRequest, WorkflowVO,
};
use crate::common::error::{ApiResponse, AppError};
use axum::{
    Json,
    extract::{Path, State},
};

#[derive(serde::Deserialize)]
pub struct IdParam {
    pub id: i64,
}

#[derive(serde::Deserialize)]
pub struct UuidParam {
    pub uuid: String,
}

pub async fn create_workflow_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkflowRequest>,
) -> Result<Json<ApiResponse<WorkflowVO>>, AppError> {
    let workflow = state.dog_workflow_service.create(req).await?;
    Ok(Json(ApiResponse::ok(WorkflowVO::from(workflow))))
}

pub async fn get_workflow_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<WorkflowVO>>, AppError> {
    let workflow = state.dog_workflow_service.get_by_id(params.id).await?;
    Ok(Json(ApiResponse::ok(WorkflowVO::from(workflow))))
}

pub async fn get_workflow_by_uuid_handler(
    State(state): State<AppState>,
    Path(params): Path<UuidParam>,
) -> Json<ApiResponse<Option<WorkflowVO>>> {
    match state.dog_workflow_service.get_by_uuid(&params.uuid).await {
        Ok(workflow) => Json(ApiResponse::ok(Some(WorkflowVO::from(workflow)))),
        Err(_) => Json(ApiResponse::ok(None)),
    }
}

pub async fn update_workflow_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateWorkflowRequest>,
) -> Result<Json<ApiResponse<WorkflowVO>>, AppError> {
    let workflow = state.dog_workflow_service.update(params.id, req).await?;
    Ok(Json(ApiResponse::ok(WorkflowVO::from(workflow))))
}

pub async fn delete_workflow_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_workflow_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}
