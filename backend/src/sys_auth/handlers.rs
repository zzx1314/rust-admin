use crate::api::AppState;
use crate::common::error::{ApiResponse, AppError};
use crate::sys_auth::domain::{SetMenuAuthRequest, SysAuthMenuVo};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct RoleCodePath {
    role_code: String,
}

pub async fn get_menu_data_handler(
    State(state): State<AppState>,
    Path(RoleCodePath { role_code }): Path<RoleCodePath>,
) -> Result<Json<ApiResponse<Vec<SysAuthMenuVo>>>, AppError> {
    let result = state.sys_auth_service.get_role_auth(&role_code).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn set_menu_auth_handler(
    State(state): State<AppState>,
    Json(req): Json<SetMenuAuthRequest>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.sys_auth_service.set_menu_auth(req).await?;
    Ok(Json(ApiResponse::ok(())))
}
