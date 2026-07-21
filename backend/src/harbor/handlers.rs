use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use crate::harbor::models::{
    CreateMemberRequest, CreateProjectRequest, HarborMember, HarborProject, HarborRepository,
    ProjectQuery, ProjectSummary,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct ProjectNameParam {
    pub project_name: String,
}

#[derive(Deserialize)]
pub struct MemberIdParam {
    pub project_name: String,
    pub member_id: i64,
}

#[derive(Deserialize)]
pub struct PaginationQuery {
    pub page: Option<i64>,
    #[serde(alias = "page_size")]
    pub page_size: Option<i64>,
}

pub async fn list_projects_handler(
    State(state): State<AppState>,
    Query(query): Query<ProjectQuery>,
) -> Result<Json<ApiResponse<PageResponse<HarborProject>>>, AppError> {
    let result = state.harbor_service.list_projects(&query).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn get_project_summary_handler(
    State(state): State<AppState>,
    Path(params): Path<ProjectNameParam>,
) -> Result<Json<ApiResponse<ProjectSummary>>, AppError> {
    let summary = state
        .harbor_service
        .get_project_summary(&params.project_name)
        .await?;
    Ok(Json(ApiResponse::ok(summary)))
}

pub async fn list_repositories_handler(
    State(state): State<AppState>,
    Path(params): Path<ProjectNameParam>,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<ApiResponse<PageResponse<HarborRepository>>>, AppError> {
    let result = state
        .harbor_service
        .list_repositories(&params.project_name, query.page, query.page_size)
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_members_handler(
    State(state): State<AppState>,
    Path(params): Path<ProjectNameParam>,
    Query(query): Query<PaginationQuery>,
) -> Result<Json<ApiResponse<PageResponse<HarborMember>>>, AppError> {
    let result = state
        .harbor_service
        .list_members(&params.project_name, query.page, query.page_size)
        .await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn create_project_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectRequest>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.harbor_service.create_project(req).await?;
    Ok(Json(ApiResponse::ok(())))
}

pub async fn delete_project_handler(
    State(state): State<AppState>,
    Path(params): Path<ProjectNameParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.harbor_service.delete_project(&params.project_name).await?;
    Ok(Json(ApiResponse::ok(())))
}

pub async fn add_member_handler(
    State(state): State<AppState>,
    Path(params): Path<ProjectNameParam>,
    Json(req): Json<CreateMemberRequest>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .harbor_service
        .add_member(&params.project_name, req)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}

pub async fn remove_member_handler(
    State(state): State<AppState>,
    Path(params): Path<MemberIdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state
        .harbor_service
        .remove_member(&params.project_name, params.member_id)
        .await?;
    Ok(Json(ApiResponse::ok(())))
}
