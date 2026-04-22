use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::system::sys_menu::domain::{CreateMenuRequest, Menu, MenuTree, MenuVo, UpdateMenuRequest};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use axum_extra::TypedHeader;
use axum_extra::headers::Authorization;
use axum_extra::headers::authorization::Bearer;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentIdQuery {
    parent_id: Option<i64>,
}

pub async fn create_menu_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateMenuRequest>,
) -> Result<Json<ApiResponse<Menu>>, AppError> {
    let menu = state.menu_service.create_menu(req).await?;
    Ok(Json(ApiResponse::ok(menu)))
}

pub async fn get_menu_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Menu>>, AppError> {
    let menu = state.menu_service.get_menu(&id).await?;
    Ok(Json(ApiResponse::ok(menu)))
}

pub async fn get_all_menus_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<MenuVo>>>, AppError> {
    let menus = state.menu_service.get_all_menus().await?;
    let vos: Vec<MenuVo> = menus.into_iter().map(MenuVo::from).collect();
    Ok(Json(ApiResponse::ok(vos)))
}

pub async fn get_menu_tree_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<Menu>>>, AppError> {
    let menus = state.menu_service.get_menu_tree().await?;
    Ok(Json(ApiResponse::ok(menus)))
}

pub async fn get_menus_by_parent_handler(
    State(state): State<AppState>,
    Query(query): Query<ParentIdQuery>,
) -> Result<Json<ApiResponse<Vec<Menu>>>, AppError> {
    let menus = state
        .menu_service
        .get_menus_by_parent(query.parent_id)
        .await?;
    Ok(Json(ApiResponse::ok(menus)))
}

pub async fn update_menu_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateMenuRequest>,
) -> Result<Json<ApiResponse<Menu>>, AppError> {
    let menu = state.menu_service.update_menu(&id, req).await?;
    Ok(Json(ApiResponse::ok(menu)))
}

pub async fn delete_menu_handler(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(StatusCode, ()), AppError> {
    state.menu_service.delete_menu(&id).await?;
    Ok((StatusCode::NO_CONTENT, ()))
}

pub async fn get_user_menu_handler(
    State(state): State<AppState>,
    auth: TypedHeader<Authorization<Bearer>>,
) -> Result<Json<ApiResponse<Vec<MenuTree>>>, AppError> {
    let token = auth.token();
    let user_id = state.auth_service.validate_token(token).await?;
    let tree = state
        .menu_service
        .get_user_menu(&user_id.to_string())
        .await?;
    Ok(Json(ApiResponse::ok(tree)))
}
