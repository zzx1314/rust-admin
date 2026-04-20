use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::org::domain::{CreateOrgRequest, Org, OrgTreeDto, OrgTreeQuery, UpdateOrgRequest};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Debug, serde::Deserialize)]
pub struct RemoveByIdsRequest {
    pub ids: Vec<i64>,
}

#[derive(Debug, serde::Serialize)]
pub struct RemoveByIdsResponse {
    pub success: bool,
    pub msg: String,
}

#[derive(Deserialize)]
pub struct OrgIdParam {
    id: i64,
}

#[derive(Deserialize)]
pub struct ParentQueryParam {
    parent_id: Option<i64>,
}

pub async fn create_org_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateOrgRequest>,
) -> Result<Json<ApiResponse<Org>>, AppError> {
    let org = state.org_service.create_org(req).await?;
    Ok(Json(ApiResponse::ok(org)))
}

pub async fn get_org_handler(
    State(state): State<AppState>,
    Path(params): Path<OrgIdParam>,
) -> Result<Json<ApiResponse<Org>>, AppError> {
    let org = state.org_service.get_org(&params.id).await?;
    Ok(Json(ApiResponse::ok(org)))
}

pub async fn get_all_orgs_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<Org>>>, AppError> {
    let orgs = state.org_service.get_all_orgs().await?;
    Ok(Json(ApiResponse::ok(orgs)))
}

pub async fn get_org_tree_handler(
    State(state): State<AppState>,
    Query(query): Query<OrgTreeQuery>,
) -> Result<Json<ApiResponse<Vec<OrgTreeDto>>>, AppError> {
    let orgs = state.org_service.get_org_tree(query).await?;
    Ok(Json(ApiResponse::ok(orgs)))
}

pub async fn get_orgs_by_parent_handler(
    State(state): State<AppState>,
    Query(query): Query<ParentQueryParam>,
) -> Result<Json<ApiResponse<Vec<Org>>>, AppError> {
    let orgs = state
        .org_service
        .get_orgs_by_parent(query.parent_id)
        .await?;
    Ok(Json(ApiResponse::ok(orgs)))
}

pub async fn update_org_handler(
    State(state): State<AppState>,
    Path(params): Path<OrgIdParam>,
    Json(req): Json<UpdateOrgRequest>,
) -> Result<Json<ApiResponse<Org>>, AppError> {
    let org = state.org_service.update_org(&params.id, req).await?;
    Ok(Json(ApiResponse::ok(org)))
}

pub async fn delete_org_handler(
    State(state): State<AppState>,
    Path(params): Path<OrgIdParam>,
) -> Result<(StatusCode, ()), AppError> {
    state.org_service.delete_org(&params.id).await?;
    Ok((StatusCode::NO_CONTENT, ()))
}

pub async fn remove_orgs_by_ids_handler(
    State(state): State<AppState>,
    Json(req): Json<RemoveByIdsRequest>,
) -> Result<Json<RemoveByIdsResponse>, AppError> {
    for id in req.ids {
        state.org_service.delete_org(&id).await?;
    }
    Ok(Json(RemoveByIdsResponse {
        success: true,
        msg: "删除成功".to_string(),
    }))
}