//! 板端 AI 存证图片回传（中台「同步媒体」指令）。
//!
//! 为什么放在客户端：中台/业务平台都读不到板子的磁盘，`/media/media-data/picture`
//! 只有本进程（与板端 Agent 同机）能读；中台凭证也只留在业务平台，不下发到机器狗。
//!
//! 链路：
//!
//! ```text
//! 中台「同步媒体」按钮（参数配置里的 media_picture_dir / media_frames）
//!   → 业务平台 push_command（原样透传 params）
//!     → 设备 WS：command = 同步媒体
//!       → 本模块扫 /media/media-data/picture/<planId>/pt<n>/*.jpg
//!         → 每 BATCH_MAX_FILES 张打成一个 zip（Stored，不重复压缩 JPEG）
//!           → POST {业务平台}/api/media/ingest（file=zip + mediaInfo.entries）
//!             → 业务平台转发中台 /api/dogInternal/media/upload
//!               → 中台解包，逐张入库 + 落盘（一张一条 `dog_media`）
//!                 → 中台「媒体资源」页展示
//! ```
//!
//! 为什么打 zip：一轮巡检几十张，按张请求在 4G 上握手开销占比太高；打包后一次
//! 握手传 8 张。**入库粒度仍是一张一条**（媒体页要逐张预览），所以中台返回
//! `results[]`，客户端**只删返回 `ok` 的那几张**，失败的留在板端下次重试。
//!
//! 板端文件**上传成功才删**（中台按 sha256 去重，`created=false` 也算成功），
//! 删完把空的 `pt<n>/`、计划目录一起收掉，避免 SSD 被巡检图写满。
//!
//! 一轮「同步媒体」把目录里**所有**符合布局的图片回传，不再按「每航点最多 N 张」截断：
//! 板端同一个航点每跑一轮就往 `pt<n>/` 里多写 `mediaFrames` 张（文件名带时间戳），
//! 十轮就是几十张；以前按 `frames` 截断，每次都只传走最老的 N 张，剩下的永远留在板子上
//! （2026-11-20 现场：点完「同步媒体」回执说删了 6 张，目录里还躺着 6 张）。
//! `frames`（中台 `media_frames`）现在只管**板端每轮写几张**，回传侧以「清空目录」为准；
//! 单轮上限 `MAX_FILES_PER_RUN` 仍然保留，防止误配目录一次传爆。
//!
//! 与 `mapupload` 的分工：那边回传地图包（走 `/api/map-versions/ingest`），
//! 这边只回传图片（走 `/api/media/ingest`），登录约定一致。

use robodog_device_client::config::Config;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{info, warn};

/// 板端 AI 存证根目录默认值（中台参数配置 `dog_config.media_picture_dir` 优先，
/// 板端 `MEDIA_DATA_DIR` 是同一份约定的兜底）。
pub const DEFAULT_PICTURE_DIR: &str = "/media/media-data/picture";
/// 板端每航点每轮写入几张的默认值（中台 `dog_config.media_frames`，1~10）。
///
/// 只用于日志与老中台兼容：回传以「清空目录」为准，不再按它截断。
pub const DEFAULT_FRAMES: usize = 3;
/// 一次「同步媒体」最多扫多少张：防止误配路径（例如指到 `/`）把整盘都传上去。
pub const MAX_FILES_PER_RUN: usize = 200;
/// 单张图片上限，与业务平台 / 中台保持一致（8MB）。
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// 一个压缩包最多装几张。8 张 ≈ 2.4MB，4G 上约 20~30s 一个包；
/// 太少省不下握手、太多单包重传代价大。
const BATCH_MAX_FILES: usize = 8;
/// 一个压缩包的**原始字节**上限（JPEG 已压缩，zip 用 Stored 只做打包）。
const BATCH_MAX_BYTES: usize = 8 * 1024 * 1024;
/// 连续失败几次（按张计）就中止本轮（网络断了没必要把几百张都试一遍）。
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
/// 单个压缩包上传超时：8MB 在弱网（~1Mbps）也要 1 分钟以上
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// 一张待回传的图片
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFile {
    pub path: PathBuf,
    pub plan_id: String,
    pub point_index: i32,
    pub size: u64,
}

/// 压缩包里的一个条目（`zip_name` 是包内名，元数据里的原名在 `remarks`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchEntry {
    pub zip_name: String,
    pub file: MediaFile,
}

/// 一次同步的结果（`Display` 直接作为指令回执文案给中台审计看）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncSummary {
    pub scanned: usize,
    pub uploaded: usize,
    pub existing: usize,
    pub deleted: usize,
    pub failed: usize,
    pub batches: usize,
    /// 本轮结束后板端还剩几张（失败 / 超过 `MAX_FILES_PER_RUN` 截断的）
    pub remaining: usize,
    pub errors: Vec<String>,
    pub picture_dir: String,
}

impl SyncSummary {
    /// 板端目录是否已经清空（中台据此显示「完成」还是「还有剩余」）。
    pub fn drained(&self) -> bool {
        self.remaining == 0
    }
}

impl std::fmt::Display for SyncSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.scanned == 0 && self.failed == 0 {
            return write!(f, "板端媒体目录没有待回传的图片（{}）", self.picture_dir);
        }
        write!(
            f,
            "媒体回传完成：扫描 {} 张，上传成功 {}（新增 {} / 已存在 {}），失败 {}，已删除板端文件 {}，共 {} 个压缩包",
            self.scanned,
            self.uploaded,
            self.uploaded.saturating_sub(self.existing),
            self.existing,
            self.failed,
            self.deleted,
            self.batches
        )?;
        if self.drained() {
            write!(f, "；板端目录已清空（{}）", self.picture_dir)?;
        } else {
            write!(
                f,
                "；板端仍有 {} 张未回传，请再点一次「同步媒体」（{}）",
                self.remaining, self.picture_dir
            )?;
        }
        if !self.errors.is_empty() {
            write!(f, "；前几条错误：{}", self.errors.join(" | "))?;
        }
        Ok(())
    }
}

/// 一轮同步的进度（每传完一个压缩包上报一次，供中台设备页显示「同步中 8/24」）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncProgress {
    pub uploaded: usize,
    pub failed: usize,
    pub total: usize,
    pub batches_done: usize,
    pub batches_total: usize,
}

/// 进度回调。同步是长任务（弱网下一包 20~30s），没法等 `command_result`
/// 才知道进展，所以每批结束就回调一次。
pub type ProgressFn<'a> = &'a (dyn Fn(SyncProgress) + Send + Sync);

/// 解析中台下发的指令参数。
///
/// 兼容三种写法：`params.params.pictureDir`（业务平台包了一层）、`params.pictureDir`、
/// 以及完全没有参数（回退默认目录/帧数）。路径必须是绝对路径且不含 `..`：
/// 它是板端目录，拼错就会去扫别的地方。
pub fn parse_params(params: Option<&Value>) -> Result<(PathBuf, usize, Option<String>), String> {
    let params = params
        .and_then(|value| value.get("params"))
        .or(params)
        .cloned()
        .unwrap_or_else(|| json!({}));
    let dir = params
        .get("pictureDir")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_PICTURE_DIR);
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() {
        return Err(format!(
            "板端媒体目录必须是绝对路径（收到 {}）",
            dir.display()
        ));
    }
    if dir
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(format!(
            "板端媒体目录不允许包含 ..（收到 {}）",
            dir.display()
        ));
    }
    let frames = params
        .get("frames")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_FRAMES)
        .clamp(1, 10);
    let plan_id = params
        .get("planId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok((dir, frames, plan_id))
}

/// 扫出待回传的图片：`<root>/<planId>/pt<n>/<*.jpg|jpeg|png>`。
///
/// - 只认 `pt<数字>` 子目录（与板端 `evidence.py` 的落盘布局一致）；
/// - **每个航点不再按张数截断**（见文件头：同一个 `pt<n>/` 会攒下多轮巡检的帧，
///   截断会让板端永远传不干净）；总数仍受 `MAX_FILES_PER_RUN` 约束；
/// - 任何解析失败都跳过，不让一个脏目录毁掉整轮回传。
pub fn collect_files(root: &Path, plan_filter: Option<&str>) -> Result<Vec<MediaFile>, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("板端媒体目录不可用（{}）: {error}", root.display()))?;
    let mut files = Vec::new();
    let plans = std::fs::read_dir(&root)
        .map_err(|error| format!("读取板端媒体目录失败（{}）: {error}", root.display()))?;
    for plan_entry in plans.flatten() {
        if !plan_entry.path().is_dir() {
            continue;
        }
        let plan_id = plan_entry.file_name().to_string_lossy().to_string();
        if plan_id.starts_with('.') {
            continue;
        }
        if let Some(filter) = plan_filter
            && plan_id != filter
        {
            continue;
        }
        let points = match std::fs::read_dir(plan_entry.path()) {
            Ok(points) => points,
            Err(error) => {
                warn!(
                    "读取计划目录失败（{}）: {error}",
                    plan_entry.path().display()
                );
                continue;
            }
        };
        for point_entry in points.flatten() {
            let point_path = point_entry.path();
            if !point_path.is_dir() {
                continue;
            }
            let point_name = point_entry.file_name().to_string_lossy().to_string();
            let Some(point_index) = parse_point_index(&point_name) else {
                continue;
            };
            // 只认板端媒体根下的目录：软链指向别处时不传
            match point_path.canonicalize() {
                Ok(real) if real.starts_with(&root) => {}
                _ => {
                    warn!("跳过越界的航点目录：{}", point_path.display());
                    continue;
                }
            }
            let mut point_files: Vec<MediaFile> = std::fs::read_dir(&point_path)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| is_image(entry.path().as_path()))
                .filter_map(|entry| {
                    let path = entry.path();
                    let size = entry.metadata().ok()?.len();
                    Some(MediaFile {
                        path,
                        plan_id: plan_id.clone(),
                        point_index,
                        size,
                    })
                })
                .collect();
            point_files.sort_by(|a, b| a.path.cmp(&b.path));
            files.extend(point_files);
            if files.len() >= MAX_FILES_PER_RUN {
                files.truncate(MAX_FILES_PER_RUN);
                return Ok(files);
            }
        }
    }
    Ok(files)
}

/// 数一遍板端还剩几张待回传的图（不受 `MAX_FILES_PER_RUN` 截断影响）。
///
/// 只要一个数，所以不建 `MediaFile`、也不跟着软链走；目录读不了就当 0。
fn count_images(root: &Path, plan_filter: Option<&str>) -> usize {
    let Ok(root) = root.canonicalize() else {
        return 0;
    };
    let Ok(plans) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut count = 0usize;
    for plan_entry in plans.flatten() {
        let plan_path = plan_entry.path();
        if !plan_path.is_dir() || plan_entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if let Some(filter) = plan_filter
            && plan_entry.file_name().to_string_lossy() != filter
        {
            continue;
        }
        let Ok(points) = std::fs::read_dir(&plan_path) else {
            continue;
        };
        for point_entry in points.flatten() {
            let point_path = point_entry.path();
            if !point_path.is_dir()
                || parse_point_index(&point_entry.file_name().to_string_lossy()).is_none()
            {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&point_path) {
                count += entries
                    .flatten()
                    .filter(|entry| is_image(&entry.path()))
                    .count();
            }
        }
    }
    count
}

/// 按「每包 8 张 / 8MB」把待传图片切成压缩包批次。
pub fn build_batches(files: Vec<MediaFile>) -> Vec<Vec<BatchEntry>> {
    let mut batches = Vec::new();
    let mut current: Vec<BatchEntry> = Vec::new();
    let mut current_bytes = 0u64;
    for file in files {
        let overflow = !current.is_empty()
            && (current.len() >= BATCH_MAX_FILES
                || current_bytes.saturating_add(file.size) > BATCH_MAX_BYTES as u64);
        if overflow {
            batches.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        let zip_name = zip_entry_name(current.len() + 1, &file.path);
        current_bytes = current_bytes.saturating_add(file.size);
        current.push(BatchEntry { zip_name, file });
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// 包内文件名：`0001.jpg` —— **纯 ASCII**。
///
/// zip 里的中文名在不同 `unzip` 实现里会乱码（板端/运维会用 CLI 解包排查），
/// 原名放 `mediaInfo.entries[].remarks`，入中台后进 `dog_media.remarks`。
fn zip_entry_name(seq: usize, path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .unwrap_or_else(|| "jpg".to_string());
    format!("{seq:04}.{ext}")
}

/// 把一批图片打成 zip（`Stored`：JPEG 已经压缩过，Deflate 只烧板子 CPU）。
async fn build_zip(entries: &[BatchEntry]) -> Result<Vec<u8>, String> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    let mut total = 0usize;
    for entry in entries {
        let bytes = tokio::fs::read(&entry.file.path)
            .await
            .map_err(|error| format!("读取 {} 失败: {error}", entry.zip_name))?;
        if bytes.is_empty() {
            return Err(format!("{} 是空文件", entry.file.path.display()));
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!(
                "{} 超过 8MB 上限（{} 字节）",
                entry.file.path.display(),
                bytes.len()
            ));
        }
        total += bytes.len();
        writer
            .start_file(entry.zip_name.clone(), options)
            .map_err(|error| format!("写入 {} 失败: {error}", entry.zip_name))?;
        writer
            .write_all(&bytes)
            .map_err(|error| format!("写入 {} 失败: {error}", entry.zip_name))?;
    }
    if total == 0 {
        return Err("没有可打包的图片".to_string());
    }
    let cursor = writer
        .finish()
        .map_err(|error| format!("生成压缩包失败: {error}"))?;
    Ok(cursor.into_inner())
}

/// `pt3` → `3`；不匹配（`ptx` / `preview`）返回 None。
pub fn parse_point_index(name: &str) -> Option<i32> {
    let rest = name.strip_prefix("pt")?;
    if rest.is_empty() || !rest.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    rest.parse().ok()
}

fn is_image(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "png")
}

fn captured_at(path: &Path) -> String {
    use chrono::{DateTime, Local};
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or_else(|_| SystemTime::now());
    let beijing = DateTime::<Local>::from(modified);
    beijing.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 执行一轮「同步媒体」。
///
/// 返回本轮统计（`Display` 直接作为指令回执文案；`drained()` 供中台判断
/// 「目录已清空」还是「还有剩余」）。
/// `progress` 每传完一个压缩包回调一次（中台设备页据此显示「同步中 8/24」）。
pub async fn sync(
    params: Option<&Value>,
    config: &Config,
    progress: Option<ProgressFn<'_>>,
) -> Result<SyncSummary, String> {
    let (picture_dir, frames, plan_filter) = parse_params(params)?;
    let files = collect_files(&picture_dir, plan_filter.as_deref())?;
    if files.is_empty() {
        // 顺手把「只剩 result.json / 空壳子」的目录清掉：上一轮可能已经把图传完，
        // 只有结论文件还在，不清会让板端永远留一堆空航点目录。
        prune_empty(&picture_dir).await;
        return Ok(SyncSummary {
            picture_dir: picture_dir.display().to_string(),
            ..Default::default()
        });
    }
    let batches = build_batches(files);
    let total: usize = batches.iter().map(Vec::len).sum();
    let batches_total = batches.len();
    info!(
        "同步媒体：{} 下找到 {} 张待回传图片（板端每航点每轮写 {frames} 张，本轮全部回传），分 {} 个压缩包",
        picture_dir.display(),
        total,
        batches_total
    );
    // 第一次上报：让中台立刻看到「开始同步」和总量（弱网下一包要 20~30s）
    if let Some(report) = progress {
        report(SyncProgress {
            total,
            batches_total,
            ..Default::default()
        });
    }

    // 回传走平台 API：平台只开 https 入口时要带 `server.tls_ca` 钉的信任根
    let client = crate::tls::http_builder()
        .timeout(UPLOAD_TIMEOUT)
        .build()
        .map_err(|error| format!("构造 HTTP 客户端失败: {error}"))?;
    let api_base = config.resolve_api_base();
    let token = login(&client, &api_base, config).await?;

    let mut summary = SyncSummary {
        scanned: total,
        batches: batches_total,
        picture_dir: picture_dir.display().to_string(),
        ..Default::default()
    };
    let mut consecutive_failures = 0u32;
    for (index, entries) in batches.into_iter().enumerate() {
        let outcomes = upload_batch(&client, &api_base, &token, config, &entries).await;
        let outcomes = match outcomes {
            Ok(outcomes) => outcomes,
            Err(error) => {
                // 整包失败：条目没法归因，全部算失败（板端文件全留，下次重试）
                consecutive_failures += entries.len() as u32;
                summary.failed += entries.len();
                if summary.errors.len() < 5 {
                    summary.errors.push(format!(
                        "压缩包 {}（{} 张）: {error}",
                        index + 1,
                        entries.len()
                    ));
                }
                warn!("同步媒体：第 {} 个压缩包上传失败: {error}", index + 1);
                if let Some(report) = progress {
                    report(SyncProgress {
                        uploaded: summary.uploaded,
                        failed: summary.failed,
                        total,
                        batches_done: index + 1,
                        batches_total,
                    });
                }
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    summary.errors.push(format!(
                        "连续 {consecutive_failures} 张失败，已中止本轮（板端文件保留，稍后重试）"
                    ));
                    break;
                }
                continue;
            }
        };
        for (entry, outcome) in entries.iter().zip(outcomes) {
            let Some((ok, created)) = outcome else {
                consecutive_failures += 1;
                summary.failed += 1;
                if summary.errors.len() < 5 {
                    summary.errors.push(format!(
                        "{}: 中台未返回该条目的结果",
                        entry
                            .file
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    ));
                }
                continue;
            };
            if !ok {
                consecutive_failures += 1;
                summary.failed += 1;
                if summary.errors.len() < 5 {
                    summary.errors.push(format!(
                        "{}: 中台入库失败",
                        entry
                            .file
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                    ));
                }
                continue;
            }
            consecutive_failures = 0;
            summary.uploaded += 1;
            if !created {
                summary.existing += 1;
            }
            // 中台已经收下（或是重复图）→ 板端这张就没有第二次用途了
            if remove_file(&entry.file.path).await {
                summary.deleted += 1;
            }
            cleanup_empty_dirs(&picture_dir, &entry.file).await;
        }
        if let Some(report) = progress {
            report(SyncProgress {
                uploaded: summary.uploaded,
                failed: summary.failed,
                total,
                batches_done: index + 1,
                batches_total,
            });
        }
        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            summary.errors.push(format!(
                "连续 {consecutive_failures} 张失败，已中止本轮（板端文件保留，稍后重试）"
            ));
            break;
        }
    }
    // 收尾：删掉已经空的航点/计划目录（只有 result.json 的也算空），
    // 再数一遍还剩几张——中台设备页据此显示「完成」还是「还有剩余」。
    prune_empty(&picture_dir).await;
    summary.remaining = count_images(&picture_dir, plan_filter.as_deref());
    info!("同步媒体：{summary}");
    Ok(summary)
}

/// 上传一个压缩包。
///
/// 返回与 `entries` **同序**的结果：`None` = 中台没给这个条目的结论（按失败处理）。
async fn upload_batch(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    config: &Config,
    entries: &[BatchEntry],
) -> Result<Vec<Option<(bool, bool)>>, String> {
    let zip_bytes = build_zip(entries).await?;
    let meta: Vec<Value> = entries
        .iter()
        .map(|entry| {
            json!({
                "name": entry.zip_name,
                "planId": entry.file.plan_id,
                "pointIndex": entry.file.point_index,
                "capturedAt": captured_at(&entry.file.path),
                "source": "ai-patrol",
                // 真原名（包内名是 0001.jpg 这种 ASCII）
                "remarks": entry
                    .file
                    .path
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
                    .unwrap_or_default(),
            })
        })
        .collect();
    let info = json!({
        // 中台 `mediaInfo` 用 `deviceCode`（与地图回传的 `deviceId` 不同名）；
        // 业务平台会再兜底复制一份，两个字段都在
        "deviceCode": config.device_id,
        "deviceId": config.device_id,
        "source": "ai-patrol",
        "entries": meta,
    });
    let part = reqwest::multipart::Part::bytes(zip_bytes)
        .file_name(format!("media-{}.zip", config.device_id))
        .mime_str("application/zip")
        .map_err(|error| format!("构造上传体失败: {error}"))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("mediaInfo", info.to_string());
    let url = format!("{}/api/media/ingest", api_base.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .multipart(form)
        .send()
        .await
        .map_err(|error| format!("请求业务服务失败: {error}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("业务服务返回 HTTP {status}: {text}"));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("响应不是 JSON: {error}"))?;
    let results = value
        .get("data")
        .and_then(|data| data.get("results"))
        .and_then(Value::as_array)
        .ok_or_else(|| format!("响应缺少 results: {text}"))?;
    let mut by_name: HashMap<String, (bool, bool)> = HashMap::new();
    for item in results {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let ok = item.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let created = item
            .get("created")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        by_name.insert(name.to_string(), (ok, created));
    }
    Ok(entries
        .iter()
        .map(|entry| by_name.get(&entry.zip_name).copied())
        .collect())
}

/// 登录业务平台换 JWT（媒体回传 / 录像回传共用）。
pub(crate) async fn login(
    client: &reqwest::Client,
    api_base: &str,
    config: &Config,
) -> Result<String, String> {
    let url = format!("{}/api/auth/login", api_base.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&json!({
            "username": config.api_username,
            "password": config.api_password(),
        }))
        .send()
        .await
        .map_err(|error| format!("登录业务服务失败: {error}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("登录业务服务失败 HTTP {status}: {text}"));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("登录响应不是 JSON: {error}"))?;
    value
        .get("data")
        .and_then(|data| data.get("token"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("登录响应缺少 token: {text}"))
}

async fn remove_file(path: &Path) -> bool {
    match tokio::fs::remove_file(path).await {
        Ok(()) => true,
        Err(error) => {
            warn!("同步媒体：删除板端文件失败（{}）: {error}", path.display());
            false
        }
    }
}

/// 收掉已经传完的航点目录：**先删 `result.json` 再删目录**。
///
/// 板端在媒体目录里还留了一份 `result.json`（同 `detect_jobs` 的结论），
/// 只删 jpg 的话目录永远非空、`remove_dir` 一直失败，一天巡检下来会堆一堆空壳。
async fn cleanup_empty_dirs(root: &Path, file: &MediaFile) {
    let point_dir = file
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| file.path.clone());
    if has_images(&point_dir) {
        return; // 这个航点还有没传完的帧（本轮失败 / 后面还有批次）
    }
    let _ = tokio::fs::remove_file(point_dir.join("result.json")).await;
    let _ = tokio::fs::remove_dir(&point_dir).await;
    let _ = tokio::fs::remove_dir(root.join(&file.plan_id)).await;
}

/// 收尾清理：把「已经没有图片」的航点目录（连 `result.json`）和空计划目录删掉。
///
/// 为什么还需要这一次扫描：`cleanup_empty_dirs` 只在「某张图刚被删掉」时触发，
/// 而现场更常见的情况是整个航点上一轮就传完了、只留下一个 `result.json` 壳子，
/// 或者本轮一张都没传成。不清的话板子上会积一堆空目录，「同步完成后目录是空的」
/// 这个预期就对不上。
async fn prune_empty(root: &Path) {
    let Ok(root) = root.canonicalize() else {
        return;
    };
    let Ok(plans) = std::fs::read_dir(&root) else {
        return;
    };
    for plan_entry in plans.flatten() {
        let plan_path = plan_entry.path();
        if !plan_path.is_dir() || plan_entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(points) = std::fs::read_dir(&plan_path) else {
            continue;
        };
        for point_entry in points.flatten() {
            let point_path = point_entry.path();
            if !point_path.is_dir() || has_images(&point_path) {
                continue;
            }
            let _ = tokio::fs::remove_file(point_path.join("result.json")).await;
            let _ = tokio::fs::remove_dir(&point_path).await;
        }
        // 计划目录空了（没有航点、也没别的文件）就连它一起收掉
        let _ = tokio::fs::remove_dir(&plan_path).await;
    }
}

fn has_images(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| entries.flatten().any(|entry| is_image(&entry.path())))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "media-sync-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn parse_params_defaults_and_clamps() {
        let (dir, frames, plan) = parse_params(None).expect("默认参数可用");
        assert_eq!(dir, PathBuf::from(DEFAULT_PICTURE_DIR));
        assert_eq!(frames, DEFAULT_FRAMES);
        assert!(plan.is_none());

        let value = json!({"params": {"pictureDir": "/data/media", "frames": 99, "planId": "T-1"}});
        let (dir, frames, plan) = parse_params(Some(&value)).expect("内层 params 优先");
        assert_eq!(dir, PathBuf::from("/data/media"));
        assert_eq!(frames, 10, "帧数上限 10");
        assert_eq!(plan.as_deref(), Some("T-1"));
    }

    #[test]
    fn parse_params_rejects_relative_and_traversal() {
        assert!(parse_params(Some(&json!({"pictureDir": "media-data"}))).is_err());
        assert!(parse_params(Some(&json!({"pictureDir": "/media/../etc"}))).is_err());
    }

    #[test]
    fn point_index_parses_only_ptn() {
        assert_eq!(parse_point_index("pt0"), Some(0));
        assert_eq!(parse_point_index("pt12"), Some(12));
        assert_eq!(parse_point_index("pt"), None);
        assert_eq!(parse_point_index("ptx"), None);
        assert_eq!(parse_point_index("preview"), None);
    }

    #[test]
    fn collect_files_drains_every_frame_of_the_point() {
        let root = tmp_root("collect");
        let point = root.join("T-1").join("pt2");
        std::fs::create_dir_all(&point).expect("创建航点目录");
        for name in [
            "dogcam_01_20261120_145844.jpg",
            "dogcam_02_20261120_145844.jpg",
            "dogcam_03_20261120_145844.jpg",
            "dogcam_01_20261120_165044.jpg",
            "dogcam_02_20261120_165044.jpg",
            "dogcam_03_20261120_165044.jpg",
            "result.json",
        ] {
            std::fs::write(point.join(name), b"x").expect("写文件");
        }
        // 非 `pt<n>` 目录与计划过滤都不该被收
        std::fs::create_dir_all(root.join("T-1").join("preview")).expect("创建脏目录");
        std::fs::write(root.join("T-1").join("preview").join("a.jpg"), b"x").expect("写文件");

        let files = collect_files(&root, Some("T-1")).expect("扫描成功");
        assert_eq!(
            files.len(),
            6,
            "两轮巡检攒下的 6 帧必须一次全部回传（不能按每航点 3 张截断，
             否则板端目录永远传不干净——2026-11-20 现场问题）"
        );
        assert!(files.iter().all(|file| file.point_index == 2));
        assert_eq!(files[0].plan_id, "T-1");
        assert!(
            files
                .iter()
                .all(|file| file.path.extension().and_then(|e| e.to_str()) == Some("jpg"))
        );

        assert!(
            collect_files(&root, Some("other"))
                .expect("扫描成功")
                .is_empty(),
            "计划过滤"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_files_reports_missing_root() {
        let error =
            collect_files(Path::new("/definitely/not/here"), None).expect_err("目录不存在要报错");
        assert!(error.contains("不可用"), "{error}");
    }

    #[tokio::test]
    async fn prune_empty_removes_shell_dirs_left_by_previous_runs() {
        let root = tmp_root("prune");
        // 上一轮已经把图传走，只剩结论文件
        let shell = root.join("T-9").join("pt0");
        std::fs::create_dir_all(&shell).expect("创建航点目录");
        std::fs::write(shell.join("result.json"), b"{}").expect("写 result.json");
        // 还有图的航点不能被碰
        let alive = root.join("T-9").join("pt1");
        std::fs::create_dir_all(&alive).expect("创建航点目录");
        std::fs::write(alive.join("a.jpg"), b"x").expect("写图");

        assert_eq!(
            count_images(&root, Some("T-9")),
            1,
            "只剩 result.json 的航点不该算作待回传"
        );
        assert_eq!(count_images(&root, Some("T-其他")), 0, "计划过滤");

        prune_empty(&root).await;
        assert!(!shell.exists(), "只剩 result.json 的航点目录要收掉");
        assert!(alive.join("a.jpg").is_file(), "还有图的航点不能动");
        assert!(root.join("T-9").is_dir(), "计划目录还有图，不能删");

        // 计划目录也空了就连它一起收
        std::fs::remove_file(alive.join("a.jpg")).expect("删图");
        prune_empty(&root).await;
        assert!(!root.join("T-9").exists(), "空计划目录要收掉");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn cleanup_removes_result_json_and_empty_plan_dir() {
        let root = tmp_root("cleanup");
        let point = root.join("T-2").join("pt1");
        std::fs::create_dir_all(&point).expect("创建航点目录");
        std::fs::write(point.join("result.json"), b"{}").expect("写 result.json");
        let file = MediaFile {
            path: point.join("a.jpg"),
            plan_id: "T-2".to_string(),
            point_index: 1,
            size: 1,
        };
        cleanup_empty_dirs(&root, &file).await;
        assert!(!point.exists(), "没有图片后应连 result.json 一起收掉");
        assert!(!root.join("T-2").exists(), "计划目录空了就删掉");

        // 还有没传完的帧：目录必须保留（否则下次重试就没图了）
        let point = root.join("T-3").join("pt0");
        std::fs::create_dir_all(&point).expect("创建航点目录");
        std::fs::write(point.join("b.jpg"), b"x").expect("写图");
        let file = MediaFile {
            path: point.join("c.jpg"),
            plan_id: "T-3".to_string(),
            point_index: 0,
            size: 1,
        };
        cleanup_empty_dirs(&root, &file).await;
        assert!(point.join("b.jpg").is_file(), "没传完的帧不能被清掉");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn summary_mentions_counts_and_errors() {
        let summary = SyncSummary {
            scanned: 4,
            uploaded: 3,
            existing: 1,
            deleted: 3,
            failed: 1,
            batches: 1,
            remaining: 1,
            errors: vec!["a.jpg: 超时".to_string()],
            picture_dir: "/media/media-data/picture".to_string(),
        };
        let text = summary.to_string();
        assert!(text.contains("扫描 4 张"), "{text}");
        assert!(text.contains("新增 2 / 已存在 1"), "{text}");
        assert!(text.contains("共 1 个压缩包"), "{text}");
        assert!(text.contains("仍有 1 张未回传"), "{text}");
        assert!(text.contains("a.jpg: 超时"), "{text}");

        let drained = SyncSummary {
            scanned: 2,
            uploaded: 2,
            deleted: 2,
            batches: 1,
            picture_dir: "/media/media-data/picture".to_string(),
            ..Default::default()
        };
        assert!(drained.drained());
        assert!(
            drained.to_string().contains("板端目录已清空"),
            "{}",
            drained
        );
    }

    fn media_file(
        dir: &std::path::Path,
        plan: &str,
        point: i32,
        name: &str,
        size: u64,
    ) -> MediaFile {
        MediaFile {
            path: dir.join(plan).join(format!("pt{point}")).join(name),
            plan_id: plan.to_string(),
            point_index: point,
            size,
        }
    }

    #[test]
    fn batches_split_by_count_and_bytes() {
        let dir = PathBuf::from("/media/media-data/picture");
        // 9 张 1KB：按「每包 8 张」切成 8 + 1
        let files: Vec<MediaFile> = (0..9)
            .map(|index| media_file(&dir, "T-1", 0, &format!("{index}.jpg"), 1024))
            .collect();
        let batches = build_batches(files);
        assert_eq!(batches.len(), 2, "每包最多 8 张");
        assert_eq!(batches[0].len(), 8);
        assert_eq!(batches[1].len(), 1);
        assert_eq!(batches[0][0].zip_name, "0001.jpg", "包内名按序号，ASCII");
        assert_eq!(batches[0][7].zip_name, "0008.jpg");
        assert_eq!(batches[1][0].zip_name, "0001.jpg", "新包序号重新开始");

        // 字节上限：单张 6MB 时一个包只装得下 1 张（8MB 上限）
        let big: Vec<MediaFile> = (0..3)
            .map(|index| media_file(&dir, "T-2", index, &format!("{index}.jpg"), 6 * 1024 * 1024))
            .collect();
        let batches = build_batches(big);
        assert_eq!(batches.len(), 3, "6MB + 6MB 超过 8MB → 每张一包");
        assert!(batches.iter().all(|batch| batch.len() == 1));

        assert!(build_batches(Vec::new()).is_empty());
    }

    #[tokio::test]
    async fn zip_contains_every_entry_with_ascii_names() {
        let root = tmp_root("zip");
        let point = root.join("T-1").join("pt0");
        std::fs::create_dir_all(&point).expect("创建航点目录");
        let mut files = Vec::new();
        for index in 0..2 {
            let name = format!("dogcam_0{index}.jpg");
            std::fs::write(point.join(&name), format!("jpeg-{index}")).expect("写图");
            files.push(media_file(&root, "T-1", 0, &name, 6));
        }
        let entries: Vec<BatchEntry> = build_batches(files).into_iter().flatten().collect();
        assert_eq!(entries.len(), 2);
        let bytes = build_zip(&entries).await.expect("打包成功");
        assert!(bytes.starts_with(b"PK\x03\x04"), "应是 zip 魔数");

        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("能解包");
        assert_eq!(archive.len(), 2);
        let mut names = Vec::new();
        for index in 0..archive.len() {
            let entry = archive.by_index(index).expect("读条目");
            names.push(entry.name().to_string());
        }
        assert_eq!(names, vec!["0001.jpg", "0002.jpg"]);
        let mut first = archive.by_name("0001.jpg").expect("按名取");
        let mut content = String::new();
        std::io::Read::read_to_string(&mut first, &mut content).expect("读内容");
        assert_eq!(content, "jpeg-0");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn zip_rejects_oversized_entry() {
        let root = tmp_root("zip-big");
        let point = root.join("T-1").join("pt0");
        std::fs::create_dir_all(&point).expect("创建航点目录");
        std::fs::write(point.join("big.jpg"), vec![0u8; 9 * 1024 * 1024]).expect("写大图");
        // 扫描时只记大小，打包时按真实字节复核 8MB 上限
        let files = vec![media_file(&root, "T-1", 0, "big.jpg", 9 * 1024 * 1024)];
        let entries: Vec<BatchEntry> = build_batches(files).into_iter().flatten().collect();
        let error = build_zip(&entries).await.expect_err("超 8MB 应拒绝");
        assert!(error.contains("8MB"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
