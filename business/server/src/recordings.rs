//! 巡检录像：航点「录像采集」/ 航段「全程录像」的回传、播放与留存。
//!
//! 链路（录像本身在板端，平台只管存与看）：
//!
//! ```text
//! 路线动作「录像采集 / 全程录像」（可多选相机 `recordCameras`）
//!   → 设备客户端在 5009 状态机起停时机调板端 Agent（/api/record/*）
//!     → Agent 每一路一个 ffmpeg，从本机 mediamtx 拉流落 mp4（文件名带相机名）
//!       → 客户端 POST /api/recordings/ingest（本模块，multipart：file + meta）
//!         → 落盘 <recording_dir>/<设备>/<任务号>/…mp4 + 一条 task_recordings
//!           → 任务详情「任务录像」直接播放 / 下载（按相机区分）
//! ```
//!
//! 为什么不上中台「媒体资源」：那边是 AI 存证**训练图**（JPG/PNG、sha256 去重、
//! 打包导出），视频是任务执行结果，体积 / 留存 / 使用方式都不同。
//!
//! 约定：
//!
//! - **上传凭 `taskCode` 落地**：板端不知道任务自增 id，客户端只知道 `taskCode`；
//!   平台在这里补齐 `task_id` / `device_id` / `park_id`，顺带校验「设备确实是这条
//!   任务的执行设备」——否则任何一台在线设备都能往别人的任务里塞视频。
//! - **按 sha256 去重**：弱网重传同一段录像不新增行，也不重复占磁盘。
//! - **只回相对路径**：`media_path` 一类字段从不把服务器绝对目录发给前端。
//! - **Range 播放**：`GET /api/recordings/{id}/file` 支持 `bytes=`，浏览器边下边播。
//! - **播放凭 `?pt=` 短时令牌**：`<video src>` 带不上 `Authorization`，前端先调
//!   `GET /api/recordings/{id}/play` 换一个只对**这条录像**有效、10 分钟过期的签名
//!   令牌，再把它挂在 `?pt=` 上让浏览器自己按 Range 拉。
//!
//!   ⚠️ 以前是「先整段取成 Blob 再 `createObjectURL`」：37~64MB 的切片在 4G / 弱网下
//!   要几十秒到几分钟，界面只有一行「正在取回录像…」，axios 还有 120s 硬超时——
//!   现场报的「点回放没反应」大半是这一段（2026-11-24）。直连 Range 流是秒开，
//!   还能拖动进度条，也不占浏览器内存。
//! - **计划删了录像还能看**：可见性先按任务归属，任务已经不存在（计划被删、只剩录像
//!   行）就退到录像自己的园区，否则这些行会变成谁都打不开、也删不掉的孤儿。

use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use std::path::{Component, Path as FsPath, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use tracing::{info, warn};

/// 单个 mp4 上传上限。
///
/// 板端全程录像按 `RECORD_SEGMENT_SECS`（默认 300s）切片，1Mbps 下每段约 37MB；
/// 300MB 留足高码率与弱网重传的余量，同时挡住「误传整个目录」。
pub const RECORDING_MAX_BYTES: usize = 300 * 1024 * 1024;

/// 播放令牌有效期（秒）。够长到看完一段切片，短到链接被翻出来时已经失效。
pub const PLAY_TOKEN_TTL_SECS: i64 = 600;

/// 录像查询 / 播放的返回体（不暴露 `file_path` / `sha256`）。
#[derive(Debug, Clone, FromRow, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecording {
    pub id: i64,
    pub task_id: Option<i64>,
    pub task_code: String,
    pub device_code: String,
    pub plan_id: Option<String>,
    pub point_index: i64,
    pub point_name: Option<String>,
    /// `point`（航点录像）/ `task`（航段全程录像）
    pub scope: String,
    /// 录像源相机（`dogcam` / `cam0`~`cam3` / `dog-ipcam`）。
    ///
    /// 老客户端（板端还没升级多路录像）不传 `camera`，入库按 `dogcam` 算——
    /// 那时候板端只可能录默认源。
    pub camera: String,
    pub file_name: String,
    pub file_size: i64,
    pub duration_sec: f64,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub created_at: String,
}

/// 回落录像需要的任务口径（只查三列，别让 clippy 对四元组报警）
#[derive(Debug, FromRow)]
struct TaskBinding {
    id: i64,
    device_id: Option<i64>,
    park_id: Option<i64>,
}

#[derive(Debug, FromRow)]
struct RecordingPath {
    file_path: String,
    task_id: Option<i64>,
    park_id: Option<i64>,
}

/// `GET /api/recordings/{id}/play` 的回执：令牌 + 过期时间（秒级 epoch）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayLink {
    pub token: String,
    pub expires_at: i64,
}

/// 播放令牌的载荷：只认「哪条录像」+ 过期时间，不带用户身份。
#[derive(Debug, Serialize, Deserialize)]
struct PlayClaims {
    rid: i64,
    exp: usize,
}

#[derive(Debug, Deserialize)]
pub struct PlayQuery {
    /// `?pt=<播放令牌>`（`<video src>` 带不上 Bearer 时用）
    #[serde(default)]
    pt: Option<String>,
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
    scope: Option<String>,
    /// 录像源相机（`cameras` 多选时每路一条记录，各带自己的相机名）
    #[serde(default)]
    camera: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    ended_at: Option<String>,
    /// 客户端发的是 `durationSec`（板端边车同名）。
    ///
    /// ⚠️ 不能用 `rename_all = "camelCase"` 派生：`duration_s` 会变成 `durationS`，
    /// 与客户端对不上，于是入库永远 0（任务详情「时长」一直显示 `—`）。
    #[serde(default, rename = "durationSec", alias = "durationS")]
    duration_s: Option<f64>,
}

const RECORDING_SELECT: &str = "SELECT id, task_id, task_code, device_code, plan_id, point_index, \
     point_name, scope, camera, file_name, file_size, duration_s AS duration_sec, started_at, \
     ended_at, created_at FROM task_recordings";

/// `POST /api/recordings/ingest` — 设备客户端回传一段录像。
///
/// multipart 两个字段：
///
/// - `file`：`.mp4`（≤ [`RECORDING_MAX_BYTES`]）；
/// - `meta`：`{taskCode, deviceCode, planId, pointIndex, pointName, scope, camera, startedAt,
///   endedAt, durationSec}`（客户端从板端边车 `/api/record/list` 里拿）。
///
/// 回 `{ created, recording }`：`created=false` 表示这段之前传过（sha256 命中）。
pub async fn ingest_recording(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    let temp_dir = state.config.recording_dir.join(".incoming");
    tokio::fs::create_dir_all(&temp_dir)
        .await
        .map_err(|error| {
            AppError::Internal(format!(
                "创建录像暂存目录失败（{}）: {error}",
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
                let name = field.file_name().unwrap_or("recording.mp4").to_string();
                let next = stream_to_temp(&temp_dir, name, field).await?;
                // 同一请求里出现两个 file 字段时，把前一个临时文件收掉再换新的
                if let Some(previous) = uploaded.replace(next) {
                    let _ = tokio::fs::remove_file(&previous.path).await;
                }
            }
            Some("meta") | Some("mediaInfo") => {
                meta_text =
                    Some(field.text().await.map_err(|error| {
                        AppError::BadRequest(format!("读取录像信息失败: {error}"))
                    })?);
            }
            _ => {}
        }
    }

    let Some(mut file) = uploaded else {
        return Err(AppError::BadRequest("请上传录像文件".to_string()));
    };
    let meta_text = meta_text.ok_or_else(|| AppError::BadRequest("缺少 meta".to_string()))?;
    let meta: IngestMeta = serde_json::from_str(&meta_text)
        .map_err(|error| AppError::BadRequest(format!("录像信息格式错误: {error}")))?;

    let outcome = save_recording(&state, &meta, &mut file).await;
    if outcome.is_err() {
        // 任何失败都不留半截文件在 .incoming（磁盘压力主要来自这些残片）
        let _ = tokio::fs::remove_file(&file.path).await;
    }
    let (recording, created) = outcome?;
    Ok(ApiResponse::ok(serde_json::json!({
        "created": created,
        "recording": recording,
    })))
}

struct Uploaded {
    /// 本机临时文件（还在 `.incoming/` 里）
    path: PathBuf,
    /// 客户端给的文件名（只用于日志与扩展名判断）
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
    if !name.to_ascii_lowercase().ends_with(".mp4") {
        return Err(AppError::BadRequest("录像回传仅支持 .mp4".to_string()));
    }
    // 暂存名只要进程内唯一即可：时间戳到微秒 + 进程号（同一秒并发上传多段也不会撞）
    let path = temp_dir.join(format!(
        "{}-{}.mp4",
        std::process::id(),
        chrono::Local::now().format("%Y%m%d%H%M%S%6f")
    ));
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|error| AppError::Internal(format!("创建录像暂存文件失败: {error}")))?;
    let mut hasher = Sha256::new();
    let mut size = 0usize;
    // 文件头一起校验：mp4 的 `ftyp` 在第 4~8 字节，先攒够 12 字节再判
    let mut head = Vec::with_capacity(12);
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|error| AppError::BadRequest(format!("读取录像失败: {error}")))?
    {
        size += chunk.len();
        if size > RECORDING_MAX_BYTES {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(AppError::BadRequest(format!(
                "单段录像超过 {}MB 上限",
                RECORDING_MAX_BYTES / 1024 / 1024
            )));
        }
        if head.len() < 12 {
            let take = (12 - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..take]);
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| AppError::Internal(format!("写入录像失败: {error}")))?;
    }
    file.flush()
        .await
        .map_err(|error| AppError::Internal(format!("写入录像失败: {error}")))?;
    drop(file);
    if size == 0 {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(AppError::BadRequest("上传的录像为空".to_string()));
    }
    if !is_mp4(&head) {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(AppError::BadRequest(
            "文件内容不是有效的 MP4（缺少 ftyp 头）".to_string(),
        ));
    }
    Ok(Uploaded {
        path,
        name,
        size,
        sha256: hex_sha256(hasher.finalize().as_slice()),
    })
}

fn is_mp4(head: &[u8]) -> bool {
    head.len() >= 8 && &head[4..8] == b"ftyp"
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

async fn save_recording(
    state: &AppState,
    meta: &IngestMeta,
    file: &mut Uploaded,
) -> Result<(TaskRecording, bool), AppError> {
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

    // sha256 去重：同一段重传不新增行（客户端弱网重试是常态）
    if let Some(existing) =
        sqlx::query_as::<_, TaskRecording>(&format!("{RECORDING_SELECT} WHERE sha256 = ?1"))
            .bind(&file.sha256)
            .fetch_optional(&state.db)
            .await?
    {
        let _ = tokio::fs::remove_file(&file.path).await;
        return Ok((existing, false));
    }

    let scope = match meta.scope.as_deref().map(str::trim) {
        Some("task") => "task",
        _ => "point",
    };
    // 相机名只保留字母数字与 `-` `_`（`dog-ipcam` 本来就带连字符）；空就是默认源
    let camera = match meta.camera.as_deref().map(str::trim) {
        Some(camera) if !camera.is_empty() => safe_component(camera),
        _ => "dogcam".to_string(),
    };
    let point_index = meta.point_index.unwrap_or(-1).max(-1);
    let device_dir = safe_component(if device_code.is_empty() {
        "unknown"
    } else {
        device_code
    });
    let relative = format!(
        "{device_dir}/{}/{}-{}-{}.mp4",
        safe_component(task_code),
        now_stamp(),
        scope,
        &file.sha256[..8]
    );
    let absolute = state.config.recording_dir.join(&relative);
    if let Some(parent) = absolute.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| AppError::Internal(format!("创建录像目录失败: {error}")))?;
    }
    tokio::fs::rename(&file.path, &absolute)
        .await
        .map_err(|error| AppError::Internal(format!("移动录像文件失败: {error}")))?;

    let file_name = absolute
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("recording.mp4")
        .to_string();
    let duration = meta.duration_s.unwrap_or(0.0).max(0.0);
    let created_at = now();
    let result = sqlx::query(
        "INSERT INTO task_recordings (task_id, task_code, device_id, park_id, device_code, plan_id,
            point_index, point_name, scope, camera, file_name, file_path, file_size, duration_s,
            started_at, ended_at, sha256, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
    )
    .bind(task_id)
    .bind(task_code)
    .bind(task_device)
    .bind(park_id)
    .bind(device_code)
    .bind(&meta.plan_id)
    .bind(point_index)
    .bind(&meta.point_name)
    .bind(scope)
    .bind(&camera)
    .bind(&file_name)
    .bind(&relative)
    .bind(file.size as i64)
    .bind(duration)
    .bind(&meta.started_at)
    .bind(&meta.ended_at)
    .bind(&file.sha256)
    .bind(&created_at)
    .execute(&state.db)
    .await;
    if let Err(error) = result {
        // 落库失败就把刚移过去的文件收掉，别在盘上留孤儿
        let _ = tokio::fs::remove_file(&absolute).await;
        return Err(error.into());
    }

    let label = if scope == "task" {
        "全程录像".to_string()
    } else {
        meta.point_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("航点{}", point_index.max(0)))
    };
    info!(
        task = task_code,
        device = %device_code,
        scope,
        point = point_index,
        camera = %camera,
        size = file.size,
        file = %file.name,
        "巡检录像已入库（{label}）"
    );
    let _ = sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('recording', '录像回传', ?1, ?2, ?3, ?4)",
    )
    .bind(format!(
        "{label} · {camera} · {:.1}s · {:.1}MB",
        duration,
        file.size as f64 / 1024.0 / 1024.0
    ))
    .bind(task_device)
    .bind(task_id)
    .bind(now())
    .execute(&state.db)
    .await;

    let recording =
        sqlx::query_as::<_, TaskRecording>(&format!("{RECORDING_SELECT} WHERE sha256 = ?1"))
            .bind(&file.sha256)
            .fetch_one(&state.db)
            .await?;
    Ok((recording, true))
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

/// `GET /api/tasks/{id}/recordings` — 任务详情里的录像列表（新的在前）。
///
/// 计划被删、只剩录像行时也照常返回（按录像自己的园区鉴权），不然那些片子
/// 谁都看不到、也没入口删。
pub async fn list_task_recordings(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<TaskRecording>>>, AppError> {
    let task = match crate::handlers::tasks::fetch_task(&state, id).await {
        Ok(task) => Some(task),
        Err(AppError::NotFound(_)) => None,
        Err(error) => return Err(error),
    };
    let rows = match &task {
        Some(task) => {
            crate::handlers::parks::ensure_owner(task, &claims)?;
            sqlx::query_as::<_, TaskRecording>(&format!(
                "{RECORDING_SELECT} WHERE task_id = ?1 OR task_code = ?2 \
                 ORDER BY created_at DESC, id DESC LIMIT 200"
            ))
            .bind(id)
            .bind(&task.task_code)
            .fetch_all(&state.db)
            .await?
        }
        None => {
            if !claims.is_admin {
                ensure_park_owner(&state, &claims, orphan_park(&state, id).await?).await?;
            }
            sqlx::query_as::<_, TaskRecording>(&format!(
                "{RECORDING_SELECT} WHERE task_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 200"
            ))
            .bind(id)
            .fetch_all(&state.db)
            .await?
        }
    };
    Ok(ApiResponse::ok(rows))
}

/// `GET /api/recordings/{id}/play` — 换一个短时播放令牌。
///
/// `<video src>` 带不上 `Authorization`，所以先在这里（走 Bearer）验一次权限，
/// 再拿一个只对这条录像有效、[`PLAY_TOKEN_TTL_SECS`] 秒过期的令牌挂到 `?pt=` 上。
pub async fn play_link(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<Json<ApiResponse<PlayLink>>, AppError> {
    let row = fetch_recording_path(&state, id).await?;
    ensure_recording_visible(&state, &claims, row.task_id, row.park_id).await?;
    let expires_at =
        (chrono::Utc::now() + chrono::Duration::seconds(PLAY_TOKEN_TTL_SECS)).timestamp();
    let play_claims = PlayClaims {
        rid: id,
        exp: expires_at.max(0) as usize,
    };
    let token = encode(
        &Header::default(),
        &play_claims,
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|error| AppError::Internal(format!("生成播放凭证失败: {error}")))?;
    Ok(ApiResponse::ok(PlayLink { token, expires_at }))
}

/// `GET /api/recordings/{id}/file` — 播放 / 下载（支持 `Range`，浏览器边下边播）。
///
/// 鉴权两条路：正常请求带 Bearer；`<video src>` 这种带不上头的走 `?pt=` 播放令牌。
/// 令牌只证明「有权看这一段」，不携带用户身份——所以它换不到别的接口。
pub async fn download_recording(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<PlayQuery>,
    claims: Option<axum::Extension<Claims>>,
) -> Result<Response, AppError> {
    let path = fetch_recording_path(&state, id).await?;
    // 这条路由在 public 路由器里（`<video src>` 带不上头），所以 Bearer 要自己解；
    // 带 Bearer 的请求（老前端 `getBlob` / 下载工具）与 `?pt=` 令牌二选一。
    let claims = match claims {
        Some(axum::Extension(claims)) => Some(claims),
        None if headers.contains_key(header::AUTHORIZATION) => {
            Some(crate::auth::claims_from_header(&state, &headers)?)
        }
        None => None,
    };
    match claims {
        Some(claims) => {
            ensure_recording_visible(&state, &claims, path.task_id, path.park_id).await?;
        }
        None => {
            let token = query.pt.as_deref().unwrap_or_default();
            verify_play_token(&state.config.jwt_secret, id, token)?;
        }
    }
    let absolute = resolve_path(&state.config.recording_dir, &path.file_path)?;
    let meta = tokio::fs::metadata(&absolute).await.map_err(|error| {
        AppError::NotFound(format!("录像文件不存在（{}）: {error}", absolute.display()))
    })?;
    let total = meta.len();
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_range(value, total));

    let (start, end, status) = match range {
        Some((start, end)) => (start, end, StatusCode::PARTIAL_CONTENT),
        None => (0, total.saturating_sub(1), StatusCode::OK),
    };
    let length = if total == 0 { 0 } else { end - start + 1 };

    let mut file = tokio::fs::File::open(&absolute)
        .await
        .map_err(|error| AppError::Internal(format!("打开录像失败: {error}")))?;
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|error| AppError::Internal(format!("定位录像区间失败: {error}")))?;
    }
    let stream = ReaderStream::new(file.take(length));

    let mut response = Body::from_stream(stream).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap_or(HeaderValue::from_static("0")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("inline; filename=\"recording-{id}.mp4\""))
            .unwrap_or(HeaderValue::from_static("inline")),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {start}-{end}/{total}"))
                .unwrap_or(HeaderValue::from_static("bytes 0-0/0")),
        );
    }
    *response.status_mut() = status;
    // 回放卡住时的第一手证据：哪条录像、多大、多久（有这条日志才能区分「没点进来」
    // 「浏览器在拉」和「服务端没响应」）。Range 请求会连着来几条，量级可以接受。
    info!(
        recording = id,
        bytes = length,
        total,
        ranged = range.is_some(),
        "录像播放"
    );
    Ok(response)
}

/// 查一段录像的落盘信息（播放 / 下载 / 删除 / 列表兜底共用）。
async fn fetch_recording_path(state: &AppState, id: i64) -> Result<RecordingPath, AppError> {
    sqlx::query_as::<_, RecordingPath>(
        "SELECT file_path, task_id, park_id FROM task_recordings WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("录像不存在".to_string()))
}

/// 录像可见性：**先按任务归属**（老口径 `ensure_owner`），任务已不存在（计划被删、
/// 只剩录像行）就退到**录像自己的园区**。
///
/// 为什么不能只按任务判：计划一删，`task_recordings.task_id` 就指向一个不存在的行，
/// 播放 / 删除全部变成 404「任务不存在」，录像既看不到也删不掉（2026-11-24 现场：
/// `TK-1124-17` 删掉后 6 段录像全成了孤儿）。
async fn ensure_recording_visible(
    state: &AppState,
    claims: &Claims,
    task_id: Option<i64>,
    park_id: Option<i64>,
) -> Result<(), AppError> {
    if claims.is_admin {
        return Ok(());
    }
    if let Some(task_id) = task_id {
        match crate::handlers::tasks::fetch_task(state, task_id).await {
            Ok(task) => return crate::handlers::parks::ensure_owner(&task, claims),
            // 计划已删：走园区兜底，别把孤儿录像锁死
            Err(AppError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
    }
    let Some(park_id) = park_id else {
        return Err(AppError::NotFound("录像不存在".to_string()));
    };
    ensure_park_owner(state, claims, park_id).await
}

/// 园区归属判定（录像 / 照片这类「任务结果」在计划删掉之后的兜底口径）。
async fn ensure_park_owner(
    state: &AppState,
    claims: &Claims,
    park_id: i64,
) -> Result<(), AppError> {
    if claims.is_admin {
        return Ok(());
    }
    let created_by: Option<i64> = sqlx::query_scalar("SELECT created_by FROM parks WHERE id = ?1")
        .bind(park_id)
        .fetch_optional(&state.db)
        .await?
        .flatten();
    match created_by {
        Some(uid) if uid == claims.sub => Ok(()),
        _ => Err(AppError::NotFound("录像不存在".to_string())),
    }
}

/// 孤儿录像的园区：按任务 id 反查第一条录像的 `park_id`（没有录像时直接 404）。
async fn orphan_park(state: &AppState, task_id: i64) -> Result<i64, AppError> {
    let park_id: Option<i64> =
        sqlx::query_scalar("SELECT park_id FROM task_recordings WHERE task_id = ?1 LIMIT 1")
            .bind(task_id)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    park_id.ok_or_else(|| AppError::NotFound("任务不存在".to_string()))
}

/// 校验 `?pt=` 播放令牌：签名 / 过期时间交给 `jsonwebtoken` 验，这里再钉住 `rid`，
/// 免得拿 A 段的令牌去放 B 段。
fn verify_play_token(secret: &str, id: i64, token: &str) -> Result<(), AppError> {
    let invalid = || AppError::Unauthorized("录像播放凭证无效或已过期".to_string());
    if token.is_empty() {
        return Err(AppError::Unauthorized("请先登录".to_string()));
    }
    let data = decode::<PlayClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| invalid())?;
    if data.claims.rid != id {
        return Err(invalid());
    }
    Ok(())
}

/// 解析 `Range: bytes=start-end`（只认单区间；越界返回 None 走整文件）。
fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    if total == 0 {
        return None;
    }
    let (start, end) = if start.trim().is_empty() {
        // `bytes=-500`：最后 500 字节
        let suffix: u64 = end.trim().parse().ok()?;
        (total.saturating_sub(suffix), total.saturating_sub(1))
    } else {
        let start: u64 = start.trim().parse().ok()?;
        let end = if end.trim().is_empty() {
            total.saturating_sub(1)
        } else {
            end.trim().parse::<u64>().ok()?.min(total.saturating_sub(1))
        };
        (start, end)
    };
    (start <= end && start < total).then_some((start, end))
}

/// `DELETE /api/recordings/{id}` — 删一条录像（文件 + 行一起收，盘上不留孤儿）。
pub async fn delete_recording(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let path = fetch_recording_path(&state, id).await?;
    ensure_recording_visible(&state, &claims, path.task_id, path.park_id).await?;
    if let Ok(absolute) = resolve_path(&state.config.recording_dir, &path.file_path)
        && let Err(error) = tokio::fs::remove_file(&absolute).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        warn!("删除录像文件失败（{}）: {error}", absolute.display());
    }
    sqlx::query("DELETE FROM task_recordings WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(true))
}

/// 相对路径 → 绝对路径；只认 `recording_dir` 下面、且不含 `..` 的路径。
fn resolve_path(root: &FsPath, relative: &str) -> Result<PathBuf, AppError> {
    let relative = PathBuf::from(relative.trim());
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(AppError::BadRequest("录像路径不合法".to_string()));
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
        _ => Err(AppError::BadRequest("录像路径越界".to_string())),
    }
}

/// 超期录像清理（保留 `config.recording_keep_days` 天；`0` = 不清理）。
///
/// 由 `schedule::sweep` 每分钟顺手调一次：查不到超期行时只有一条 COUNT 级的开销。
pub async fn sweep_expired(state: &AppState) -> Result<usize, AppError> {
    purge_incoming(state).await;
    let keep_days = state.config.recording_keep_days;
    if keep_days <= 0 {
        return Ok(0);
    }
    let cutoff = (chrono::Local::now() - chrono::Duration::days(keep_days))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, file_path FROM task_recordings WHERE created_at < ?1")
            .bind(&cutoff)
            .fetch_all(&state.db)
            .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let mut removed = 0usize;
    for (id, relative) in &rows {
        if let Ok(absolute) = resolve_path(&state.config.recording_dir, relative) {
            let _ = tokio::fs::remove_file(&absolute).await;
            // 任务目录空了就一起收掉：一天几十段 × 几十条任务，空目录会越攒越多
            if let Some(parent) = absolute.parent() {
                let _ = tokio::fs::remove_dir(parent).await; // 非空时失败，忽略
            }
        }
        sqlx::query("DELETE FROM task_recordings WHERE id = ?1")
            .bind(id)
            .execute(&state.db)
            .await?;
        removed += 1;
    }
    info!(removed, keep_days, "巡检录像超期清理完成");
    Ok(removed)
}

/// 清 `.incoming/` 里超过 1 小时的残片。
///
/// 上传中途断网 / 进程重启时，临时文件不会走到 `remove_file`（handler 没机会跑完），
/// 大文件攒起来很快；它们不跨请求复用，放着只会吃磁盘。
async fn purge_incoming(state: &AppState) {
    const STALE_SECS: u64 = 3600;
    let incoming = state.config.recording_dir.join(".incoming");
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
