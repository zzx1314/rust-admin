//! 航点拍照（路线动作「可见光拍照」）的回传、查看与留存。
//!
//! 链路（照片在板端拍，平台只管存与看）：
//!
//! ```text
//! 路线动作「可见光拍照」（photoCamera / photoFrames）
//!   → 设备客户端在 5009 到点那一帧调板端 Agent（POST /api/photo）
//!     → Agent 用 ffmpeg 从本机 mediamtx 抓 JPEG（dogcam / cam0..cam3）
//!       → 任务终态 / 客户端启动：GET /api/photo/list → POST /api/photos/ingest
//!         （本模块，multipart：file + meta）
//!         → 落盘 <photo_dir>/<设备>/<任务号>/pt<n>/…jpg + 一条 task_photos
//!           → 上传成功才回调 /api/photo/delete 删板端文件
//!             → 任务详情「任务照片」直接看 / 下载
//! ```
//!
//! 为什么照片来自**背包相机**（dogcam = 机器狗前置可见光，cam0~3 = 背包四路 USB）：
//!
//! - 本体侧「任务里的 `1` = 拍照」走的是上装 `/capture_image`，服务未就绪时
//!   `14005` 直接失败（见 `docs/official-app-parity.md` §5.5 与
//!   `zhpro/route.rs::capture_hint`），而且本体没实现 5010/5011 记录查询；
//! - 背包相机本来就在跑（实时视频 / AI 抽帧同一路），抓一张图不依赖任何新协议，
//!   现场立刻能看到、能复核。本体侧的 `tasks=[1]` 行为保持不动。
//!
//! 约定（与 `recordings.rs` 同一套，改之前先对照看）：
//!
//! - **上传凭 `taskCode` 落地**：板端不知道任务自增 id，客户端只知道 `taskCode`；
//!   平台在这里补齐 `task_id` / `device_id` / `park_id`，并校验「设备确实是这条任务的
//!   执行设备」——否则任何一台在线设备都能往别人的任务里塞图。
//! - **按 sha256 去重**：弱网重传同一张图不新增行，也不重复占磁盘。
//! - **只回相对路径**：接口从不把服务器绝对目录发给前端。
//! - **前端按 blob 取图**：`GET /api/photos/{id}/file` 要 Bearer，`<img src>` 带不上头，
//!   所以页面先取 Blob 再 `URL.createObjectURL`（与录像完全一致）。

use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::path::{Component, Path as FsPath, PathBuf};
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

/// 单张照片上传上限。
///
/// 1080P JPEG（`-q:v 2`）约 300~600KB；16MB 留出高分辨率/多帧误传的余量，
/// 又不能让一次误传把磁盘写满。
pub const PHOTO_MAX_BYTES: usize = 16 * 1024 * 1024;

/// 任务详情的照片上限（一次巡检几十张很常见，但接口别无限往回吐）。
pub const PHOTO_LIST_LIMIT: i64 = 200;

/// 照片查询 / 下载的返回体（不暴露 `file_path` / `sha256`）。
#[derive(Debug, Clone, FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPhoto {
    pub id: i64,
    pub task_id: Option<i64>,
    pub task_code: String,
    pub device_code: String,
    pub plan_id: Option<String>,
    pub point_index: i64,
    pub point_name: Option<String>,
    /// 抓拍源：`dogcam` / `cam0`~`cam3`
    pub camera: String,
    pub file_name: String,
    pub file_size: i64,
    pub captured_at: Option<String>,
    pub created_at: String,
}

/// 回落照片需要的任务口径
#[derive(Debug, FromRow)]
struct TaskBinding {
    id: i64,
    device_id: Option<i64>,
    park_id: Option<i64>,
}

#[derive(Debug, FromRow)]
struct PhotoPath {
    file_path: String,
    task_id: Option<i64>,
}

/// 客户端回传的元数据（`meta` 字段，JSON 字符串）。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct IngestMeta {
    task_code: String,
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    plan_id: Option<String>,
    #[serde(default)]
    point_index: Option<i64>,
    #[serde(default)]
    point_name: Option<String>,
    #[serde(default)]
    camera: Option<String>,
    /// 板端抓拍时刻（板子本地时间）
    #[serde(default)]
    captured_at: Option<String>,
}

const PHOTO_SELECT: &str = "SELECT id, task_id, task_code, device_code, plan_id, point_index, \
     point_name, camera, file_name, file_size, captured_at, created_at FROM task_photos";

/// `POST /api/photos/ingest` — 设备客户端回传一张航点照片。
///
/// multipart 两个字段：
///
/// - `file`：`.jpg` / `.jpeg`（≤ [`PHOTO_MAX_BYTES`]）；
/// - `meta`：`{taskCode, deviceCode, planId, pointIndex, pointName, camera, capturedAt}`
///   （客户端从板端 `/api/photo/list` 里拿）。
///
/// 回 `{ created, photo }`：`created=false` 表示这张之前传过（sha256 命中）。
pub async fn ingest_photo(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let temp_dir = state.config.photo_dir.join(".incoming");
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .map_err(|error| {
            AppError::Internal(format!(
                "创建照片暂存目录失败（{}）: {error}",
                temp_dir.display()
            ))
        })?;

    let mut meta_text: Option<String> = None;
    let mut uploaded: Option<Uploaded> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::BadRequest(format!("无效的上传请求: {error}")))?
    {
        match field.name() {
            Some("file") => {
                let name = field.file_name().unwrap_or("photo.jpg").to_string();
                let next = stream_to_temp(&temp_dir, name, field).await?;
                if let Some(previous) = uploaded.replace(next) {
                    let _ = tokio::fs::remove_file(&previous.path).await;
                }
            }
            Some("meta") | Some("mediaInfo") => {
                meta_text =
                    Some(field.text().await.map_err(|error| {
                        AppError::BadRequest(format!("读取照片信息失败: {error}"))
                    })?);
            }
            _ => {}
        }
    }

    let Some(mut file) = uploaded else {
        return Err(AppError::BadRequest("请上传照片文件".to_string()));
    };
    let meta_text = meta_text.ok_or_else(|| AppError::BadRequest("缺少 meta".to_string()))?;
    let meta: IngestMeta = serde_json::from_str(&meta_text)
        .map_err(|error| AppError::BadRequest(format!("照片信息格式错误: {error}")))?;

    let outcome = save_photo(&state, &meta, &mut file).await;
    if outcome.is_err() {
        let _ = tokio::fs::remove_file(&file.path).await;
    }
    let (photo, created) = outcome?;
    Ok(ApiResponse::ok(serde_json::json!({
        "created": created,
        "photo": photo,
    })))
}

struct Uploaded {
    path: PathBuf,
    name: String,
    size: usize,
    sha256: String,
}

/// 把 multipart 的文件字段流式写到 `.incoming/`，边写边算 sha256，超限立刻中止。
async fn stream_to_temp(
    temp_dir: &FsPath,
    name: String,
    mut field: axum::extract::multipart::Field<'_>,
) -> Result<Uploaded, AppError> {
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".jpg") && !lower.ends_with(".jpeg") {
        return Err(AppError::BadRequest(
            "照片回传仅支持 .jpg / .jpeg".to_string(),
        ));
    }
    let path = temp_dir.join(format!(
        "{}-{}.jpg",
        std::process::id(),
        chrono::Local::now().format("%Y%m%d%H%M%S%6f")
    ));
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|error| AppError::Internal(format!("创建照片暂存文件失败: {error}")))?;
    let mut hasher = Sha256::new();
    let mut size = 0usize;
    // 文件头一起校验：JPEG 的 SOI 是 `FF D8 FF`，先攒够 3 字节再判
    let mut head = Vec::with_capacity(3);
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|error| AppError::BadRequest(format!("读取照片失败: {error}")))?
    {
        size += chunk.len();
        if size > PHOTO_MAX_BYTES {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(AppError::BadRequest(format!(
                "单张照片超过 {}MB 上限",
                PHOTO_MAX_BYTES / 1024 / 1024
            )));
        }
        if head.len() < 3 {
            let take = (3 - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| AppError::Internal(format!("写入照片失败: {error}")))?;
    }
    file.flush()
        .await
        .map_err(|error| AppError::Internal(format!("写入照片失败: {error}")))?;
    drop(file);
    if size == 0 {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(AppError::BadRequest("上传的照片为空".to_string()));
    }
    if !is_jpeg(&head) {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(AppError::BadRequest(
            "文件内容不是有效的 JPEG（缺少 SOI 头）".to_string(),
        ));
    }
    Ok(Uploaded {
        path,
        name,
        size,
        sha256: hex_sha256(hasher.finalize().as_slice()),
    })
}

fn is_jpeg(head: &[u8]) -> bool {
    head.len() >= 3 && head[0] == 0xFF && head[1] == 0xD8 && head[2] == 0xFF
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

async fn save_photo(
    state: &AppState,
    meta: &IngestMeta,
    file: &mut Uploaded,
) -> Result<(TaskPhoto, bool), AppError> {
    let task_code = meta.task_code.trim();
    if task_code.is_empty() {
        return Err(AppError::BadRequest("meta.taskCode 不能为空".to_string()));
    }
    let device_code = meta.device_code.trim();

    // 任务 + 设备口径：任务号必须存在，且（设了设备的）必须是这条任务的执行设备
    let task: Option<TaskBinding> =
        sqlx::query_as("SELECT id, device_id, park_id FROM tasks WHERE task_code = ?1")
            .bind(task_code)
            .fetch_optional(&state.db)
            .await?;
    let TaskBinding {
        id: task_id,
        device_id: task_device,
        park_id,
    } = task.ok_or_else(|| AppError::NotFound(format!("任务不存在: {task_code}")))?;
    if !device_code.is_empty()
        && let Some(task_device) = task_device
    {
        let actual: Option<(String,)> =
            sqlx::query_as("SELECT device_id FROM devices WHERE id = ?1")
                .bind(task_device)
                .fetch_optional(&state.db)
                .await?;
        if let Some((actual,)) = actual
            && actual != device_code
        {
            return Err(AppError::BadRequest(format!(
                "设备 {device_code} 不是任务 {task_code} 的执行设备（应为 {actual}）"
            )));
        }
    }

    // sha256 去重：同一张重传不新增行
    if let Some(existing) =
        sqlx::query_as::<_, TaskPhoto>(&format!("{PHOTO_SELECT} WHERE sha256 = ?1"))
            .bind(&file.sha256)
            .fetch_optional(&state.db)
            .await?
    {
        let _ = tokio::fs::remove_file(&file.path).await;
        return Ok((existing, false));
    }

    let point_index = meta.point_index.unwrap_or(-1).max(-1);
    let camera = match meta.camera.as_deref().map(str::trim) {
        Some(camera) if !camera.is_empty() => safe_component(camera),
        _ => "dogcam".to_string(),
    };
    let device_dir = safe_component(if device_code.is_empty() {
        "unknown"
    } else {
        device_code
    });
    // 目录按「设备 / 任务 / 航点」分层：现场找图、批量下载都靠它
    let relative = format!(
        "{device_dir}/{}/pt{}/{}-{}-{}.jpg",
        safe_component(task_code),
        point_index.max(0),
        now_stamp(),
        camera,
        &file.sha256[..8]
    );
    let absolute = state.config.photo_dir.join(&relative);
    if let Some(parent) = absolute.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| AppError::Internal(format!("创建照片目录失败: {error}")))?;
    }
    tokio::fs::rename(&file.path, &absolute)
        .await
        .map_err(|error| AppError::Internal(format!("移动照片文件失败: {error}")))?;

    let file_name = absolute
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("photo.jpg")
        .to_string();
    let created_at = now();
    let result = sqlx::query(
        "INSERT INTO task_photos (task_id, task_code, device_id, park_id, device_code, plan_id,
            point_index, point_name, camera, file_name, file_path, file_size, captured_at,
            sha256, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
    )
    .bind(task_id)
    .bind(task_code)
    .bind(task_device)
    .bind(park_id)
    .bind(device_code)
    .bind(&meta.plan_id)
    .bind(point_index)
    .bind(&meta.point_name)
    .bind(&camera)
    .bind(&file_name)
    .bind(&relative)
    .bind(file.size as i64)
    .bind(&meta.captured_at)
    .bind(&file.sha256)
    .bind(&created_at)
    .execute(&state.db)
    .await;
    if let Err(error) = result {
        // 落库失败就把刚移过去的文件收掉，别在盘上留孤儿
        let _ = tokio::fs::remove_file(&absolute).await;
        return Err(error.into());
    }

    let label = meta
        .point_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("航点{}", point_index.max(0)));
    info!(
        task = task_code,
        device = %device_code,
        point = point_index,
        camera = %camera,
        size = file.size,
        file = %file.name,
        "航点照片已入库（{label}）"
    );
    let _ = sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('photo', '照片回传', ?1, ?2, ?3, ?4)",
    )
    .bind(format!(
        "{label} · {camera} · {:.0}KB",
        file.size as f64 / 1024.0
    ))
    .bind(task_device)
    .bind(task_id)
    .bind(now())
    .execute(&state.db)
    .await;

    let photo = sqlx::query_as::<_, TaskPhoto>(&format!("{PHOTO_SELECT} WHERE sha256 = ?1"))
        .bind(&file.sha256)
        .fetch_one(&state.db)
        .await?;
    Ok((photo, true))
}

fn now_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// 目录名清洗：只留字母数字 + `-` `_`，其余换成 `_`（设备号 / 任务号里可能有汉字）
fn safe_component(value: &str) -> String {
    let cleaned: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned.chars().take(80).collect()
    }
}

/// `GET /api/tasks/{id}/photos` — 任务详情里的照片列表（新的在前）。
pub async fn list_task_photos(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<TaskPhoto>>>, AppError> {
    let task = crate::handlers::tasks::fetch_task(&state, id).await?;
    crate::handlers::parks::ensure_owner(&task, &claims)?;
    let rows = sqlx::query_as::<_, TaskPhoto>(&format!(
        "{PHOTO_SELECT} WHERE task_id = ?1 OR task_code = ?2 \
         ORDER BY point_index ASC, created_at DESC, id DESC LIMIT ?3"
    ))
    .bind(id)
    .bind(&task.task_code)
    .bind(PHOTO_LIST_LIMIT)
    .fetch_all(&state.db)
    .await?;
    Ok(ApiResponse::ok(rows))
}

/// `GET /api/photos/{id}/file?download=1` — 原图（前端取 Blob 后挂 `<img>` / 下载）。
#[derive(Debug, serde::Deserialize)]
pub struct DownloadQuery {
    /// `1` = `Content-Disposition: attachment`（直接点链接时走下载而不是浏览器内预览）
    #[serde(default)]
    download: Option<i64>,
}

pub async fn download_photo(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::extract::Query(query): axum::extract::Query<DownloadQuery>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<Response, AppError> {
    let path =
        sqlx::query_as::<_, PhotoPath>("SELECT file_path, task_id FROM task_photos WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound("照片不存在".to_string()))?;
    if let Some(task_id) = path.task_id {
        let task = crate::handlers::tasks::fetch_task(&state, task_id).await?;
        crate::handlers::parks::ensure_owner(&task, &claims)?;
    }
    let absolute = resolve_path(&state.config.photo_dir, &path.file_path)?;
    let bytes = tokio::fs::read(&absolute).await.map_err(|error| {
        AppError::NotFound(format!("照片文件不存在（{}）: {error}", absolute.display()))
    })?;
    let disposition = if query.download.unwrap_or(0) > 0 {
        "attachment"
    } else {
        "inline"
    };
    let mut response = Body::from(bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("{disposition}; filename=\"photo-{id}.jpg\""))
            .unwrap_or(HeaderValue::from_static("inline")),
    );
    *response.status_mut() = StatusCode::OK;
    Ok(response)
}

/// `DELETE /api/photos/{id}` — 删一张照片（文件 + 行一起收，盘上不留孤儿）。
pub async fn delete_photo(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let path =
        sqlx::query_as::<_, PhotoPath>("SELECT file_path, task_id FROM task_photos WHERE id = ?1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| AppError::NotFound("照片不存在".to_string()))?;
    if let Some(task_id) = path.task_id {
        let task = crate::handlers::tasks::fetch_task(&state, task_id).await?;
        crate::handlers::parks::ensure_owner(&task, &claims)?;
    }
    if let Ok(absolute) = resolve_path(&state.config.photo_dir, &path.file_path)
        && let Err(error) = tokio::fs::remove_file(&absolute).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warn!("删除照片文件失败（{}）: {error}", absolute.display());
    }
    sqlx::query("DELETE FROM task_photos WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(true))
}

/// 相对路径 → 绝对路径；只认 `photo_dir` 下面、且不含 `..` 的路径。
fn resolve_path(root: &FsPath, relative: &str) -> Result<PathBuf, AppError> {
    let relative = PathBuf::from(relative.trim());
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(AppError::BadRequest("照片路径不合法".to_string()));
    }
    let absolute = root.join(&relative);
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let parent = absolute.parent().map(|parent| {
        parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf())
    });
    match parent {
        Some(parent) if parent.starts_with(&root) => Ok(absolute),
        _ => Err(AppError::BadRequest("照片路径越界".to_string())),
    }
}

/// 超期照片清理（保留 `config.photo_keep_days` 天；`0` = 不清理）。
///
/// 由 `schedule::sweep` 每分钟顺手调一次：查不到超期行时只有一条 COUNT 级的开销。
pub async fn sweep_expired(state: &AppState) -> Result<usize, AppError> {
    purge_incoming(state).await;
    let keep_days = state.config.photo_keep_days;
    if keep_days <= 0 {
        return Ok(0);
    }
    let cutoff = (chrono::Local::now() - chrono::Duration::days(keep_days))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, file_path FROM task_photos WHERE created_at < ?1")
            .bind(&cutoff)
            .fetch_all(&state.db)
            .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let mut removed = 0usize;
    for (id, relative) in &rows {
        if let Ok(absolute) = resolve_path(&state.config.photo_dir, relative) {
            let _ = tokio::fs::remove_file(&absolute).await;
            if let Some(parent) = absolute.parent() {
                let _ = tokio::fs::remove_dir(parent).await; // 非空时失败，忽略
            }
        }
        sqlx::query("DELETE FROM task_photos WHERE id = ?1")
            .bind(id)
            .execute(&state.db)
            .await?;
        removed += 1;
    }
    info!(removed, keep_days, "航点照片超期清理完成");
    Ok(removed)
}

/// 清 `.incoming/` 里超过 1 小时的残片（上传中途断网 / 进程重启留下的）。
async fn purge_incoming(state: &AppState) {
    const STALE_SECS: u64 = 3600;
    let incoming = state.config.photo_dir.join(".incoming");
    let Ok(mut entries) = tokio::fs::read_dir(&incoming).await else {
        return;
    };
    let now = std::time::SystemTime::now();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let stale = entry
            .metadata()
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age.as_secs() > STALE_SECS);
        if stale {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}
