use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use crate::system::sys_dict_item::domain::{
    CreateSysDictItemRequest, SysDictItem, SysDictItemPageQuery, SysDictItemVO,
    UpdateSysDictItemRequest,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct DictItemIdParam {
    id: i64,
}

#[derive(Deserialize)]
pub struct DictTypeQuery {
    #[serde(rename = "type")]
    r#type: String,
}

pub async fn create_dict_item_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateSysDictItemRequest>,
) -> Result<Json<ApiResponse<SysDictItem>>, AppError> {
    let item = state.sys_dict_item_service.create_dict_item(req).await?;
    Ok(Json(ApiResponse::ok(item)))
}

pub async fn get_dict_item_handler(
    State(state): State<AppState>,
    Path(params): Path<DictItemIdParam>,
) -> Result<Json<ApiResponse<SysDictItem>>, AppError> {
    let item = state
        .sys_dict_item_service
        .get_dict_item(&params.id)
        .await?;
    Ok(Json(ApiResponse::ok(item)))
}

pub async fn get_all_dict_items_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<SysDictItem>>>, AppError> {
    let items = state.sys_dict_item_service.get_all_dict_items().await?;
    Ok(Json(ApiResponse::ok(items)))
}

pub async fn get_dict_items_by_dict_id_handler(
    State(state): State<AppState>,
    Path(dict_id): Path<i64>,
) -> Result<Json<ApiResponse<Vec<SysDictItem>>>, AppError> {
    let items = state
        .sys_dict_item_service
        .get_dict_items_by_dict_id(&dict_id)
        .await?;
    Ok(Json(ApiResponse::ok(items)))
}

pub async fn get_dict_items_by_type_handler(
    State(state): State<AppState>,
    Query(query): Query<DictTypeQuery>,
) -> Result<Json<ApiResponse<Vec<SysDictItem>>>, AppError> {
    let items = state
        .sys_dict_item_service
        .get_dict_items_by_type(&query.r#type)
        .await?;
    Ok(Json(ApiResponse::ok(items)))
}

pub async fn get_dict_items_page_handler(
    State(state): State<AppState>,
    Query(query): Query<SysDictItemPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<SysDictItemVO>>>, AppError> {
    let page_response = state
        .sys_dict_item_service
        .get_dict_items_page(query)
        .await?;
    Ok(Json(ApiResponse::ok(page_response)))
}

pub async fn update_dict_item_handler(
    State(state): State<AppState>,
    Path(params): Path<DictItemIdParam>,
    Json(req): Json<UpdateSysDictItemRequest>,
) -> Result<Json<ApiResponse<SysDictItem>>, AppError> {
    let item = state
        .sys_dict_item_service
        .update_dict_item(&params.id, req)
        .await?;
    Ok(Json(ApiResponse::ok(item)))
}

pub async fn delete_dict_item_handler(
    State(state): State<AppState>,
    Path(params): Path<DictItemIdParam>,
) -> Result<(StatusCode, ()), AppError> {
    state
        .sys_dict_item_service
        .delete_dict_item(&params.id)
        .await?;
    Ok((StatusCode::NO_CONTENT, ()))
}

pub async fn get_safe_policy_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<HashMap<String, String>>>, AppError> {
    let policy = state.sys_dict_item_service.get_safe_policy().await?;
    Ok(Json(ApiResponse::ok(policy)))
}
