use crate::api::AppState;
use crate::business::app_review::domain::{
    CreateReviewRequest, ReviewActionRequest, ReviewPageQuery, ReviewVO,
};
use crate::business::harbor::models::HarborWebhookPayload;
use crate::common::error::{ApiResponse, AppError};
use crate::common::pagination::PageResponse;
use axum::http::HeaderMap;
use axum::{
    Json,
    extract::{Path, Query, State},
};
use axum_extra::TypedHeader;
use axum_extra::headers::Authorization;
use axum_extra::headers::authorization::Bearer;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct ReviewIdParam {
    pub id: i64,
}

pub async fn create_review_handler(
    State(state): State<AppState>,
    auth: TypedHeader<Authorization<Bearer>>,
    Json(req): Json<CreateReviewRequest>,
) -> Result<Json<ApiResponse<ReviewVO>>, AppError> {
    let user_id = state.auth_service.validate_token(auth.token()).await?;
    let review = state
        .app_review_service
        .create_review(req, Some(user_id))
        .await?;
    Ok(Json(ApiResponse::ok(ReviewVO::from(review))))
}

pub async fn list_reviews_handler(
    State(state): State<AppState>,
    auth: TypedHeader<Authorization<Bearer>>,
    Query(mut query): Query<ReviewPageQuery>,
) -> Result<Json<ApiResponse<PageResponse<ReviewVO>>>, AppError> {
    let user_id = state.auth_service.validate_token(auth.token()).await?;
    let roles = state.role_service.get_roles_for_user(&user_id).await?;
    let is_admin = roles.iter().any(|role| {
        role.code
            .as_deref()
            .map(|code| {
                let code = code.trim().to_ascii_lowercase();
                code == "admin"
                    || code == "administrator"
                    || code == "role_admin"
                    || code == "sysadm"
                    || code == "110"
            })
            .unwrap_or(false)
            || role.name.trim() == "管理员"
            || role.name.trim().eq_ignore_ascii_case("administrator")
    });
    tracing::info!(
        user_id,
        is_admin,
        role_count = roles.len(),
        roles = ?roles.iter().map(|role| (&role.name, &role.code)).collect::<Vec<_>>(),
        "Loading application reviews"
    );
    if !is_admin {
        query.created_by = Some(user_id);
    }
    let result = state.app_review_service.list_reviews(query).await?;
    Ok(Json(ApiResponse::ok(result)))
}

pub async fn get_review_handler(
    State(state): State<AppState>,
    auth: TypedHeader<Authorization<Bearer>>,
    Path(params): Path<ReviewIdParam>,
) -> Result<Json<ApiResponse<ReviewVO>>, AppError> {
    let user_id = state.auth_service.validate_token(auth.token()).await?;
    let review = state.app_review_service.get_review(params.id).await?;
    let roles = state.role_service.get_roles_for_user(&user_id).await?;
    let is_admin = roles.iter().any(|role| {
        role.code
            .as_deref()
            .map(|code| {
                let code = code.trim().to_ascii_lowercase();
                code == "admin"
                    || code == "administrator"
                    || code == "role_admin"
                    || code == "sysadm"
                    || code == "110"
            })
            .unwrap_or(false)
            || role.name.trim() == "管理员"
            || role.name.trim().eq_ignore_ascii_case("administrator")
    });
    if !is_admin && review.created_by != Some(user_id) {
        return Err(AppError::NotFound(format!(
            "Review with id {} not found",
            params.id
        )));
    }
    Ok(Json(ApiResponse::ok(ReviewVO::from(review))))
}

pub async fn approve_review_handler(
    State(state): State<AppState>,
    Path(params): Path<ReviewIdParam>,
    auth: TypedHeader<Authorization<Bearer>>,
    Json(req): Json<ReviewActionRequest>,
) -> Result<Json<ApiResponse<ReviewVO>>, AppError> {
    let user_id = state.auth_service.validate_token(auth.token()).await?;
    let review = state
        .app_review_service
        .approve_review(params.id, user_id, req.reviewer_comment)
        .await?;
    Ok(Json(ApiResponse::ok(ReviewVO::from(review))))
}

pub async fn reject_review_handler(
    State(state): State<AppState>,
    Path(params): Path<ReviewIdParam>,
    auth: TypedHeader<Authorization<Bearer>>,
    Json(req): Json<ReviewActionRequest>,
) -> Result<Json<ApiResponse<ReviewVO>>, AppError> {
    let user_id = state.auth_service.validate_token(auth.token()).await?;
    let review = state
        .app_review_service
        .reject_review(params.id, user_id, req.reviewer_comment)
        .await?;
    Ok(Json(ApiResponse::ok(ReviewVO::from(review))))
}

pub async fn delete_review_handler(
    State(state): State<AppState>,
    Path(params): Path<ReviewIdParam>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    state.app_review_service.delete_review(params.id).await?;
    Ok(Json(ApiResponse::ok(())))
}

pub async fn harbor_webhook_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<HarborWebhookPayload>,
) -> Result<Json<ApiResponse<Vec<ReviewVO>>>, AppError> {
    if let Some(secret) = state
        .harbor_config
        .as_ref()
        .and_then(|c| c.webhook_secret.as_ref())
    {
        let provided = headers
            .get("x-webhook-secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if provided != secret {
            return Err(AppError::Unauthorized("Invalid webhook secret".to_string()));
        }
    }

    if payload.event_type != "PUSH_ARTIFACT" {
        return Ok(Json(ApiResponse::ok(vec![])));
    }

    let harbor_config = state
        .harbor_config
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("Harbor is not configured".to_string()))?;

    let repo = &payload.event_data.repository;
    if repo.namespace != harbor_config.staging_project {
        tracing::info!(
            "Ignoring webhook for project {} (only {} is monitored)",
            repo.namespace,
            harbor_config.staging_project
        );
        return Ok(Json(ApiResponse::ok(vec![])));
    }

    let dest_project = harbor_config.production_project.clone();
    let mut created = Vec::new();

    for resource in payload.event_data.resources {
        let tag = match resource.tag {
            Some(tag) if !tag.is_empty() => tag,
            _ => continue,
        };

        let repository_name = repo
            .repo_full_name
            .clone()
            .unwrap_or_else(|| format!("{}/{}", repo.namespace, repo.name));

        let req = CreateReviewRequest {
            src_project: repo.namespace.clone(),
            dest_project: dest_project.clone(),
            repository_name,
            tag,
            digest: resource.digest,
            artifact_id: None,
            reviewer_comment: None,
        };

        tracing::info!(
            project = %req.src_project,
            repository = %req.repository_name,
            tag = %req.tag,
            digest = ?req.digest,
            "Creating application review from Harbor webhook"
        );

        match state
            .app_review_service
            .create_review_with_dedup(req, None)
            .await
        {
            Ok(Some(review)) => created.push(ReviewVO::from(review)),
            Ok(None) => {
                tracing::info!(
                    "Approved review with identical digest already exists, skipping deduplicated artifact"
                );
            }
            Err(AppError::Conflict(_)) => {
                tracing::info!("Pending review already exists for artifact, skipping");
            }
            Err(e) => return Err(e),
        }
    }

    Ok(Json(ApiResponse::ok(created)))
}
