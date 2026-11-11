//! `POST /api/map-versions/ingest` — 接收设备客户端回传的机器人建图结果。
//!
//! 与 `/api/map-versions/import-backend` 的区别：
//! - import-backend 是「中台已有包 → 导入业务库」，需要读根 backend 的 SQLite；
//! - ingest 是「设备刚建好图 → 先推入中台 dog_map → 再导入业务库」，两步一次完成。
//!
//! 请求为 multipart：
//! - `file`：地图 `.zip` 包（必填）
//! - `mapInfo`：`{ mapName, parkId, bind, mapType, remarks, source, sourceRef }`

use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::ingest::{self, IngestRequest};
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
    let Some(park_id) = request.park_id else {
        return Err(AppError::BadRequest(
            "缺少 parkId，无法确定地图归属园区（请先为设备绑定园区）".to_string(),
        ));
    };
    let park: Option<(String, String)> =
        sqlx::query_as("SELECT park_code, park_name FROM parks WHERE id = ?1")
            .bind(park_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((park_code, park_name)) = park else {
        return Err(AppError::NotFound(format!("园区 {park_id} 不存在")));
    };
    let park_ref = ingest::ParkRef {
        code: park_code,
        name: park_name,
    };

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

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| AppError::Internal(format!("构造 HTTP 客户端失败: {e}")))?;

    // 第一跳：推入中台 dog_map
    let zentai = ingest::push_to_zentai(
        &state.config,
        &http,
        &request,
        &park_ref,
        &file_name,
        &package,
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
    let version = "v1.0".to_string();
    let timestamp = now();
    let map_id = format!("ROS-MAP-{source_id}@{version}");
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
        "INSERT INTO map_versions (id, park_id, map_name, version, frame_id, crs, origin,
            resolution, floors, road_count, no_go_count, status, map_hash, map_width,
            map_height, note, published_at, created_at, source, source_id, file_name,
            file_path, file_size)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '1F', 0, 0, '部分标定', ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
         ON CONFLICT(id) DO UPDATE SET
            park_id = excluded.park_id,
            map_name = excluded.map_name,
            map_hash = excluded.map_hash,
            map_width = excluded.map_width,
            map_height = excluded.map_height,
            origin = excluded.origin,
            resolution = excluded.resolution,
            note = excluded.note,
            file_name = excluded.file_name,
            file_size = excluded.file_size",
    )
    .bind(&map_id)
    .bind(park_id)
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
    .execute(&state.db)
    .await?;

    // 可选：绑定为园区当前基线
    if request.bind {
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

    let map = crate::handlers::maps::fetch_map(&state, &map_id).await?;
    Ok(ApiResponse::ok(json!({
        "mapId": map.id,
        "mapName": map.map_name,
        "parkId": map.park_id,
        "zentaiMapId": zentai.id,
        "bound": request.bind,
        "stashPath": stash_path.display().to_string(),
        "map": map,
    })))
}
