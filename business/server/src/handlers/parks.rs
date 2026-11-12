use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{Park, ParkQuery, ParkRequest};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::Extension;
use sqlx::QueryBuilder;

const PARK_SELECT: &str =
    "SELECT p.id, p.park_code, p.park_name, p.park_description, p.park_address,
    p.area, p.owner_name, p.park_type, p.status, p.color, p.created_at, p.updated_at,
    p.map_version_id, p.map_status, p.created_by,
    (SELECT COUNT(*) FROM zones z WHERE z.park_id = p.id) AS zone_count,
    (SELECT COUNT(*) FROM devices d WHERE d.park_id = p.id) AS device_count,
    (SELECT COUNT(*) FROM tasks t WHERE t.park_id = p.id) AS task_count,
    (SELECT COUNT(*) FROM tasks t WHERE t.park_id = p.id AND t.task_status = '已完成') AS done_count
    FROM parks p";

pub async fn list_parks(
    State(state): State<AppState>,
    Query(query): Query<ParkQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<Park>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(PARK_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(q) = query.q.filter(|value| !value.trim().is_empty()) {
        let keyword = format!("%{}%", q.trim());
        builder
            .push(" AND (p.park_name LIKE ")
            .push_bind(keyword.clone())
            .push(" OR p.park_code LIKE ")
            .push_bind(keyword.clone())
            .push(" OR p.owner_name LIKE ")
            .push_bind(keyword)
            .push(")");
    }
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND p.status = ").push_bind(status);
    }
    // 非管理员只能看到自己创建的园区
    if !claims.is_admin {
        builder.push(" AND p.created_by = ").push_bind(claims.sub);
    }
    builder.push(" ORDER BY p.id ASC");

    let parks = builder
        .build_query_as::<Park>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(parks))
}

pub async fn get_park(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Park>>, AppError> {
    let park = fetch_park(&state, id).await?;
    ensure_owner(&park, &claims)?;
    Ok(ApiResponse::ok(park))
}

pub async fn create_park(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ParkRequest>,
) -> Result<Json<ApiResponse<Park>>, AppError> {
    if payload.park_name.trim().is_empty() {
        return Err(AppError::BadRequest("园区名称不能为空".to_string()));
    }
    let next_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM parks")
        .fetch_one(&state.db)
        .await?;
    let park_code = payload
        .park_code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("PK-{next_id:03}"));
    let timestamp = now();

    sqlx::query(
        "INSERT INTO parks (park_code, park_name, park_description, park_address, area, owner_name,
            park_type, status, color, created_at, updated_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )
    .bind(&park_code)
    .bind(payload.park_name.trim())
    .bind(payload.park_description)
    .bind(payload.park_address)
    .bind(payload.area.unwrap_or(0.0))
    .bind(payload.owner_name)
    .bind(payload.park_type)
    .bind(payload.status.unwrap_or_else(|| "运行中".to_string()))
    .bind(payload.color.unwrap_or_else(|| "#0284c7".to_string()))
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(claims.sub)
    .execute(&state.db)
    .await?;

    let id: i64 = sqlx::query_scalar("SELECT id FROM parks WHERE park_code = ?1")
        .bind(&park_code)
        .fetch_one(&state.db)
        .await?;
    Ok(ApiResponse::ok(fetch_park(&state, id).await?))
}

pub async fn update_park(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ParkRequest>,
) -> Result<Json<ApiResponse<Park>>, AppError> {
    let existing = fetch_park(&state, id).await?;
    if !claims.is_admin && existing.created_by.map(|uid| uid != claims.sub).unwrap_or(true) {
        return Err(AppError::NotFound("园区不存在".to_string()));
    }
    let (map_version_id, map_status) = resolve_park_map(&state, id, payload.map_version_id)
        .await?
        .unwrap_or((existing.map_version_id, existing.map_status));
    sqlx::query(
        "UPDATE parks SET park_name = ?1, park_description = ?2, park_address = ?3, area = ?4,
            owner_name = ?5, park_type = ?6, status = ?7, color = ?8, map_version_id = ?9,
            map_status = ?10, updated_at = ?11
         WHERE id = ?12",
    )
    .bind(if payload.park_name.trim().is_empty() {
        existing.park_name
    } else {
        payload.park_name.trim().to_string()
    })
    .bind(payload.park_description.or(existing.park_description))
    .bind(payload.park_address.or(existing.park_address))
    .bind(payload.area.unwrap_or(existing.area))
    .bind(payload.owner_name.or(existing.owner_name))
    .bind(payload.park_type.or(existing.park_type))
    .bind(payload.status.unwrap_or(existing.status))
    .bind(payload.color.unwrap_or(existing.color))
    .bind(map_version_id)
    .bind(map_status)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_park(&state, id).await?))
}

pub async fn delete_park(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let park = fetch_park(&state, id).await?;
    if !claims.is_admin && park.created_by.map(|uid| uid != claims.sub).unwrap_or(true) {
        return Err(AppError::NotFound("园区不存在".to_string()));
    }
    let mut transaction = state.db.begin().await?;
    sqlx::query("DELETE FROM zones WHERE park_id = ?1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE devices SET park_id = NULL, zone_id = NULL,
            status = CASE WHEN status = '离线' THEN '离线' ELSE '待绑定' END
         WHERE park_id = ?1",
    )
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE tasks SET park_id = NULL, zone_id = NULL WHERE park_id = ?1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    let result = sqlx::query("DELETE FROM parks WHERE id = ?1")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

/// 非管理员只能操作自己创建的资源。返回 `NotFound` 而非 `Forbidden`，
/// 避免暴露「该记录由他人创建」的信息。
pub(crate) fn ensure_owner<T: OwnerCheck>(resource: &T, claims: &Claims) -> Result<(), AppError> {
    if claims.is_admin {
        return Ok(());
    }
    match resource.creator_id() {
        Some(uid) if uid == claims.sub => Ok(()),
        _ => Err(AppError::NotFound("记录不存在".to_string())),
    }
}

/// 让多种资源类型可以共用 `ensure_owner`
pub(crate) trait OwnerCheck {
    fn creator_id(&self) -> Option<i64>;
}

impl OwnerCheck for Park {
    fn creator_id(&self) -> Option<i64> {
        self.created_by
    }
}

async fn fetch_park(state: &AppState, id: i64) -> Result<Park, AppError> {
    let sql = format!("{PARK_SELECT} WHERE p.id = ?1");
    sqlx::query_as::<_, Park>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("园区不存在".to_string()))
}

/// 解析园区表单提交的地图基线绑定。
/// 返回 `None` 表示请求未携带该字段，保留原值；`Some((None, status))` 表示解绑。
pub(crate) async fn resolve_park_map(
    state: &AppState,
    park_id: i64,
    requested: Option<String>,
) -> Result<Option<(Option<String>, String)>, AppError> {
    let Some(requested) = requested else {
        return Ok(None);
    };
    let requested = requested.trim();
    if requested.is_empty() {
        return Ok(Some((None, "待标定".to_string())));
    }
    if requested == "__none__" {
        return Ok(Some((None, "未绑定".to_string())));
    }
    let map = sqlx::query_as::<_, (i64, String)>(
        "SELECT park_id, status FROM map_versions WHERE id = ?1",
    )
    .bind(requested)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::BadRequest(format!("地图基线 {requested} 不存在")))?;
    if map.0 != park_id {
        return Err(AppError::BadRequest(
            "地图基线不属于该园区，无法绑定".to_string(),
        ));
    }
    let status = if map.1 == "部分标定" {
        "部分标定".to_string()
    } else {
        "已标定".to_string()
    };
    Ok(Some((Some(requested.to_string()), status)))
}
