//! `POST /api/map-versions/ingest` — 接收设备客户端回传的机器人建图结果。
//!
//! 与 `/api/map-versions/import-backend` 的区别：
//! - import-backend 是「中台已有包 → 导入业务库」，需要读根 backend 的 SQLite；
//! - ingest 是「设备刚建好图 → 先推入中台 dog_map → 再导入业务库」，两步一次完成。
//!
//! 请求为 multipart：
//! - `file`：地图 `.zip` 包（必填）
//! - `mapInfo`：`{ mapName, parkId, bind, mapType, remarks, source, sourceRef, mode }`
//!
//! `mode` 决定去重语义（见 [`crate::ingest::IngestMode`]）：
//! - `incremental`（默认）：命中 `sourceRef` 直接跳过，只补平台没有的图；
//! - `refresh`：中台/控制台显式「同步地图」时用，内容指纹变了就原地覆盖中台
//!   同一条 `dog_map`（`mapInfo.mapId`），内容没变仍然跳过。中台的不可变版本
//!   （已发布 / 部分标定 / 已归档）会拒绝覆盖，错误原样带回同步报告。

use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::ingest::{self, IngestRequest};
use crate::zentai;
use axum::Json;
use axum::extract::{Multipart, State};
use serde_json::{Value, json};
use tracing::{info, warn};

pub async fn ingest_map(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let mut info_json: Option<Value> = None;
    let mut file: Option<(String, Vec<u8>)> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("无效的上传请求: {e}")))?
    {
        match field.name() {
            Some("file") => {
                let name = field.file_name().unwrap_or("map.zip").to_string();
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("读取地图包失败: {e}")))?;
                file = Some((name, data.to_vec()));
            }
            Some("mapInfo") => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("读取地图信息失败: {e}")))?;
                info_json = Some(
                    serde_json::from_str(&text)
                        .map_err(|e| AppError::BadRequest(format!("地图信息格式错误: {e}")))?,
                );
            }
            _ => {}
        }
    }

    let info_json = info_json.ok_or_else(|| AppError::BadRequest("缺少 mapInfo".to_string()))?;
    let request = IngestRequest::from_json(&info_json)?;
    let (file_name, package) =
        file.ok_or_else(|| AppError::BadRequest("请上传地图包".to_string()))?;

    if !package.starts_with(b"PK") {
        return Err(AppError::BadRequest("地图包不是有效的 zip".to_string()));
    }
    if package.len() > 256 * 1024 * 1024 {
        return Err(AppError::BadRequest("地图包超过 256MB 上限".to_string()));
    }
    // 园区是可选的：地图只跟设备有关系。设备还没绑定园区时先按「设备地图」入库
    // （`park_id IS NULL`），设备绑定人登录后创建园区时再从自己的设备地图里挑一张绑定。
    let park_ref = match request.park_id {
        Some(park_id) => {
            let park: Option<(String, String)> =
                sqlx::query_as("SELECT park_code, park_name FROM parks WHERE id = ?1")
                    .bind(park_id)
                    .fetch_optional(&state.db)
                    .await?;
            let Some((park_code, park_name)) = park else {
                return Err(AppError::NotFound(format!("园区 {park_id} 不存在")));
            };
            Some(ingest::ParkRef {
                code: park_code,
                name: park_name,
            })
        }
        None => None,
    };
    let device_code = request
        .device_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    // 去重：同一台设备上同一机器人地图路径只入库一次。
    // 自动回传（4001）与中台手动同步（4012 + 文件服务）共用这个判断；
    // 设备未必绑定园区，所以按 `device_code` 而不按园区判重，
    // 升级前入库的历史行没有 device_code，一并算已同步（宁可漏传不重传）。
    //
    // 「已同步」还要求中台那张地图仍存在：中台是地图主库，图被中台删掉（软删）后
    // 业务库里的历史行不能算已同步，否则这张图在中台永远回不来。
    // 这种场景不换业务地图 id：路线 / 任务通过 `map_version_id` 引用它。
    //
    // 两种模式的区别（`mapInfo.mode`）：
    // - incremental（默认）：命中就跳过，只补平台没有的图，设备定时轮询走这条；
    // - refresh：中台 / 控制台显式点「同步地图」时用，要求「全部更新到本体最新」，
    //   所以不按 sourceRef 跳过；只有内容指纹完全一致才跳过，变了就覆盖中台同一条记录。
    let refresh = request.mode.is_refresh();
    let mut reuse_map_id: Option<String> = None;
    let mut replace_zentai_id: Option<i64> = None;
    if let Some(source_ref) = request
        .source_ref
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        // 同一台机器人的同一个本体目录可能留下**好几行**（历史重复回传 / 派生版本 /
        // 归档）。以前 `LIMIT 1` 不带任何排序，踩到哪行算哪行：2026-11-27 现场就是
        // 挑中了已归档的 `ROS-MAP-1795402650069@v1.0`（指向一张「已发布」的中台地图），
        // 之后每次「同步地图」都去覆盖那张已发布图、被 400 拒掉，整条同步报失败。
        // 现在按「同一设备的行优先、未归档的优先、最新的优先」挑。
        let existing: Option<(String, Option<String>, Option<String>, String)> =
            match device_code.as_deref() {
                Some(code) => {
                    sqlx::query_as(
                        "SELECT id, source_id, file_path, status FROM map_versions
                     WHERE (device_code = ?1 OR device_code IS NULL) AND source_ref = ?2
                     ORDER BY CASE WHEN device_code = ?1 THEN 0 ELSE 1 END,
                              CASE WHEN status = '已归档' THEN 1 ELSE 0 END,
                              created_at DESC, id DESC
                     LIMIT 1",
                    )
                    .bind(code)
                    .bind(source_ref)
                    .fetch_optional(&state.db)
                    .await?
                }
                None => {
                    sqlx::query_as(
                        "SELECT id, source_id, file_path, status FROM map_versions
                     WHERE source_ref = ?1
                     ORDER BY CASE WHEN status = '已归档' THEN 1 ELSE 0 END,
                              created_at DESC, id DESC
                     LIMIT 1",
                    )
                    .bind(source_ref)
                    .fetch_optional(&state.db)
                    .await?
                }
            };
        if let Some((map_id, source_id, stored_file, status)) = existing {
            let archived = status.trim() == "已归档";
            let stale_zentai_id = source_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse::<i64>().ok());
            let alive = match stale_zentai_id {
                Some(zentai_id) => {
                    match zentai::alive_map_ids(&state.config, &[zentai_id]).await {
                        Ok(alive) => Some(alive.contains(&zentai_id)),
                        Err(error) => {
                            // 中台不可达：按已同步处理，宁可漏传也不重复下载大文件
                            warn!("校验中台地图 {zentai_id} 是否仍存在失败，按已同步处理: {error}");
                            None
                        }
                    }
                }
                // 没有中台 id 的历史行（如手工导入）无法校验，仍按已同步处理
                None => None,
            };

            if refresh && archived {
                // 归档行不是这个本体目录的「有效副本」：拿它去覆盖中台只会撞上
                // 「已发布不可原地覆盖」，也不能把它复活。按新图入库，归档行原样留档。
                info!(
                    "地图 {source_ref} 的业务行 {map_id} 已归档，按新图回传（中台 {} 不动）",
                    stale_zentai_id
                        .map(|id| format!("#{id}"))
                        .unwrap_or_else(|| "记录".to_string())
                );
            } else if refresh {
                // 刷新：内容没变就不折腾中台（客户端每次现场重打包，zip 时间戳不同，
                // 所以比的是「内容指纹」而不是整包哈希）。
                //
                // ⚠️ 但「中台那张图被删了」必须重传：中台是地图主库，运维在中台删掉
                // 一张机器人地图后点「同步地图」，唯一的指望就是把图拿回来。以前这里
                // 不看 `alive` 就按内容未变化跳过，等于**删了就永远补不回来**
                // （2026-11-27 现场：删掉 2026_11_27_09_54_17 再同步，回执「无需更新 4」，
                // 中台列表里依然没有那张图）。
                let deleted_in_zentai = alive == Some(false);
                if !deleted_in_zentai
                    && package_matches_stored(&state, stored_file.as_deref(), &package).await
                {
                    let map = crate::handlers::maps::fetch_map(&state, &map_id).await?;
                    info!("地图 {source_ref} 内容未变化，刷过同步（{map_id}）");
                    return Ok(ApiResponse::ok(json!({
                        "mapId": map.id,
                        "mapName": map.map_name,
                        "parkId": map.park_id,
                        "zentaiMapId": stale_zentai_id,
                        "bound": false,
                        "skipped": true,
                        "mode": request.mode.as_str(),
                        "reason": "地图包内容未变化，平台副本已是最新",
                        "stashPath": Value::Null,
                        "map": map,
                    })));
                }
                reuse_map_id = Some(map_id.clone());
                // 中台那张图还在 → 原地覆盖它，不在地图列表里多出一份副本；
                // 已被删掉则不传 id，按新建恢复。
                if alive == Some(true) {
                    replace_zentai_id = stale_zentai_id;
                }
                info!(
                    "刷新地图 {source_ref}：复用业务地图 {map_id}，中台{}",
                    replace_zentai_id
                        .map(|id| format!("覆盖 dog_map #{id}"))
                        .unwrap_or_else(|| "记录已不存在，重新上传".to_string())
                );
            } else {
                if alive != Some(false) {
                    let map = crate::handlers::maps::fetch_map(&state, &map_id).await?;
                    info!("地图 {source_ref} 已同步过，跳过重复入库（{map_id}）");
                    return Ok(ApiResponse::ok(json!({
                        "mapId": map.id,
                        "mapName": map.map_name,
                        "parkId": map.park_id,
                        "zentaiMapId": Value::Null,
                        "bound": false,
                        "skipped": true,
                        "mode": request.mode.as_str(),
                        "reason": "sourceRef 已存在，跳过重复同步",
                        "stashPath": Value::Null,
                        "map": map,
                    })));
                }
                info!(
                    "中台地图 {} 已被删除，重新上传并复用业务地图 {map_id}",
                    stale_zentai_id.unwrap_or_default()
                );
                reuse_map_id = Some(map_id);
            }
        }
    }

    // 本地留档：中台失败时可直接重放，不必再回机器人取包
    let stash_path = ingest::stash_package(&state.config.ingest_dir, &file_name, &package).await?;
    info!(
        "收到机器人建图回传: {} ({} 字节) → {}",
        request.map_name,
        package.len(),
        stash_path.display()
    );

    // 先本地解析一次：即使中台不可用，也能拿到尺寸/坐标系用于报错定位
    let local = ingest::parse_local_meta(&package);

    // 地图归属：设备在中台被分配给谁，地图就记在谁名下。
    // 查询失败只降级为服务账号名下，不阻断回传。
    let owner_user_id = match request.device_id.as_deref() {
        Some(device_id) if state.config.zentai_ready() => {
            match zentai::device_owner(&state.config, device_id).await {
                Ok(owner) => owner,
                Err(error) => {
                    warn!("查询设备 {device_id} 的中台归属失败: {error}");
                    None
                }
            }
        }
        _ => None,
    };

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| AppError::Internal(format!("构造 HTTP 客户端失败: {e}")))?;

    // 第一跳：推入中台 dog_map（刷新模式下带 mapId 原地覆盖）
    let zentai = ingest::push_to_zentai(
        &state.config,
        &http,
        &request,
        park_ref.as_ref(),
        &file_name,
        &package,
        owner_user_id,
        replace_zentai_id,
    )
    .await
    .inspect_err(|e| {
        warn!(
            "地图推入中台失败: {e}（包已暂存 {}，可重放）",
            stash_path.display()
        );
    })?;

    // 第二跳：写入业务库 map_versions
    let source_id = zentai.id.to_string();
    let version = ingest::ROBOT_MAP_VERSION.to_string();
    let timestamp = now();
    // 中台删掉后重传时沿用原业务地图 id（路线/任务引用它），否则按中台新 id 生成
    let map_id = reuse_map_id
        .clone()
        .unwrap_or_else(|| format!("ROS-MAP-{source_id}@{version}"));
    let note = request.remarks.clone().unwrap_or_else(|| {
        format!(
            "机器人建图自动回传 · 中台 dog_map #{}{}",
            zentai.id,
            request
                .source_ref
                .as_deref()
                .map(|p| format!(" · 本体路径 {p}"))
                .unwrap_or_default()
        )
    });

    sqlx::query(
        "INSERT INTO map_versions (id, park_id, device_code, map_name, version, frame_id, crs,
            origin, resolution, floors, road_count, no_go_count, status, map_hash, map_width,
            map_height, note, published_at, created_at, source, source_id, file_name,
            file_path, file_size, owner_user_id, source_ref)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '1F', 0, 0, '部分标定', ?10, ?11, ?12,
            ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
         ON CONFLICT(id) DO UPDATE SET
            -- 设备地图首次回传时园区为空，但重新同步不应把已有园区归属冲掉
            park_id = COALESCE(excluded.park_id, map_versions.park_id),
            device_code = COALESCE(excluded.device_code, map_versions.device_code),
            map_name = excluded.map_name,
            map_hash = excluded.map_hash,
            map_width = excluded.map_width,
            map_height = excluded.map_height,
            origin = excluded.origin,
            resolution = excluded.resolution,
            note = excluded.note,
            file_name = excluded.file_name,
            file_path = excluded.file_path,
            file_size = excluded.file_size,
            owner_user_id = COALESCE(excluded.owner_user_id, map_versions.owner_user_id),
            source_id = excluded.source_id,
            source_ref = excluded.source_ref",
    )
    .bind(&map_id)
    .bind(request.park_id)
    .bind(&device_code)
    .bind(&zentai.map_name)
    .bind(&version)
    .bind(format!("ros_map_{} / 局部米制", zentai.id))
    .bind("ROS map_server / 局部 ENU")
    .bind(&local.origin)
    .bind(&local.resolution)
    .bind(&local.sha256)
    .bind(local.width as i64)
    .bind(local.height as i64)
    .bind(&note)
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(request.source.as_deref().unwrap_or("robot-mapping"))
    .bind(&source_id)
    .bind(zentai.file_name.clone().unwrap_or(file_name.clone()))
    .bind(stash_path.display().to_string())
    .bind(package.len() as i64)
    .bind(owner_user_id)
    .bind(
        request
            .source_ref
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    )
    .execute(&state.db)
    .await?;

    // 可选：绑定为园区当前基线（设备未绑定园区时没有可绑的对象，忽略）
    if request.bind {
        match request.park_id {
            Some(park_id) => {
                sqlx::query(
                    "UPDATE parks SET map_version_id = ?1, map_status = '部分标定', updated_at = ?2
                     WHERE id = ?3",
                )
                .bind(&map_id)
                .bind(&timestamp)
                .bind(park_id)
                .execute(&state.db)
                .await?;
                info!("地图 {map_id} 已绑定为园区 {park_id} 的当前基线");
            }
            None => info!("地图 {map_id} 暂未归属园区（设备未绑定园区），跳过基线绑定"),
        }
    }

    let map = crate::handlers::maps::fetch_map(&state, &map_id).await?;
    Ok(ApiResponse::ok(json!({
        "mapId": map.id,
        "mapName": map.map_name,
        "parkId": map.park_id,
        "zentaiMapId": zentai.id,
        "bound": request.bind,
        "skipped": false,
        "mode": request.mode.as_str(),
        "replaced": replace_zentai_id.is_some(),
        "stashPath": stash_path.display().to_string(),
        "map": map,
    })))
}

/// 平台侧已有副本与刚上传的包内容是否完全一致（按内容指纹，不看 zip 时间戳）。
///
/// 读不到本地副本（已删、路径失效）时一律当作「有变化」：宁可多传一次，
/// 也不要把「本体的新图」误判成已经同步。
async fn package_matches_stored(
    state: &AppState,
    stored_file: Option<&str>,
    package: &[u8],
) -> bool {
    let Some(stored_file) = stored_file.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    let Some(path) =
        crate::handlers::maps::resolve_package_path(&state.config.backend_db_path, stored_file)
    else {
        return false;
    };
    match tokio::fs::read(&path).await {
        Ok(stored) => ingest::package_fingerprint(&stored) == ingest::package_fingerprint(package),
        Err(error) => {
            warn!("读取本地地图副本失败（{}）: {error}", path.display());
            false
        }
    }
}
