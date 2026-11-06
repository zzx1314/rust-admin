use crate::api::AppState;
use crate::business::dog::route_library::domain::{
    CreateRouteRequest, RoutePageQuery, RouteVO, UpdateRouteRequest,
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

pub async fn create_route_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateRouteRequest>,
) -> Result<Json<ApiResponse<RouteVO>>, AppError> {
    let route = state.dog_route_library_service.create(req).await?;
    let vo = state.dog_route_library_service.to_vo(route).await?;
    Ok(Json(ApiResponse::ok(vo)))
}

pub async fn get_route_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<RouteVO>>, AppError> {
    let route = state.dog_route_library_service.get_by_id(params.id).await?;
    let vo = state.dog_route_library_service.to_vo(route).await?;
    Ok(Json(ApiResponse::ok(vo)))
}

pub async fn list_routes_handler(
    State(state): State<AppState>,
    Query(query): Query<RoutePageQuery>,
) -> Result<Json<ApiResponse<PageResponse<RouteVO>>>, AppError> {
    let result = state.dog_route_library_service.list_page(query).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn list_all_routes_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<RouteVO>>>, AppError> {
    let routes = state.dog_route_library_service.list_all().await?;
    Ok(Json(ApiResponse::ok(
        routes.into_iter().map(RouteVO::from).collect(),
    )))
}

pub async fn update_route_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
    Json(req): Json<UpdateRouteRequest>,
) -> Result<Json<ApiResponse<RouteVO>>, AppError> {
    let route = state
        .dog_route_library_service
        .update(params.id, req)
        .await?;
    let vo = state.dog_route_library_service.to_vo(route).await?;
    Ok(Json(ApiResponse::ok(vo)))
}

pub async fn delete_route_handler(
    State(state): State<AppState>,
    Path(params): Path<IdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.dog_route_library_service.delete(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}
