//! 中台（`dog_map`）侧的地图库：已同步来源、可导入清单、ROS 元信息。
//!
//! 这里只读**根 backend 的 SQLite**，不写业务库。

// 同一模块拆出来的几块共用一个词汇表（`MAP_SELECT` / helper / `AppState`），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncedSourcesQuery {
    pub park_id: Option<i64>,
    /// 设备编号：地图跟着设备走，手动同步按设备判重（首选）
    pub device_id: Option<String>,
}

/// `GET /api/map-versions/synced-sources?deviceId=...`（兼容 `?parkId=...`）
///
/// 返回已入库的机器人地图路径（`source_ref`）。设备客户端手动同步前先拉这个列表，
/// 跳过已同步地图，避免重复下载大文件。
///
/// 地图归属先看设备再看园区：同一台设备（或未登记设备的历史行）的本体路径只回传一次，
/// 设备没绑定园区也能正常判重。
///
/// 「已同步」还要求中台那张地图仍然存在：中台是地图主库，某张图在中台被删掉后，
/// 业务库里的历史行不能算已同步，否则设备端永远跳过、中台列表里也永远看不到。
/// 中台不可达时退回旧行为（按业务库记录判定），不把网络抖动放大成整机重传。
pub async fn list_synced_sources(
    State(state): State<AppState>,
    Query(query): Query<SyncedSourcesQuery>,
) -> Result<Json<ApiResponse<Vec<String>>>, AppError> {
    let device_id = query
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if device_id.is_none() && query.park_id.is_none() {
        return Err(AppError::BadRequest(
            "缺少 deviceId（或 parkId），无法判定哪些地图已同步".to_string(),
        ));
    }
    let sql = if device_id.is_some() {
        // 升级前入库的行没有 device_code：按其本体路径算已同步，宁可漏传不重传
        "SELECT source_ref, source_id FROM map_versions
         WHERE (device_code = ?1 OR device_code IS NULL)
           AND source_ref IS NOT NULL AND source_ref <> ''
         ORDER BY created_at ASC"
    } else {
        "SELECT source_ref, source_id FROM map_versions
         WHERE park_id = ?1 AND source_ref IS NOT NULL AND source_ref <> ''
         ORDER BY created_at ASC"
    };
    let mut statement = sqlx::query_as::<_, (String, Option<String>)>(sql);
    statement = match device_id.as_deref() {
        Some(device_id) => statement.bind(device_id),
        None => statement.bind(query.park_id),
    };
    let rows = statement.fetch_all(&state.db).await?;

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
pub(crate) fn zentai_id(source_id: Option<&str>) -> Option<i64> {
    source_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<i64>().ok())
}

/// 中台 `dog_map` 的列清单
///
/// `park_code` / `park_name` 由根 backend 迁移 26 添加。若根库还是旧结构（迁移未跑），
/// 退化成 NULL，避免整个地图列表接口因缺少一列而 500。
pub(crate) fn dog_map_select_columns(has_park: bool) -> &'static str {
    if has_park {
        "d.park_code, d.park_name, d.user_id, u.username AS owner_username"
    } else {
        "NULL AS park_code, NULL AS park_name, d.user_id, u.username AS owner_username"
    }
}

/// 判断根库 `dog_map` 是否已经具备园区列
pub(crate) async fn dog_map_has_park_columns(pool: &SqlitePool) -> Result<bool, AppError> {
    let columns: Vec<(String,)> = sqlx::query_as("SELECT name FROM pragma_table_info('dog_map')")
        .fetch_all(pool)
        .await?;
    Ok(columns.iter().any(|(name,)| name == "park_code"))
}

/// 根库是否已有地图分享表（根 backend 迁移 35 才有）。
/// 旧库（业务平台先于中台升级）降级为「没有分享」，不能因为缺表把地图列表打挂。
pub(crate) async fn dog_map_has_share_table(pool: &SqlitePool) -> Result<bool, AppError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'dog_map_share'",
    )
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}

/// 中台地图是否分享给了「分配给该中台账号」的设备。
///
/// 中台地图列表与这里同一口径：分享出去的地图，目标设备的管理员也能看到、
/// 也能导入业务库，不需要重新建图。
pub(crate) async fn map_shared_to_user(
    pool: &SqlitePool,
    map_id: i64,
    user_id: i64,
) -> Result<bool, AppError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM dog_map_share s
         JOIN dog_device d ON d.device_id = s.device_code AND d.is_deleted = 0
         JOIN dog_user_device ud ON ud.device_id = d.id AND ud.is_deleted = 0
         WHERE s.is_deleted = 0 AND s.map_id = ?1 AND ud.user_id = ?2",
    )
    .bind(map_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
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
    // 业务用户只能看到自己名下机器狗采集的地图（含共享给自己管辖设备的图）；
    // 同一台狗绑定多个人时，`d.user_id` 只记第一个归属人，所以还要放行
    // 「上报设备分配给了我」的图，否则后绑定的人在导入列表里看不到本狗的地图。
    // 中台管理员可看全部，也可用 ownerUserId 指定查看某个用户的地图。
    let owner_filter = if claims.is_admin {
        query.owner_user_id
    } else {
        claims.zentai_user_id
    };
    if let Some(owner) = owner_filter {
        if dog_map_has_share_table(&pool).await? {
            builder
                .push(" AND (d.user_id = ")
                .push_bind(owner)
                .push(
                    " OR d.device_code IN (SELECT dev.device_id FROM dog_device dev \
                     JOIN dog_user_device ud ON ud.device_id = dev.id AND ud.is_deleted = 0 \
                     WHERE dev.is_deleted = 0 AND ud.user_id = ",
                )
                .push_bind(owner)
                .push(
                    ") OR d.id IN (SELECT s.map_id FROM dog_map_share s \
                     JOIN dog_device dev ON dev.device_id = s.device_code AND dev.is_deleted = 0 \
                     JOIN dog_user_device ud ON ud.device_id = dev.id AND ud.is_deleted = 0 \
                     WHERE s.is_deleted = 0 AND ud.user_id = ",
                )
                .push_bind(owner)
                .push("))");
        } else {
            builder.push(" AND d.user_id = ").push_bind(owner);
        }
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
pub(crate) struct RosMapMeta {
    pub(crate) resolution: Option<String>,
    pub(crate) origin: Option<String>,
    pub(crate) width: Option<i64>,
    pub(crate) height: Option<i64>,
}

/// 从根 backend 上传的 `.zip` 地图包中读取 ROS map_server 元数据。
/// 优先 `map_edit/`（遥控器编辑版），没有编辑版时才用 `model.yaml` + `model.pgm`。
pub(crate) fn read_ros_map_meta(
    backend_db: &std::path::Path,
    relative: &str,
) -> Option<RosMapMeta> {
    let upload_root = backend_db.parent()?.join("device_uploads");
    let path = upload_root.join(relative);
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;

    let mut names: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if !entry.is_dir() {
            names.push(entry.name().to_string());
        }
    }

    let mut yaml_text = String::new();
    if let Some(name) =
        ingest::preferred_map_entry(names.iter().map(String::as_str), &[".yaml", ".yml"])
    {
        let mut entry = archive.by_name(name).ok()?;
        entry.read_to_string(&mut yaml_text).ok()?;
    }
    let pgm_bytes = ingest::preferred_map_entry(names.iter().map(String::as_str), &[".pgm"])
        .and_then(|name| {
            let mut entry = archive.by_name(name).ok()?;
            let mut data = Vec::new();
            entry.read_to_end(&mut data).ok()?;
            Some(data)
        });

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

pub(crate) fn pgm_dimensions(bytes: &[u8]) -> Option<(i64, i64)> {
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
pub(crate) async fn backend_pool(state: &AppState) -> Result<SqlitePool, AppError> {
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
