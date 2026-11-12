use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{
    BackendMap, BackendMapQuery, ImportBackendMapRequest, MapQuery, MapRaster, MapRefRoute,
    MapRefTask, MapRefs, MapVersion, MapVersionRequest,
};
use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{QueryBuilder, SqlitePool};
use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::str::FromStr;

const MAP_SELECT: &str = "SELECT m.id, m.park_id, p.park_name, m.map_name, m.version,
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
) -> Result<Json<ApiResponse<Vec<MapVersion>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(MAP_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(park_id) = query.park_id {
        builder.push(" AND m.park_id = ").push_bind(park_id);
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

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedSourcesQuery {
    pub park_id: Option<i64>,
}

/// `GET /api/map-versions/synced-sources?parkId=...`
///
/// 返回该园区已经入库的机器人地图路径（`source_ref`）。设备客户端手动同步前
/// 先拉这个列表，跳过已同步地图，避免重复下载大文件。
///
/// 「已同步」还要求中台那张地图仍然存在：中台是地图主库，某张图在中台被删掉后，
/// 业务库里的历史行不能算已同步，否则设备端永远跳过、中台列表里也永远看不到。
/// 中台不可达时退回旧行为（按业务库记录判定），不把网络抖动放大成整机重传。
pub async fn list_synced_sources(
    State(state): State<AppState>,
    Query(query): Query<SyncedSourcesQuery>,
) -> Result<Json<ApiResponse<Vec<String>>>, AppError> {
    let park_id = query
        .park_id
        .ok_or_else(|| AppError::BadRequest("缺少 parkId".to_string()))?;
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT source_ref, source_id FROM map_versions
         WHERE park_id = ?1 AND source_ref IS NOT NULL AND source_ref <> ''
         ORDER BY created_at ASC",
    )
    .bind(park_id)
    .fetch_all(&state.db)
    .await?;

    let zentai_ids: Vec<i64> = rows
        .iter()
        .filter_map(|(_, source_id)| zentai_id(source_id.as_deref()))
        .collect();
    let alive = match crate::zentai::alive_map_ids(&state.config, &zentai_ids).await {
        Ok(alive) => Some(alive),
        Err(error) => {
            tracing::warn!("校验中台地图是否仍存在失败，按业务库记录判定已同步（{error}）");
            None
        }
    };

    let sources = rows
        .into_iter()
        .filter(|(_, source_id)| match &alive {
            // 中台不可用：保持旧行为，避免整机重传
            None => true,
            Some(alive) => match zentai_id(source_id.as_deref()) {
                // 没有中台 id 的历史行无法校验，按已同步处理
                None => true,
                Some(id) => alive.contains(&id),
            },
        })
        .map(|(source_ref, _)| source_ref)
        .collect();
    Ok(ApiResponse::ok(sources))
}

/// 业务库 `map_versions.source_id` → 中台 `dog_map.id`。
fn zentai_id(source_id: Option<&str>) -> Option<i64> {
    source_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<i64>().ok())
}

/// 中台 `dog_map` 的列清单
///
/// `park_code` / `park_name` 由根 backend 迁移 26 添加。若根库还是旧结构（迁移未跑），
/// 退化成 NULL，避免整个地图列表接口因缺少一列而 500。
fn dog_map_select_columns(has_park: bool) -> &'static str {
    if has_park {
        "d.park_code, d.park_name, d.user_id, u.username AS owner_username"
    } else {
        "NULL AS park_code, NULL AS park_name, d.user_id, u.username AS owner_username"
    }
}

/// 判断根库 `dog_map` 是否已经具备园区列
async fn dog_map_has_park_columns(pool: &SqlitePool) -> Result<bool, AppError> {
    let columns: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info('dog_map')")
        .fetch_all(pool)
        .await?;
    Ok(columns.iter().any(|(name,)| name == "park_code"))
}

/// 列出根目录 `backend` 的 `dog_map` 地图库（ROS map_server 包等）。
///
/// 可选 `parkCode`：只返回该业务园区下的地图（设备端建图回传时写入）。
pub async fn list_backend_maps(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<BackendMapQuery>,
) -> Result<Json<ApiResponse<Vec<BackendMap>>>, AppError> {
    let pool = match backend_pool(&state).await {
        Ok(pool) => pool,
        Err(_) => return Ok(ApiResponse::ok(Vec::new())),
    };
    let has_park = dog_map_has_park_columns(&pool).await?;
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(format!(
        "SELECT d.id, d.map_name, {}, d.file_name, d.file_path, d.file_format,
            d.file_size, d.remarks, d.map_version, d.frame_id, d.crs, d.map_origin, d.resolution,
            d.floors, d.road_count, d.nogo_count, d.map_status, d.map_hash, d.publish_time,
            d.map_width, d.map_height, d.create_time
         FROM dog_map d
         LEFT JOIN p_sys_user u ON u.id = d.user_id
         WHERE d.is_deleted = 0",
        dog_map_select_columns(has_park)
    ));
    if let Some(park_code) = query.park_code.filter(|value| !value.trim().is_empty()) {
        if !has_park {
            // 旧结构无法按园区过滤，直接返回空集合而不是把所有地图混进来
            return Ok(ApiResponse::ok(Vec::new()));
        }
        builder
            .push(" AND d.park_code = ")
            .push_bind(park_code.trim().to_string());
    }
    // 业务用户只能看到自己名下机器狗采集的地图；中台管理员可看全部，
    // 也可用 ownerUserId 指定查看某个用户的地图。
    let owner_filter = if claims.is_admin {
        query.owner_user_id
    } else {
        claims.zentai_user_id
    };
    if let Some(owner) = owner_filter {
        builder.push(" AND d.user_id = ").push_bind(owner);
    } else if claims.zentai_user_id.is_some() && !claims.is_admin {
        return Ok(ApiResponse::ok(Vec::new()));
    }
    builder.push(" ORDER BY d.id DESC");
    let maps = builder
        .build_query_as::<BackendMap>()
        .fetch_all(&pool)
        .await?;
    Ok(ApiResponse::ok(maps))
}

/// 把根 backend 的地图导入当前业务库，并可选地绑定为园区当前基线。
///
/// `parkId` 缺省时按中台地图携带的 `parkCode` 反查业务园区。
pub async fn import_backend_map(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ImportBackendMapRequest>,
) -> Result<Json<ApiResponse<MapVersion>>, AppError> {
    let backend_pool = backend_pool(&state).await?;
    let has_park = dog_map_has_park_columns(&backend_pool).await?;
    let source_sql = format!(
        "SELECT d.id, d.map_name, {}, d.file_name, d.file_path, d.file_format,
            d.file_size, d.remarks, d.map_version, d.frame_id, d.crs, d.map_origin, d.resolution,
            d.floors, d.road_count, d.nogo_count, d.map_status, d.map_hash, d.publish_time,
            d.map_width, d.map_height, d.create_time
         FROM dog_map d
         LEFT JOIN p_sys_user u ON u.id = d.user_id
         WHERE d.id = ?1 AND d.is_deleted = 0",
        dog_map_select_columns(has_park)
    );
    let source = sqlx::query_as::<_, BackendMap>(&source_sql)
        .bind(payload.backend_map_id)
        .fetch_optional(&backend_pool)
        .await?
        .ok_or_else(|| AppError::NotFound("backend 地图不存在".to_string()))?;

    // 非管理员只能导入自己名下的地图。
    if !claims.is_admin
        && let Some(zentai_user_id) = claims.zentai_user_id
        && source.user_id != Some(zentai_user_id)
    {
        return Err(AppError::NotFound(
            "地图不存在或不在你的管辖范围内".to_string(),
        ));
    }

    // 园区：请求优先；否则按中台地图携带的园区编码反查
    let park_id = match payload.park_id {
        Some(park_id) => park_id,
        None => {
            let code = source
                .park_code
                .as_deref()
                .map(str::trim)
                .filter(|code| !code.is_empty())
                .ok_or_else(|| {
                    AppError::BadRequest("该中台地图未关联园区，请在导入时指定园区".to_string())
                })?;
            sqlx::query_scalar::<_, i64>("SELECT id FROM parks WHERE park_code = ?1")
                .bind(code)
                .fetch_optional(&state.db)
                .await?
                .ok_or_else(|| AppError::NotFound(format!("园区编码 {code} 未在业务库中登记")))?
        }
    };

    let park_name: String = sqlx::query_scalar("SELECT park_name FROM parks WHERE id = ?1")
        .bind(park_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("园区不存在".to_string()))?;

    let version = source
        .map_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("v1.0")
        .to_string();
    let status = if source.map_status.as_deref() == Some("已发布") {
        "已发布"
    } else {
        "部分标定"
    };
    let source_id = source.id.to_string();
    let timestamp = now();
    let ros_meta = source
        .file_path
        .as_deref()
        .and_then(|path| read_ros_map_meta(&state.config.backend_db_path, path));

    let existing: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM map_versions
         WHERE source = 'backend' AND source_id = ?1 AND park_id = ?2
         ORDER BY created_at ASC LIMIT 1",
    )
    .bind(&source_id)
    .bind(park_id)
    .fetch_optional(&state.db)
    .await?;

    let map_id = if let Some((id,)) = existing {
        id
    } else {
        let mut candidate = format!("ROS-MAP-{}@{}", source.id, version);
        let mut suffix = 1;
        while sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM map_versions WHERE id = ?1")
            .bind(&candidate)
            .fetch_one(&state.db)
            .await?
            > 0
        {
            candidate = format!("ROS-MAP-{}@{}-{}", source.id, version, suffix);
            suffix += 1;
        }
        candidate
    };

    let map_name = format!("ROS-MAP-{}", source.id);
    let frame_id = source
        .frame_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("ros_map_{} / 局部米制", source.id));
    let crs = source
        .crs
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ROS map_server / 局部 ENU".to_string());
    let origin = source
        .map_origin
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| ros_meta.as_ref().and_then(|meta| meta.origin.clone()))
        .unwrap_or_else(|| "0.0, 0.0, 0.0".to_string());
    let resolution = source
        .resolution
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| ros_meta.as_ref().and_then(|meta| meta.resolution.clone()))
        .unwrap_or_else(|| "0.05 m/px".to_string());
    let floors = source
        .floors
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "1F".to_string());
    let map_hash = source
        .map_hash
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| demo_hash(&format!("{source_id}:{map_id}"), &timestamp));
    let file_name = source.file_name.clone();
    let file_path = source.file_path.clone();
    let file_size = source.file_size;
    let note = format!(
        "从 backend 地图库导入「{}」{} · 园区：{}",
        source.map_name,
        file_name
            .as_deref()
            .map(|name| format!(" · 文件 {name}"))
            .unwrap_or_default(),
        park_name
    );
    let published_at = source
        .publish_time
        .clone()
        .unwrap_or_else(|| timestamp.clone());

    if existing_row_exists(&state, &map_id).await? {
        sqlx::query(
            "UPDATE map_versions SET map_name = ?1, version = ?2, frame_id = ?3, crs = ?4,
                origin = ?5, resolution = ?6, floors = ?7, road_count = ?8, no_go_count = ?9,
                status = ?10, map_hash = ?11, map_width = ?12, map_height = ?13, note = ?14,
                published_at = ?15, source = 'backend', source_id = ?16, file_name = ?17,
                file_path = ?18, file_size = ?19, owner_user_id = ?20, owner_username = ?21
             WHERE id = ?22",
        )
        .bind(&map_name)
        .bind(&version)
        .bind(&frame_id)
        .bind(&crs)
        .bind(&origin)
        .bind(&resolution)
        .bind(&floors)
        .bind(source.road_count.unwrap_or(0))
        .bind(source.nogo_count.unwrap_or(0))
        .bind(status)
        .bind(&map_hash)
        .bind(
            source
                .map_width
                .or_else(|| ros_meta.as_ref().and_then(|meta| meta.width)),
        )
        .bind(
            source
                .map_height
                .or_else(|| ros_meta.as_ref().and_then(|meta| meta.height)),
        )
        .bind(&note)
        .bind(&published_at)
        .bind(&source_id)
        .bind(&file_name)
        .bind(&file_path)
        .bind(file_size)
        .bind(source.user_id)
        .bind(&source.owner_username)
        .bind(&map_id)
        .execute(&state.db)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO map_versions (id, park_id, map_name, version, frame_id, crs, origin,
                resolution, floors, road_count, no_go_count, status, map_hash, map_width,
                map_height, note, published_at, created_at, source, source_id, file_name,
                file_path, file_size, owner_user_id, owner_username)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, 'backend', ?19, ?20, ?21, ?22, ?23, ?24)",
        )
        .bind(&map_id)
        .bind(park_id)
        .bind(&map_name)
        .bind(&version)
        .bind(&frame_id)
        .bind(&crs)
        .bind(&origin)
        .bind(&resolution)
        .bind(&floors)
        .bind(source.road_count.unwrap_or(0))
        .bind(source.nogo_count.unwrap_or(0))
        .bind(status)
        .bind(&map_hash)
        .bind(
            source
                .map_width
                .or_else(|| ros_meta.as_ref().and_then(|meta| meta.width)),
        )
        .bind(
            source
                .map_height
                .or_else(|| ros_meta.as_ref().and_then(|meta| meta.height)),
        )
        .bind(&note)
        .bind(&published_at)
        .bind(&timestamp)
        .bind(&source_id)
        .bind(&file_name)
        .bind(&file_path)
        .bind(file_size)
        .bind(source.user_id)
        .bind(&source.owner_username)
        .execute(&state.db)
        .await?;
    }

    if payload.bind {
        let park_status = if status == "部分标定" {
            "部分标定"
        } else {
            "已标定"
        };
        sqlx::query(
            "UPDATE parks SET map_version_id = ?1, map_status = ?2, updated_at = ?3 WHERE id = ?4",
        )
        .bind(&map_id)
        .bind(park_status)
        .bind(&timestamp)
        .bind(park_id)
        .execute(&state.db)
        .await?;
    }

    Ok(ApiResponse::ok(fetch_map(&state, &map_id).await?))
}

pub async fn get_map(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MapVersion>>, AppError> {
    Ok(ApiResponse::ok(fetch_map(&state, &id).await?))
}

/// 渲染 backend 上传的 ROS PGM 栅格地图，返回 PNG data URL。
pub async fn map_raster(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MapRaster>>, AppError> {
    let map = fetch_map(&state, &id).await?;
    let file_path = map
        .file_path
        .clone()
        .ok_or_else(|| AppError::BadRequest("该地图没有关联的 ROS 地图包".to_string()))?;
    let raster = read_ros_raster(&state.config.backend_db_path, &file_path)
        .ok_or_else(|| AppError::BadRequest("无法从地图包中解析 model.pgm".to_string()))?;
    let raster = downscale_raster(raster, 1600);
    let png = encode_png(raster.width, raster.height, &raster.pixels)
        .ok_or_else(|| AppError::Internal("PNG 编码失败".to_string()))?;
    Ok(ApiResponse::ok(MapRaster {
        map_id: map.id,
        image: format!("data:image/png;base64,{}", BASE64.encode(png)),
        resolution: map.resolution,
        origin: map.origin,
        width: raster.width,
        height: raster.height,
    }))
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
    let device_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE park_id = ?1")
        .bind(map.park_id)
        .fetch_one(&state.db)
        .await?;
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
    .bind(map.park_id)
    .execute(&mut *transaction)
    .await?;
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
        "INSERT INTO map_versions (id, park_id, map_name, version, frame_id, crs, origin,
            resolution, floors, road_count, no_go_count, status, map_hash, map_width, map_height,
            note, published_at, created_at, owner_user_id, owner_username)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, '已发布', ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
    )
    .bind(&new_id)
    .bind(map.park_id)
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
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "UPDATE map_versions SET status = '已归档'
         WHERE park_id = ?1 AND id <> ?2 AND status = '已发布'",
    )
    .bind(map.park_id)
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
    .bind(map.park_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(ApiResponse::ok(fetch_map(&state, &new_id).await?))
}

struct RosRaster {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// 从根 backend 上传的 `.zip` 中读取 `model.pgm` 并解码为 8 位灰度像素。
fn read_ros_raster(backend_db: &std::path::Path, relative: &str) -> Option<RosRaster> {
    let upload_root = backend_db.parent()?.join("device_uploads");
    let path = upload_root.join(relative);
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut pgm: Option<Vec<u8>> = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).ok()?;
        if entry.name().to_ascii_lowercase().ends_with(".pgm") {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).ok()?;
            pgm = Some(data);
            break;
        }
    }
    decode_pgm(pgm.as_deref()?)
}

fn decode_pgm(bytes: &[u8]) -> Option<RosRaster> {
    let (magic, width, height, maxval, data_start) = pgm_header(bytes)?;
    let count = (width as usize).checked_mul(height as usize)?;
    let pixels = match magic {
        "P5" if maxval <= 255 => {
            let end = data_start.checked_add(count)?;
            bytes.get(data_start..end)?.to_vec()
        }
        "P5" => {
            let end = data_start.checked_add(count.checked_mul(2)?)?;
            let data = bytes.get(data_start..end)?;
            (0..count)
                .map(|index| ((data[index * 2] as u16) << 8 | data[index * 2 + 1] as u16) >> 8)
                .map(|value| value as u8)
                .collect()
        }
        "P2" => {
            let text = std::str::from_utf8(bytes.get(data_start..)?).ok()?;
            let mut values = Vec::with_capacity(count);
            for token in text.split_ascii_whitespace() {
                if values.len() >= count {
                    break;
                }
                let value: u32 = token.parse().ok()?;
                values.push(if maxval <= 255 {
                    value as u8
                } else {
                    (value >> 8) as u8
                });
            }
            if values.len() < count {
                return None;
            }
            values
        }
        _ => return None,
    };
    Some(RosRaster {
        width,
        height,
        pixels,
    })
}

fn pgm_header(bytes: &[u8]) -> Option<(&str, u32, u32, u32, usize)> {
    let mut tokens: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < bytes.len() && tokens.len() < 4 {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        if bytes[index] == b'#' {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        tokens.push((start, index));
    }
    if tokens.len() < 4 {
        return None;
    }
    let magic = std::str::from_utf8(&bytes[tokens[0].0..tokens[0].1]).ok()?;
    let width = std::str::from_utf8(&bytes[tokens[1].0..tokens[1].1])
        .ok()?
        .parse()
        .ok()?;
    let height = std::str::from_utf8(&bytes[tokens[2].0..tokens[2].1])
        .ok()?
        .parse()
        .ok()?;
    let maxval = std::str::from_utf8(&bytes[tokens[3].0..tokens[3].1])
        .ok()?
        .parse()
        .ok()?;
    let mut data_start = tokens[3].1;
    if data_start < bytes.len() && bytes[data_start] == b'\r' {
        data_start += 1;
    }
    if data_start < bytes.len() && bytes[data_start].is_ascii_whitespace() {
        data_start += 1;
    }
    Some((magic, width, height, maxval, data_start))
}

fn downscale_raster(raster: RosRaster, max_dimension: u32) -> RosRaster {
    let longest = raster.width.max(raster.height);
    if longest <= max_dimension || longest == 0 {
        return raster;
    }
    let scale = max_dimension as f64 / longest as f64;
    let width = ((raster.width as f64 * scale).round() as u32).max(1);
    let height = ((raster.height as f64 * scale).round() as u32).max(1);
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        let source_y = (y as f64 / scale) as u32;
        for x in 0..width {
            let source_x = (x as f64 / scale) as u32;
            let source_index = (source_y.min(raster.height - 1) * raster.width
                + source_x.min(raster.width - 1)) as usize;
            pixels.push(raster.pixels[source_index]);
        }
    }
    RosRaster {
        width,
        height,
        pixels,
    }
}

fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(pixels).ok()?;
    }
    Some(output)
}

struct RosMapMeta {
    resolution: Option<String>,
    origin: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
}

/// 从根 backend 上传的 `.zip` 地图包中读取 ROS map_server 元数据。
/// 包内约定为 `model.yaml` + `model.pgm`。
fn read_ros_map_meta(backend_db: &std::path::Path, relative: &str) -> Option<RosMapMeta> {
    let upload_root = backend_db.parent()?.join("device_uploads");
    let path = upload_root.join(relative);
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    let mut yaml_text = String::new();
    let mut pgm_bytes: Option<Vec<u8>> = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).ok()?;
        let name = entry.name().to_ascii_lowercase();
        if name.ends_with(".yaml") || name.ends_with(".yml") {
            entry.read_to_string(&mut yaml_text).ok()?;
        } else if name.ends_with(".pgm") {
            let mut data = Vec::new();
            entry.read_to_end(&mut data).ok()?;
            pgm_bytes = Some(data);
        }
    }

    let resolution = yaml_text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "resolution")
            .then(|| value.trim().parse::<f64>().ok())
            .flatten()
            .map(|value| format!("{value} m/px"))
    });
    let origin = yaml_text.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim() != "origin" {
            return None;
        }
        let inner = value.trim().trim_start_matches('[').trim_end_matches(']');
        let numbers: Vec<String> = inner
            .split(',')
            .map(str::trim)
            .map(str::to_string)
            .collect();
        (!numbers.is_empty()).then(|| numbers.join(", "))
    });
    let (width, height) = pgm_bytes
        .as_deref()
        .and_then(pgm_dimensions)
        .map(|(w, h)| (Some(w), Some(h)))
        .unwrap_or((None, None));

    Some(RosMapMeta {
        resolution,
        origin,
        width,
        height,
    })
}

fn pgm_dimensions(bytes: &[u8]) -> Option<(i64, i64)> {
    let mut tokens: Vec<&[u8]> = Vec::new();
    let mut index = 0;
    while index < bytes.len() && tokens.len() < 4 {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        if bytes[index] == b'#' {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        tokens.push(&bytes[start..index]);
    }
    if tokens.len() < 3 {
        return None;
    }
    let width = std::str::from_utf8(tokens[1]).ok()?.parse().ok()?;
    let height = std::str::from_utf8(tokens[2]).ok()?.parse().ok()?;
    Some((width, height))
}

async fn backend_pool(state: &AppState) -> Result<SqlitePool, AppError> {
    let path = &state.config.backend_db_path;
    if !path.is_file() {
        return Err(AppError::BadRequest(format!(
            "未找到根 backend 数据库：{}，请确认 backend 已启动并使用默认数据目录",
            path.display()
        )));
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
        .map_err(|error| AppError::Internal(format!("无法连接 backend 数据库: {error}")))?
        .read_only(true)
        .create_if_missing(false);
    SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .map_err(AppError::from)
}

async fn existing_row_exists(state: &AppState, id: &str) -> Result<bool, AppError> {
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

fn bump_version(version: &str) -> String {
    let digits: Vec<i64> = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<i64>().ok())
        .collect();
    let major = digits.first().copied().unwrap_or(0);
    let minor = digits.get(1).copied().unwrap_or(0) + 1;
    format!("v{major}.{minor}")
}

fn demo_hash(id: &str, timestamp: &str) -> String {
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    timestamp.hash(&mut hasher);
    format!(
        "{:016x}{:016x}",
        hasher.finish(),
        hasher.finish().rotate_left(17)
    )
}
