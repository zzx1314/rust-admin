//! 从中台 `dog_map` 导入一张地图（含地图包落盘、版本对齐）。

// 同一模块拆出来的几块共用一个词汇表（`MAP_SELECT` / helper / `AppState`），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

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

    // 非管理员只能导入自己名下的地图，或中台分享给自己管辖设备的地图。
    if !claims.is_admin
        && let Some(zentai_user_id) = claims.zentai_user_id
        && source.user_id != Some(zentai_user_id)
    {
        let shared = dog_map_has_share_table(&backend_pool).await?
            && map_shared_to_user(&backend_pool, source.id, zentai_user_id).await?;
        if !shared {
            return Err(AppError::NotFound(
                "地图不存在或不在你的管辖范围内".to_string(),
            ));
        }
    }

    // 园区：请求优先；否则按中台地图携带的园区编码反查
    // 状态由中台统一管理：只允许导入「已发布」的图，与园区基线绑定同一道门禁
    // （控制台已不再列「backend 地图库」，这里留着给脚本 / 排查用）。
    if source.map_status.as_deref() != Some("已发布") {
        return Err(AppError::BadRequest(format!(
            "中台地图 #{} 当前状态为 {}：只能导入中台「已发布」的地图（请先在中台发布）",
            source.id,
            source.map_status.as_deref().unwrap_or("草稿")
        )));
    }
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
    // 本体目录名：同名包已经登记过（设备回传过）就沿用，避免导入出一张本体上找不到的重复地图
    let source_ref =
        crate::mapdir::source_ref_for_file_name(&state.db, file_name.as_deref()).await?;
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
                file_path = ?18, file_size = ?19, owner_user_id = ?20, owner_username = ?21,
                source_ref = COALESCE(?23, source_ref)
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
        .bind(&source_ref)
        .execute(&state.db)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO map_versions (id, park_id, map_name, version, frame_id, crs, origin,
                resolution, floors, road_count, no_go_count, status, map_hash, map_width,
                map_height, note, published_at, created_at, source, source_id, file_name,
                file_path, file_size, owner_user_id, owner_username, source_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                ?16, ?17, ?18, 'backend', ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
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
        .bind(&source_ref)
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
