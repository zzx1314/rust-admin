use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{Zone, ZoneQuery, ZoneRequest};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::Extension;
use sqlx::QueryBuilder;

const ZONE_SELECT: &str = "SELECT z.id, z.park_id, p.park_name, z.zone_name, z.zone_type,
    z.frequency, z.status, z.created_at, z.updated_at, z.created_by,
    (SELECT COUNT(*) FROM devices d WHERE d.zone_id = z.id) AS device_count
    FROM zones z LEFT JOIN parks p ON p.id = z.park_id";

pub async fn list_zones(
    State(state): State<AppState>,
    Query(query): Query<ZoneQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<Zone>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(ZONE_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(park_id) = query.park_id {
        builder.push(" AND z.park_id = ").push_bind(park_id);
    }
    if !claims.is_admin {
        builder.push(" AND z.created_by = ").push_bind(claims.sub);
    }
    builder.push(" ORDER BY z.park_id ASC, z.id ASC");
    let zones = builder
        .build_query_as::<Zone>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(zones))
}

pub async fn get_zone(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Zone>>, AppError> {
    let zone = fetch_zone(&state, id).await?;
    super::parks::ensure_owner(&zone, &claims)?;
    Ok(ApiResponse::ok(zone))
}

pub async fn create_zone(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ZoneRequest>,
) -> Result<Json<ApiResponse<Zone>>, AppError> {
    if payload.zone_name.trim().is_empty() {
        return Err(AppError::BadRequest("区域名称不能为空".to_string()));
    }
    let timestamp = now();
    let result = sqlx::query(
        "INSERT INTO zones (park_id, zone_name, zone_type, frequency, status, created_at, updated_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(payload.park_id)
    .bind(payload.zone_name.trim())
    .bind(payload.zone_type)
    .bind(payload.frequency)
    .bind(payload.status.unwrap_or_else(|| "正常".to_string()))
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(claims.sub)
    .execute(&state.db)
    .await?;
    let zone = fetch_zone(&state, result.last_insert_rowid()).await?;
    Ok(ApiResponse::ok(zone))
}

pub async fn update_zone(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ZoneRequest>,
) -> Result<Json<ApiResponse<Zone>>, AppError> {
    let existing = fetch_zone(&state, id).await?;
    super::parks::ensure_owner(&existing, &claims)?;
    sqlx::query(
        "UPDATE zones SET park_id = ?1, zone_name = ?2, zone_type = ?3, frequency = ?4,
            status = ?5, updated_at = ?6 WHERE id = ?7",
    )
    .bind(payload.park_id)
    .bind(if payload.zone_name.trim().is_empty() {
        existing.zone_name
    } else {
        payload.zone_name.trim().to_string()
    })
    .bind(payload.zone_type.or(existing.zone_type))
    .bind(payload.frequency.or(existing.frequency))
    .bind(payload.status.unwrap_or(existing.status))
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_zone(&state, id).await?))
}

pub async fn delete_zone(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let zone = fetch_zone(&state, id).await?;
    super::parks::ensure_owner(&zone, &claims)?;
    sqlx::query("UPDATE devices SET zone_id = NULL WHERE zone_id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    let result = sqlx::query("DELETE FROM zones WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

impl super::parks::OwnerCheck for Zone {
    fn creator_id(&self) -> Option<i64> {
        self.created_by
    }
}

async fn fetch_zone(state: &AppState, id: i64) -> Result<Zone, AppError> {
    let sql = format!("{ZONE_SELECT} WHERE z.id = ?1");
    sqlx::query_as::<_, Zone>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("巡检区域不存在".to_string()))
}
