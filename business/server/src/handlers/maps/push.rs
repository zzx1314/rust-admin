//! 把地图推给狗体（中台直连 / 反向控制信道共用）。

// 同一模块拆出来的几块共用一个词汇表（`MAP_SELECT` / helper / `AppState`），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/// 「下发地图」请求（中台 → 业务平台）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushMapRequest {
    /// 中台 `dog_map.id`
    pub map_id: i64,
    /// 幂等键（中台审计用，业务侧只回显）
    #[serde(default)]
    pub request_id: Option<String>,
}

/// 把中台最新地图包写回狗体：刷新业务库副本 → 下发 `下发地图` 指令。
///
/// 前置条件：这张中台地图已经导入业务库（有 `source_id` 对应行），且行上有本体
/// 目录名 `source_ref`——否则不知道写到本体哪个目录，宁可直接报错。
pub async fn apply_push_map(
    state: &AppState,
    device_code: &str,
    payload: &PushMapRequest,
) -> Result<Value, AppError> {
    let device_code = device_code.trim();
    if payload.map_id <= 0 {
        return Err(AppError::BadRequest("缺少 mapId".to_string()));
    }
    let device = crate::handlers::devices::fetch_device_by_code(state, device_code).await?;

    // 1) 读中台 dog_map（中台才是地图主库）
    let backend = backend_pool(state).await?;
    let has_park = dog_map_has_park_columns(&backend).await?;
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
        .bind(payload.map_id)
        .fetch_optional(&backend)
        .await?
        .ok_or_else(|| {
            AppError::NotFound(format!("中台地图 #{} 不存在（可能已删除）", payload.map_id))
        })?;
    let relative = source
        .file_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("中台地图尚未上传地图包，无法下发".to_string()))?;
    let package_path = resolve_package_path(&state.config.backend_db_path, relative)
        .ok_or_else(|| AppError::BadRequest(format!("中台地图包文件不存在：{relative}")))?;
    let package = tokio::fs::read(&package_path)
        .await
        .map_err(|error| AppError::Internal(format!("读取中台地图包失败: {error}")))?;
    let package_hash = ingest::sha256_hex(&package);

    // 2) 业务库里对应的地图行（同一张中台地图可能被导入到多个园区）
    let rows: Vec<(String, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT id, source_ref, park_id FROM map_versions
         WHERE source_id = ?1 ORDER BY created_at DESC",
    )
    .bind(payload.map_id.to_string())
    .fetch_all(&state.db)
    .await?;
    if rows.is_empty() {
        return Err(AppError::BadRequest(format!(
            "中台地图 #{} 还没有导入业务平台（园区地图基线里看不到），无法下发到本体",
            payload.map_id
        )));
    }
    let main = rows
        .iter()
        .find(|(_, _, park_id)| device.park_id.is_some() && *park_id == device.park_id)
        .or_else(|| rows.first())
        .expect("rows is not empty");
    let map_id = main.0.clone();
    let source_ref = main
        .1
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(
                "该地图没有记录本体目录名（source_ref），无法确定写回本体哪个目录；请先在设备上同步这张图"
                    .to_string(),
            )
        })?;
    let map_dir = crate::mapdir::dir_from_source_ref(source_ref)
        .ok_or_else(|| AppError::BadRequest(format!("本体目录名无法解析：{source_ref}")))?;

    // 3) 刷新业务库副本：把中台最新包暂存到 ingest 目录，业务预览 / 下发都用它
    let file_name = source
        .file_name
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{map_dir}.zip"));
    let stash_name = format!("push-{map_id}-{file_name}");
    let stash = ingest::stash_package(&state.config.ingest_dir, &stash_name, &package).await?;
    let file_path = stash.display().to_string();
    let timestamp = now();
    for (row_id, _, _) in &rows {
        sqlx::query(
            "UPDATE map_versions SET file_path = ?1, file_name = ?2, file_size = ?3,
                map_hash = ?4, map_width = ?5, map_height = ?6,
                origin = COALESCE(?7, origin), resolution = COALESCE(?8, resolution)
             WHERE id = ?9",
        )
        .bind(&file_path)
        .bind(&file_name)
        .bind(package.len() as i64)
        .bind(&package_hash)
        .bind(source.map_width)
        .bind(source.map_height)
        .bind(source.map_origin.clone())
        .bind(source.resolution.clone())
        .bind(row_id)
        .execute(&state.db)
        .await?;
    }

    // 4) 下发指令：客户端拉包 → 写本体文件服务 → 重新载图
    //
    // 中台/浏览器要的「是否成功」不是 `wsDelivered`：帧送进设备连接只代表送达，
    // 不代表客户端真的把文件写进本体。带 requestId 的调用先登记等待，客户端执行完
    // 用 `command_result` 回报，下面再按真实结果返回成功 / 失败。
    let package_path = format!("/api/map-versions/{map_id}/package");
    let request_id = payload.request_id.clone().unwrap_or_default();
    let wait_request_id = request_id.trim().to_string();
    let result_rx = if wait_request_id.is_empty() {
        None
    } else {
        Some(gateway::expect_command_result(&state.command_results, &wait_request_id).await)
    };
    let dispatched = gateway::send_command_to_device(
        &state.device_connections,
        device_code,
        "下发地图",
        Some(json!({
            "mapId": map_id,
            "mapName": source.map_name,
            "mapVersion": source.map_version,
            "mapHash": package_hash,
            "sourceRef": source_ref,
            "mapDir": map_dir,
            "packagePath": package_path,
            "requestId": request_id,
            "requestedAt": timestamp,
        })),
    )
    .await;
    if !dispatched {
        if !wait_request_id.is_empty() {
            gateway::cancel_command_result(&state.command_results, &wait_request_id).await;
        }
        let healed = gateway::mark_offline_if_online(&state.db, device_code).await;
        return Err(AppError::BadRequest(format!(
            "设备 {device_code} 未连接（台账状态 {}，连接池无该设备连接），地图未下发{}；请确认设备客户端已启动且能连上业务平台",
            device.status,
            if healed {
                "，已将其标记为离线"
            } else {
                ""
            },
        )));
    }

    let execution = match result_rx {
        Some(rx) => match gateway::wait_command_result(
            &state.command_results,
            &wait_request_id,
            rx,
            PUSH_MAP_RESULT_TIMEOUT,
        )
        .await
        {
            Ok(result) => Some(result),
            Err(message) => {
                record_map_push_event(
                    state,
                    MapPushEvent {
                        device_pk: device.id,
                        device_code,
                        map_name: &source.map_name,
                        map_id: payload.map_id,
                        map_dir: &map_dir,
                        package_hash: &package_hash,
                        ok: false,
                        message: &format!("等待设备回执失败：{message}"),
                    },
                )
                .await?;
                return Err(AppError::BadRequest(format!(
                    "地图已送达设备，但未能确认执行结果：{message}。请在设备客户端日志中查看「下发地图」结果"
                )));
            }
        },
        None => None,
    };

    let (ok, message) = match &execution {
        Some(result) if !result.ok => (
            false,
            if result.message.trim().is_empty() {
                "设备执行下发地图失败（客户端未返回原因）".to_string()
            } else {
                result.message.clone()
            },
        ),
        Some(result) => (true, result.message.clone()),
        None => (
            true,
            "设备已收到下发指令（调用方未提供 requestId，无法确认执行结果）".to_string(),
        ),
    };
    record_map_push_event(
        state,
        MapPushEvent {
            device_pk: device.id,
            device_code,
            map_name: &source.map_name,
            map_id: payload.map_id,
            map_dir: &map_dir,
            package_hash: &package_hash,
            ok,
            message: &message,
        },
    )
    .await?;

    if !ok {
        return Err(AppError::BadRequest(format!(
            "设备 {device_code} 执行下发地图失败：{message}"
        )));
    }

    Ok(json!({
        "deviceId": device_code,
        "mapId": map_id,
        "mapName": source.map_name,
        "mapDir": map_dir,
        "sourceRef": source_ref,
        "mapHash": package_hash,
        "packagePath": package_path,
        "mapCount": rows.len(),
        "wsDelivered": true,
        "awaited": execution.is_some(),
        "ok": true,
        "message": message,
        "deviceResult": execution.map(|result| json!({
            "ok": result.ok,
            "message": result.message,
            "data": result.data,
        })),
        "requestId": request_id,
        "dispatchedAt": timestamp,
    }))
}
/// 「下发地图」等待设备真实回执的最长时间。
///
/// 地图包几 MB + 写文件 + 4030 重新载图，现场通常 5~20s；60s 能覆盖慢链路，
/// 超时按「结果未知」返回，而不是假装成功。
pub(crate) const PUSH_MAP_RESULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) struct MapPushEvent<'a> {
    device_pk: i64,
    device_code: &'a str,
    map_name: &'a str,
    map_id: i64,
    map_dir: &'a str,
    package_hash: &'a str,
    ok: bool,
    message: &'a str,
}

pub(crate) async fn record_map_push_event(
    state: &AppState,
    event: MapPushEvent<'_>,
) -> Result<(), AppError> {
    let title = if event.ok {
        format!("地图下发成功：{}", event.map_name)
    } else {
        format!("地图下发失败：{}", event.map_name)
    };
    let description = format!(
        "设备 {} · 中台 dog_map #{} · 本体目录 {} · map_hash {} · 设备回执：{}",
        event.device_code, event.map_id, event.map_dir, event.package_hash, event.message
    );
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('map_push', ?1, ?2, ?3, NULL, ?4)",
    )
    .bind(title)
    .bind(description)
    .bind(event.device_pk)
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(())
}
