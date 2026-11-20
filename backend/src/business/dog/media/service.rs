use crate::business::dog::config::service::DogConfigService;
use crate::business::dog::media::domain::{
    CreateMediaRequest, DogMedia, ExportMediaQuery, MediaImageVO, MediaPageQuery, MediaVO,
    UpdateMediaRequest, UploadMediaRequest,
};
use crate::business::dog::media::repository::SeaOrmDogMediaRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::path::Path;

/// 板端 AI 存证根目录（中台参数配置 `dog_config.media_picture_dir` 的默认值）。
pub const DEFAULT_MEDIA_PICTURE_DIR: &str = "/media/media-data/picture";
/// 每个航点回传/保存的固定帧数默认值（`dog_config.media_frames`）。
pub const DEFAULT_MEDIA_FRAMES: u32 = 3;
/// 单张图片上限：板端 720p 画框 JPG 通常 200~500KB，8MB 足够且能挡住误传视频。
pub const MEDIA_IMAGE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// 批量下载（训练数据）单次最多几张：再多的请按计划/时间分批。
pub const MEDIA_EXPORT_MAX_FILES: i64 = 200;
/// 批量下载单次体积上限：中台是 2 核 3.6G 的小机器，打包在内存里做。
pub const MEDIA_EXPORT_MAX_BYTES: usize = 100 * 1024 * 1024;

/// 「参数配置」页里的两个键：板端存证目录与每航点固定帧数。
pub const MEDIA_CONFIG_DIR: &str = "media_picture_dir";
pub const MEDIA_CONFIG_FRAMES: &str = "media_frames";

/// 板端采集配置（中台参数配置 → 业务平台 → `ai_start` → 客户端 → 板端 Agent）。
///
/// 同一条配置同时决定「板端写哪里」和「同步媒体时读哪里」——只配一处，
/// 两边不会漂移。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCaptureConfig {
    pub picture_dir: String,
    pub frames: u32,
}

impl Default for MediaCaptureConfig {
    fn default() -> Self {
        Self {
            picture_dir: DEFAULT_MEDIA_PICTURE_DIR.to_string(),
            frames: DEFAULT_MEDIA_FRAMES,
        }
    }
}

/// 读参数配置页里的板端媒体设置；配置缺失 / 写坏（相对路径、`..`）时回退默认值，
/// 绝不因为一条配置把巡检 AI 卡住。
pub async fn load_capture_config(service: &DogConfigService) -> MediaCaptureConfig {
    let mut picture_dir: Option<String> = None;
    let mut frames: Option<u32> = None;
    if let Ok(configs) = service.list_all().await {
        for config in configs {
            match config.config_name.trim() {
                MEDIA_CONFIG_DIR => {
                    picture_dir = config.config_value.map(|value| value.trim().to_string());
                }
                MEDIA_CONFIG_FRAMES => {
                    frames = config
                        .config_value
                        .and_then(|value| value.trim().parse().ok());
                }
                _ => {}
            }
        }
    }
    let picture_dir = picture_dir
        .filter(|value| value.starts_with('/') && !value.contains("..") && value.len() <= 200)
        .unwrap_or_else(|| DEFAULT_MEDIA_PICTURE_DIR.to_string());
    MediaCaptureConfig {
        picture_dir,
        frames: frames.unwrap_or(DEFAULT_MEDIA_FRAMES).clamp(1, 10),
    }
}

pub struct DogMediaService {
    repo: SeaOrmDogMediaRepository,
}

impl DogMediaService {
    pub fn new(repo: SeaOrmDogMediaRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateMediaRequest) -> Result<DogMedia, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogMedia, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Media with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: MediaPageQuery,
    ) -> Result<PageResponse<MediaVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseError)?;

        Ok(PageResponse::new(
            records.into_iter().map(MediaVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self) -> Result<Vec<DogMedia>, AppError> {
        self.repo.find_all().await.map_err(AppError::DatabaseError)
    }

    pub async fn update(&self, id: i64, req: UpdateMediaRequest) -> Result<DogMedia, AppError> {
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Media with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseError)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Media with id {} not found",
                id
            )));
        }
        Ok(())
    }

    /// 落盘并登记设备回传的一张图片（`/api/dogInternal/media/upload`）。
    ///
    /// 返回 `(记录, 是否新建)`：同一张图（`sha256` 相同）重复回传时复用已有记录，
    /// 文件丢了会补写，**不会**在媒体列表里刷出重复行。
    ///
    /// 落盘路径：`<upload_dir>/media/<设备>/<计划>/pt<航点>/<sha256>.<ext>`，
    /// `media_path` 存**相对路径**（页面只展示相对路径，不泄漏服务器绝对目录）。
    pub async fn save_upload(
        &self,
        info: UploadMediaRequest,
        original_name: &str,
        bytes: &[u8],
        upload_root: &Path,
    ) -> Result<(DogMedia, bool), AppError> {
        let (file_name, extension) = validate_image(original_name, bytes)?;
        let sha256 = hex_sha256(bytes);

        let device_code = info
            .device_code
            .as_deref()
            .map(safe_component)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        let plan_id = info
            .plan_id
            .as_deref()
            .map(safe_component)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "manual".to_string());
        let point_index = info.point_index.unwrap_or_default().max(0);
        let captured_at = info
            .captured_at
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(beijing_now);

        let relative =
            format!("media/{device_code}/{plan_id}/pt{point_index}/{sha256}.{extension}");
        let absolute = upload_root.join(&relative);
        write_file(&absolute, bytes).await?;

        let point_label = info
            .point_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("航点{point_index}"));
        let media_name = info
            .media_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{device_code} {plan_id} {point_label} {captured_at}"));
        let source = info
            .source
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "ai-patrol".to_string());

        let mut remarks = String::from("设备回传：");
        remarks.push_str(&format!("{point_label} / {file_name}"));
        if let Some(extra) = info
            .remarks
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            remarks.push_str(" / ");
            remarks.push_str(extra);
        }

        let req = CreateMediaRequest {
            media_name,
            media_type: Some("图片".to_string()),
            media_path: Some(relative),
            media_size: Some(i64::try_from(bytes.len()).unwrap_or(i64::MAX)),
            remarks: Some(remarks),
            device_code: Some(device_code),
            park_code: info.park_code.clone(),
            plan_id: Some(plan_id),
            point_index: Some(point_index),
            captured_at: Some(captured_at),
            source: Some(source),
            sha256: Some(sha256.clone()),
        };

        // 去重：板端可能因为断电/重试把同一张图传两次。
        // `sha256` 上是**普通**唯一索引（迁移 32：正式机的 sqlite3 3.7 不认 partial
        // index），所以这里连软删除的行一起查：
        //   - 还在：不新建行，只保证文件在盘上；
        //   - 已删：同一张图再传回来就把那条记录恢复（否则插入会撞唯一索引）。
        if let Some(existing) = self
            .repo
            .find_by_sha256_any(&sha256)
            .await
            .map_err(AppError::DatabaseError)?
        {
            if existing.is_deleted != 0 {
                let revived = self
                    .repo
                    .revive(existing.id, &req)
                    .await
                    .map_err(AppError::DatabaseError)?;
                return Ok((revived, true));
            }
            if let Some(relative) = existing.media_path.as_deref()
                && !relative.trim().is_empty()
            {
                let absolute = upload_root.join(relative);
                if !absolute.exists() {
                    write_file(&absolute, bytes).await?;
                }
            }
            return Ok((existing, false));
        }

        let record = self
            .repo
            .create(&req, generate_id())
            .await
            .map_err(AppError::DatabaseError)?;
        Ok((record, true))
    }

    /// 读一张图片的原始字节（`GET /api/dogMedia/{id}/file`）。
    ///
    /// 返回 `(字节, 下载文件名, content-type)`。这些图是训练数据，页面只做下载、
    /// 不做逐张预览，所以直接给原文件。
    pub async fn load_file(
        &self,
        id: i64,
        upload_root: &Path,
    ) -> Result<(Vec<u8>, String, String), AppError> {
        let media = self.get_by_id(id).await?;
        let absolute = resolve_media_path(&media, upload_root)?;
        let bytes = tokio::fs::read(&absolute).await.map_err(|error| {
            AppError::NotFound(format!("媒体文件不存在（{}）: {error}", absolute.display()))
        })?;
        let content_type = content_type_of(&absolute).to_string();
        let extension = absolute
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("jpg")
            .to_ascii_lowercase();
        Ok((bytes, format!("media-{id}.{extension}"), content_type))
    }

    /// 单张预览（`GET /api/dogMedia/{id}/image`）：图片内容 + 元数据。
    ///
    /// 和地图预览同一套约定（JSON + base64 data URL），前端 `<img>` 直接用；
    /// 只做单张，批量训练数据仍走 [`Self::export_zip`]。
    pub async fn load_image(&self, id: i64, upload_root: &Path) -> Result<MediaImageVO, AppError> {
        let media = self.get_by_id(id).await?;
        let (bytes, _name, content_type) = self.load_file(id, upload_root).await?;
        if bytes.len() > MEDIA_IMAGE_MAX_BYTES {
            return Err(AppError::BadRequest(
                "媒体文件超过 8MB 上限，不做预览（可直接下载）".to_string(),
            ));
        }
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(MediaImageVO {
            id: media.id,
            media_name: media.media_name,
            content_type: content_type.clone(),
            size: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            image: format!("data:{content_type};base64,{encoded}"),
        })
    }

    /// 批量下载：把筛选到的图片打成一个 zip（`Stored`，JPEG 不再压缩）。
    ///
    /// 包内结构（给训练用，`manifest.json` 里是每张图的元数据）：
    ///
    /// ```text
    /// media-export-20261120-131500.zip
    /// ├── 0001_<设备>_<计划>_pt<航点>_<媒体id>.jpg
    /// ├── …
    /// └── manifest.json      # id / 计划 / 航点 / 采集时间 / 原始文件名 / 包内文件名
    /// ```
    ///
    /// 文件在盘上丢了的行不会让整包失败：写进 manifest 的 `error` 字段，
    /// 并计入 `skipped`（页面提示「跳过 N 条」）。
    pub async fn export_zip(
        &self,
        query: ExportMediaQuery,
        upload_root: &Path,
    ) -> Result<MediaExport, AppError> {
        let ids = parse_ids(query.ids.as_deref())?;
        let limit = if ids.is_empty() {
            query.size.unwrap_or(MEDIA_EXPORT_MAX_FILES)
        } else {
            ids.len() as i64
        }
        .clamp(1, MEDIA_EXPORT_MAX_FILES);
        let page = MediaPageQuery {
            media_name: query.media_name.clone(),
            media_type: query.media_type.clone(),
            device_code: query.device_code.clone(),
            plan_id: query.plan_id.clone(),
            source: query.source.clone(),
            begin_time: query.begin_time.clone(),
            end_time: query.end_time.clone(),
            current: 1,
            size: limit,
        };
        let rows = self
            .repo
            .find_for_export(&page, &ids, limit)
            .await
            .map_err(AppError::DatabaseError)?;
        if rows.is_empty() {
            return Err(AppError::NotFound(
                "没有符合条件的媒体文件（检查筛选条件或勾选）".to_string(),
            ));
        }

        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);
        let mut manifest = Vec::with_capacity(rows.len());
        let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut total = 0usize;
        let mut written = 0usize;
        let mut skipped = 0usize;

        for row in rows {
            let entry = |file: Option<&str>, error: Option<String>| {
                let mut item = serde_json::json!({
                    "id": row.id,
                    "mediaName": row.media_name,
                    "deviceCode": row.device_code,
                    "parkCode": row.park_code,
                    "planId": row.plan_id,
                    "pointIndex": row.point_index,
                    "capturedAt": row.captured_at,
                    "source": row.source,
                    "remarks": row.remarks,
                });
                if let Some(file) = file {
                    item["file"] = serde_json::json!(file);
                }
                if let Some(error) = error {
                    item["error"] = serde_json::json!(error);
                }
                item
            };
            let absolute = match resolve_media_path(&row, upload_root) {
                Ok(path) => path,
                Err(error) => {
                    skipped += 1;
                    manifest.push(entry(None, Some(error.to_string())));
                    continue;
                }
            };
            let bytes = match tokio::fs::read(&absolute).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    skipped += 1;
                    manifest.push(entry(None, Some(format!("文件读取失败: {error}"))));
                    continue;
                }
            };
            total = total.saturating_add(bytes.len());
            if total > MEDIA_EXPORT_MAX_BYTES {
                return Err(AppError::BadRequest(format!(
                    "导出体积超过 {}MB 上限，请按计划或时间范围分批下载",
                    MEDIA_EXPORT_MAX_BYTES / 1024 / 1024
                )));
            }
            let name = unique_entry_name(&row, &absolute, &mut used);
            writer
                .start_file(name.clone(), options)
                .map_err(|error| AppError::InternalError(format!("打包失败: {error}")))?;
            writer
                .write_all(&bytes)
                .map_err(|error| AppError::InternalError(format!("打包失败: {error}")))?;
            written += 1;
            manifest.push(entry(Some(&name), None));
        }

        let manifest_bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "exportedAt": beijing_now(),
            "count": written,
            "skipped": skipped,
            "items": manifest,
        }))
        .unwrap_or_default();
        writer
            .start_file("manifest.json", options)
            .map_err(|error| AppError::InternalError(format!("打包失败: {error}")))?;
        writer
            .write_all(&manifest_bytes)
            .map_err(|error| AppError::InternalError(format!("打包失败: {error}")))?;
        let cursor = writer
            .finish()
            .map_err(|error| AppError::InternalError(format!("打包失败: {error}")))?;

        let stamp = chrono::Utc::now()
            .with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset"))
            .format("%Y%m%d-%H%M%S");
        Ok(MediaExport {
            bytes: cursor.into_inner(),
            file_name: format!("media-export-{stamp}.zip"),
            count: written,
            skipped,
        })
    }
}

/// 批量下载结果
pub struct MediaExport {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub count: usize,
    pub skipped: usize,
}

/// `ids=1,2,3` → `[1,2,3]`；非法/超量直接拒绝（避免拼出超长 SQL）。
fn parse_ids(raw: Option<&str>) -> Result<Vec<i64>, AppError> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let id = part.parse::<i64>().map_err(|_| {
            AppError::BadRequest(format!("ids 里出现非法 ID：{part}（应为逗号分隔的数字）"))
        })?;
        ids.push(id);
    }
    if ids.len() as i64 > MEDIA_EXPORT_MAX_FILES {
        return Err(AppError::BadRequest(format!(
            "一次最多导出 {MEDIA_EXPORT_MAX_FILES} 张，请分批或改用筛选条件"
        )));
    }
    Ok(ids)
}

/// `media_path` → 上传目录下的绝对路径；只认自己写下的相对路径。
fn resolve_media_path(
    media: &DogMedia,
    upload_root: &Path,
) -> Result<std::path::PathBuf, AppError> {
    let relative = media
        .media_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::NotFound(format!(
                "Media {} 没有文件路径（手工登记的记录，磁盘上没有文件）",
                media.id
            ))
        })?;
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(AppError::BadRequest(
            "媒体文件路径不合法（必须是上传目录下的相对路径）".to_string(),
        ));
    }
    Ok(upload_root.join(relative_path))
}

/// 包内文件名：`0001_<设备>_<计划>_pt<航点>_<id>.<ext>`，全 ASCII（训练脚本好处理）。
fn unique_entry_name(
    media: &DogMedia,
    absolute: &Path,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let device = safe_ascii(media.device_code.as_deref().unwrap_or("device"));
    let plan = safe_ascii(media.plan_id.as_deref().unwrap_or("plan"));
    let point = media.point_index.unwrap_or_default();
    let extension = absolute
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("jpg")
        .to_ascii_lowercase();
    let base = format!("{device}_{plan}_pt{point}_{}", media.id);
    let mut name = format!("{base}.{extension}");
    let mut seq = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}-{seq}.{extension}");
        seq += 1;
    }
    name
}

/// 只保留 ASCII 字母数字与 `-` `_`（其余替换成 `_`，截到 40 字符）。
fn safe_ascii(raw: &str) -> String {
    let value: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    if value.trim_matches('_').is_empty() {
        "item".to_string()
    } else {
        value
    }
}

fn generate_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn beijing_now() -> String {
    let beijing = chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset");
    chrono::Utc::now()
        .with_timezone(&beijing)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 路径片段白名单：字母数字、`-`、`_`、汉字；其余（含 `/`、`..`）替换成 `_`。
///
/// 与板端 `evidence._safe_job_id` 保持同一套规则，`plan_id` / 设备号会直接
/// 拼进落盘路径，这里不做过滤就是路径穿越。
fn safe_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') || (ch as u32) > 0x7f {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out.trim_matches('_').chars().take(80).collect()
}

fn content_type_of(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        _ => "image/jpeg",
    }
}

/// 校验文件名/字节，返回（清理后的文件名, 扩展名, content-type）。
///
/// 只允许 JPG/PNG，并且按文件头复核：客户端传什么 `mime` 都不可信。
fn validate_image(original_name: &str, bytes: &[u8]) -> Result<(String, String), AppError> {
    if bytes.is_empty() {
        return Err(AppError::BadRequest("上传的图片为空".to_string()));
    }
    if bytes.len() > MEDIA_IMAGE_MAX_BYTES {
        return Err(AppError::BadRequest("图片超过 8MB 上限".to_string()));
    }
    let file_name = Path::new(original_name)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "frame.jpg".to_string());
    let declared = Path::new(&file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let extension = match declared.as_str() {
        "jpg" | "jpeg" => "jpg",
        "png" => "png",
        other => {
            return Err(AppError::BadRequest(format!(
                "媒体仅支持 JPG/PNG（收到 {other:?}）"
            )));
        }
    };
    let magic_ok = if extension == "png" {
        bytes.starts_with(&[0x89, b'P', b'N', b'G'])
    } else {
        bytes.starts_with(&[0xFF, 0xD8, 0xFF])
    };
    if !magic_ok {
        return Err(AppError::BadRequest(
            "文件内容与扩展名不符（不是有效的 JPG/PNG）".to_string(),
        ));
    }
    Ok((file_name, extension.to_string()))
}

async fn write_file(absolute: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = absolute.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            AppError::InternalError(format!("创建媒体目录失败（{}）: {error}", parent.display()))
        })?;
    }
    tokio::fs::write(absolute, bytes).await.map_err(|error| {
        AppError::InternalError(format!(
            "保存媒体文件失败（{}）: {error}",
            absolute.display()
        ))
    })
}
