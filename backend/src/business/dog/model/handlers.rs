use crate::api::AppState;
use crate::business::dog::model::domain::{
    CreateModelRequest, ModelPageQuery, ModelVO, UpdateModelRequest,
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

pub async fn create_model_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateModelRequest>,
) -> Result<Json<ApiResponse<ModelVO>>, AppError> {
    let model = state.dog_model_service.create(req).await?;
    Ok(Json(ApiResponse::ok(ModelVO::from(model))))
}

pub async fn get_model_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<ModelVO>>, AppError> {
    let model = state.dog_model_service.get_by_id(params.id).await?;
    Ok(Json(ApiResponse::ok(ModelVO::from(model))))
}

pub async fn list_models_handler(
    State(state): State<AppState>,
    Query(query): Query<ModelPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<ModelVO>>>, AppError> {
    let result = state.dog_model_service.list_page(query).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_models_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<ModelVO>>>, AppError> {
    let models = state.dog_model_service.list_all().await?;
    Ok(Json(ApiResponse::ok(
        models.into_iter().map(ModelVO::from).collect(),
    )))
}

pub async fn update_model_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateModelRequest>,
) -> Result<Json<ApiResponse<ModelVO>>, AppError> {
    let model = state.dog_model_service.update(params.id, req).await?;
    Ok(Json(ApiResponse::ok(ModelVO::from(model))))
}

pub async fn delete_model_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_model_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}
