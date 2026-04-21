use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use crate::system::sys_dict::domain::{
    CreateSysDictRequest, SysDict, SysDictPageQuery, SysDictVO, UpdateSysDictRequest,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct DictIdParam {
    id: i64,
}

pub async fn create_dict_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateSysDictRequest>,
) -> Result<Json<ApiResponse<SysDict>>, AppError> {
    let dict = state.sys_dict_service.create_dict(req).await?;
    Ok(Json(ApiResponse::ok(dict)))
}

pub async fn get_dict_handler(
    State(state): State<AppState>,
    Path(params): Path<DictIdParam>,
) -> Result<Json<ApiResponse<SysDict>>, AppError> {
    let dict = state.sys_dict_service.get_dict(&params.id).await?;
    Ok(Json(ApiResponse::ok(dict)))
}

pub async fn get_all_dicts_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<SysDict>>>, AppError> {
    let dicts = state.sys_dict_service.get_all_dicts().await?;
    Ok(Json(ApiResponse::ok(dicts)))
}

pub async fn get_dicts_page_handler(
    State(state): State<AppState>,
    Query(query): Query<SysDictPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<SysDictVO>>>, AppError> {
    let page_response = state.sys_dict_service.get_dicts_page(query).await?;
    Ok(Json(ApiResponse::ok(page_response)))
}

pub async fn update_dict_handler(
    State(state): State<AppState>,
    Path(params): Path<DictIdParam>,
    Json(req): Json<UpdateSysDictRequest>,
) -> Result<Json<ApiResponse<SysDict>>, AppError> {
    let dict = state.sys_dict_service.update_dict(&params.id, req).await?;
    Ok(Json(ApiResponse::ok(dict)))
}

pub async fn delete_dict_handler(
    State(state): State<AppState>,
    Path(params): Path<DictIdParam>,
) -> Result<(StatusCode, ()), AppError> {
    state.sys_dict_service.delete_dict(&params.id).await?;
    Ok((StatusCode::NO_CONTENT, ()))
}
