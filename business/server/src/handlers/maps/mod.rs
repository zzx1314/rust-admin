use crate::app::AppState;
use crate::auth::{self, Claims};
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::gateway;
use crate::ingest;
use crate::models::{
    BackendMap, BackendMapQuery, ImportBackendMapRequest, MapQuery, MapRaster, MapRefRoute,
    MapRefTask, MapRefs, MapVersion, MapVersionRequest,
};
use axum::Json;
use axum::body::Body;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{QueryBuilder, SqlitePool};
use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::str::FromStr;
mod backend;
mod import;
mod package;
mod push;
mod raster;
#[cfg(test)]
mod tests;

// 路由（`app.rs`）与其它模块用的入口
pub use backend::{list_backend_maps, list_synced_sources};
pub use import::import_backend_map;
pub use package::map_package;
pub use push::{PushMapRequest, apply_push_map};
pub use raster::map_raster;

// 子模块之间共享（crate 内）：
pub(crate) use backend::{
    backend_pool, dog_map_has_park_columns, dog_map_has_share_table, dog_map_select_columns,
    map_shared_to_user, read_ros_map_meta,
};
pub(crate) use package::resolve_package_path;

const MAP_SELECT: &str =
    "SELECT m.id, m.park_id, p.park_name, m.device_code, m.map_name, m.version,
    m.frame_id, m.crs, m.origin, m.resolution, m.floors, m.road_count, m.no_go_count,
    m.status, m.map_hash, m.map_width, m.map_height, m.note, m.published_at, m.created_at,
    m.source, m.source_id, m.file_name, m.file_path, m.file_size,
    m.owner_user_id, m.owner_username, m.source_ref,
    (SELECT COUNT(*) FROM routes r WHERE r.map_version_id = m.id) AS route_count,
    (SELECT COUNT(*) FROM tasks t JOIN routes r ON r.id = t.route_id
        WHERE r.map_version_id = m.id) AS task_count
    FROM map_versions m
    LEFT JOIN parks p ON p.id = m.park_id";

pub async fn list_maps(
    State(state): State<AppState>,
    Query(query): Query<MapQuery>,
    claims: Option<Extension<Claims>>,
) -> Result<Json<ApiResponse<Vec<MapVersion>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(MAP_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(park_id) = query.park_id {
        builder.push(" AND m.park_id = ").push_bind(park_id);
    }
    if let Some(device_code) = query
        .device_code
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        builder
            .push(" AND m.device_code = ")
            .push_bind(device_code.to_string());
    }
    if query.unbound.unwrap_or(false) {
        builder.push(" AND m.park_id IS NULL");
    }
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND m.status = ").push_bind(status);
    }
    if let Some(source) = query
        .source
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND m.source = ").push_bind(source);
    }
    // 地图跟着设备走：设备在中台分配给谁，谁就能看到这张图。
    // 一台狗可以绑定多个人：`owner_user_id` 只记回传时的第一个归属人，
    // 其他人得靠「设备分配关系」兜住，否则后绑定的人地图列表里一张都看不到。
    // 管理员可看全部，也可用 ownerUserId 指定某个用户；其他人只看自己（含归属未知的历史行）。
    if let Some(Extension(claims)) = claims {
        if claims.is_admin {
            if let Some(owner) = query.owner_user_id {
                builder.push(" AND m.owner_user_id = ").push_bind(owner);
            }
        } else if let Some(zentai_user_id) = claims.zentai_user_id {
            // 中台不可达 / 未配置时不放行全量：退回「只看自己」的老口径。
            let device_codes = auth::allowed_device_codes(&state, &claims)
                .await
                .ok()
                .flatten()
                .map(|codes| {
                    let mut codes: Vec<String> = codes.into_iter().collect();
                    codes.sort();
                    codes
                })
                .unwrap_or_default();
            builder
                .push(" AND (m.owner_user_id = ")
                .push_bind(zentai_user_id)
                .push(" OR m.owner_user_id IS NULL");
            if !device_codes.is_empty() {
                builder.push(" OR m.device_code IN (");
                for (index, code) in device_codes.iter().enumerate() {
                    if index > 0 {
                        builder.push(", ");
                    }
                    builder.push_bind(code.clone());
                }
                builder.push(")");
            }
            builder.push(")");
        } else {
            builder.push(" AND m.owner_user_id IS NULL");
        }
    }
    builder.push(" ORDER BY m.park_id ASC, m.published_at DESC, m.created_at DESC");
    let maps = builder
        .build_query_as::<MapVersion>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(maps))
}

pub async fn list_park_maps(
    State(state): State<AppState>,
    Path(park_id): Path<i64>,
) -> Result<Json<ApiResponse<Vec<MapVersion>>>, AppError> {
    let sql = format!(
        "{MAP_SELECT} WHERE m.park_id = ?1 ORDER BY m.published_at DESC, m.created_at DESC"
    );
    let maps = sqlx::query_as::<_, MapVersion>(&sql)
        .bind(park_id)
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(maps))
}
pub async fn get_map(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MapVersion>>, AppError> {
    Ok(ApiResponse::ok(fetch_map(&state, &id).await?))
}

pub async fn map_refs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MapRefs>>, AppError> {
    let map = fetch_map(&state, &id).await?;
    let routes = sqlx::query_as::<_, MapRefRoute>(
        "SELECT id, route_code, route_name, status, version, point_count
         FROM routes WHERE map_version_id = ?1 ORDER BY id ASC",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    let tasks = sqlx::query_as::<_, MapRefTask>(
        "SELECT t.id, t.task_code, t.task_name, t.task_status, t.route_id
         FROM tasks t JOIN routes r ON r.id = t.route_id
         WHERE r.map_version_id = ?1 ORDER BY t.id ASC",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    let device_count: i64 = match map.park_id {
        Some(park_id) => {
            sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE park_id = ?1")
                .bind(park_id)
                .fetch_one(&state.db)
                .await?
        }
        // 还没绑定园区的设备地图：只算采集这张图的那台设备
        None => match map.device_code.as_deref() {
            Some(device_code) => {
                sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE device_id = ?1")
                    .bind(device_code)
                    .fetch_one(&state.db)
                    .await?
            }
            None => 0,
        },
    };
    Ok(ApiResponse::ok(MapRefs {
        map,
        routes,
        tasks,
        device_count,
    }))
}

pub async fn publish_map(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MapVersion>>, AppError> {
    let map = fetch_map(&state, &id).await?;
    if map.status == "已归档" {
        return Err(AppError::BadRequest(
            "已归档的地图基线不能重新发布".to_string(),
        ));
    }
    let timestamp = now();
    let mut transaction = state.db.begin().await?;
    sqlx::query(
        "UPDATE map_versions SET status = '已归档'
         WHERE park_id = ?1 AND id <> ?2 AND status = '已发布'",
    )
    .bind(map.park_id)
    .bind(&id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("UPDATE map_versions SET status = '已发布', published_at = ?1 WHERE id = ?2")
        .bind(&timestamp)
        .bind(&id)
        .execute(&mut *transaction)
        .await?;
    if let Some(park_id) = map.park_id {
        let park_status = if map.status == "部分标定" {
            "部分标定"
        } else {
            "已标定"
        };
        sqlx::query(
            "UPDATE parks SET map_version_id = ?1, map_status = ?2, updated_at = ?3 WHERE id = ?4",
        )
        .bind(&id)
        .bind(park_status)
        .bind(&timestamp)
        .bind(park_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(ApiResponse::ok(fetch_map(&state, &id).await?))
}

pub async fn new_map_version(
    State(state): State<AppState>,
    Path(id): Path<String>,
    payload: Option<Json<MapVersionRequest>>,
) -> Result<Json<ApiResponse<MapVersion>>, AppError> {
    let map = fetch_map(&state, &id).await?;
    let payload = payload.map(|Json(value)| value);
    let timestamp = now();
    let version = bump_version(&map.version);
    let new_id = format!("{}@{}", map.map_name, version);
    let existing: Option<(String,)> = sqlx::query_as("SELECT id FROM map_versions WHERE id = ?1")
        .bind(&new_id)
        .fetch_optional(&state.db)
        .await?;
    if existing.is_some() {
        return Err(AppError::BadRequest(format!(
            "地图版本 {new_id} 已存在，请先确认版本号"
        )));
    }
    let map_hash = payload
        .as_ref()
        .and_then(|value| value.map_hash.clone())
        .unwrap_or_else(|| demo_hash(&new_id, &timestamp));
    let note = payload
        .as_ref()
        .and_then(|value| value.note.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("由 {} 派生，重新建图标定", map.id));

    let mut transaction = state.db.begin().await?;
    sqlx::query(
        "INSERT INTO map_versions (id, park_id, device_code, map_name, version, frame_id, crs,
            origin, resolution, floors, road_count, no_go_count, status, map_hash, map_width,
            map_height, note, published_at, created_at, owner_user_id, owner_username, source_ref)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, '已发布', ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
    )
    .bind(&new_id)
    .bind(map.park_id)
    .bind(&map.device_code)
    .bind(&map.map_name)
    .bind(&version)
    .bind(&map.frame_id)
    .bind(&map.crs)
    .bind(&map.origin)
    .bind(&map.resolution)
    .bind(&map.floors)
    .bind(map.road_count)
    .bind(map.no_go_count)
    .bind(&map_hash)
    .bind(map.map_width)
    .bind(map.map_height)
    .bind(&note)
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(map.owner_user_id)
    .bind(&map.owner_username)
    .bind(&map.source_ref)
    .execute(&mut *transaction)
    .await?;
    if let Some(park_id) = map.park_id {
        sqlx::query(
            "UPDATE map_versions SET status = '已归档'
             WHERE park_id = ?1 AND id <> ?2 AND status = '已发布'",
        )
        .bind(park_id)
        .bind(&new_id)
        .execute(&mut *transaction)
        .await?;
        let park_status = if map.status == "部分标定" {
            "部分标定"
        } else {
            "已标定"
        };
        sqlx::query(
            "UPDATE parks SET map_version_id = ?1, map_status = ?2, updated_at = ?3 WHERE id = ?4",
        )
        .bind(&new_id)
        .bind(park_status)
        .bind(&timestamp)
        .bind(park_id)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;

    Ok(ApiResponse::ok(fetch_map(&state, &new_id).await?))
}
pub(crate) async fn existing_row_exists(state: &AppState, id: &str) -> Result<bool, AppError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM map_versions WHERE id = ?1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    Ok(count > 0)
}

pub(crate) async fn fetch_map(state: &AppState, id: &str) -> Result<MapVersion, AppError> {
    let sql = format!("{MAP_SELECT} WHERE m.id = ?1");
    sqlx::query_as::<_, MapVersion>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("地图基线 {id} 不存在")))
}

pub(crate) fn bump_version(version: &str) -> String {
    let digits: Vec<i64> = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<i64>().ok())
        .collect();
    let major = digits.first().copied().unwrap_or(0);
    let minor = digits.get(1).copied().unwrap_or(0) + 1;
    format!("v{major}.{minor}")
}

pub(crate) fn demo_hash(id: &str, timestamp: &str) -> String {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    timestamp.hash(&mut hasher);
    format!(
        "{:016x}{:016x}",
        hasher.finish(),
        hasher.finish().rotate_left(17)
    )
}
