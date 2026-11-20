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
//!         → 逐张 POST {业务平台}/api/media/ingest（file + mediaInfo）
//!           → 业务平台转发中台 /api/dogInternal/media/upload → dog_media
//!             → 中台「媒体资源」页展示
//! ```
//!
//! 板端文件**上传成功才删**（中台按 sha256 去重，`created=false` 也算成功），
//! 删完把空的 `pt<n>/`、计划目录一起收掉，避免 SSD 被巡检图写满。
//!
//! 与 `mapupload` 的分工：那边回传地图包（走 `/api/map-versions/ingest`），
//! 这边只回传图片（走 `/api/media/ingest`），登录与 multipart 约定完全一致。

use robodog_device_client::config::Config;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};
use tracing::{info, warn};

/// 板端 AI 存证根目录默认值（中台参数配置 `dog_config.media_picture_dir` 优先，
/// 板端 `MEDIA_DATA_DIR` 是同一份约定的兜底）。
pub const DEFAULT_PICTURE_DIR: &str = "/media/media-data/picture";
/// 每航点最多回传几张（中台 `dog_config.media_frames`，1~10）。
pub const DEFAULT_FRAMES: usize = 3;
/// 一次「同步媒体」最多扫多少张：防止误配路径（例如指到 `/`）把整盘都传上去。
pub const MAX_FILES_PER_RUN: usize = 200;
/// 单张图片上限，与业务平台 / 中台保持一致（8MB）。
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
/// 连续失败几次就中止本轮（网络断了没必要把几百张都试一遍）。
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// 一张待回传的图片
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFile {
    pub path: PathBuf,
    pub plan_id: String,
    pub point_index: i32,
    pub size: u64,
}

/// 一次同步的结果（`Display` 直接作为指令回执文案给中台审计看）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncSummary {
    pub scanned: usize,
    pub uploaded: usize,
    pub existing: usize,
    pub deleted: usize,
    pub failed: usize,
    pub errors: Vec<String>,
    pub picture_dir: String,
}

impl std::fmt::Display for SyncSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "媒体回传完成：扫描 {} 张，上传成功 {}（新增 {} / 已存在 {}），失败 {}，已删除板端文件 {}（目录 {}）",
            self.scanned,
            self.uploaded,
            self.uploaded.saturating_sub(self.existing),
            self.existing,
            self.failed,
            self.deleted,
            self.picture_dir
        )?;
        if !self.errors.is_empty() {
            write!(f, "；前几条错误：{}", self.errors.join(" | "))?;
        }
        Ok(())
    }
}

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
/// - 每个航点最多取 `frames` 张（按文件名排序，板端命名带时间戳）；
/// - 任何解析失败都跳过，不让一个脏目录毁掉整轮回传。
pub fn collect_files(
    root: &Path,
    plan_filter: Option<&str>,
    frames: usize,
) -> Result<Vec<MediaFile>, String> {
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
            files.extend(point_files.into_iter().take(frames.max(1)));
            if files.len() >= MAX_FILES_PER_RUN {
                files.truncate(MAX_FILES_PER_RUN);
                return Ok(files);
            }
        }
    }
    Ok(files)
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

fn content_type(path: &Path) -> &'static str {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .eq_ignore_ascii_case("png")
    {
        "image/png"
    } else {
        "image/jpeg"
    }
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
/// 返回指令回执文案（成功也有内容：中台审计里能直接看到传了几张、删了几张）。
pub async fn sync(params: Option<&Value>, config: &Config) -> Result<String, String> {
    let (picture_dir, frames, plan_filter) = parse_params(params)?;
    let files = collect_files(&picture_dir, plan_filter.as_deref(), frames)?;
    if files.is_empty() {
        return Ok(format!(
            "板端媒体目录没有待回传的图片（{}）",
            picture_dir.display()
        ));
    }
    info!(
        "同步媒体：{} 下找到 {} 张待回传图片（每航点上限 {frames} 张）",
        picture_dir.display(),
        files.len()
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| format!("构造 HTTP 客户端失败: {error}"))?;
    let api_base = config.resolve_api_base();
    let token = login(&client, &api_base, config).await?;

    let mut summary = SyncSummary {
        scanned: files.len(),
        picture_dir: picture_dir.display().to_string(),
        ..Default::default()
    };
    let mut consecutive_failures = 0u32;
    for file in files {
        match upload_one(&client, &api_base, &token, config, &file).await {
            Ok(created) => {
                consecutive_failures = 0;
                summary.uploaded += 1;
                if !created {
                    summary.existing += 1;
                }
                // 中台已经收下（或是重复图）→ 板端这张就没有第二次用途了
                if remove_file(&file.path).await {
                    summary.deleted += 1;
                }
                cleanup_empty_dirs(&picture_dir, &file).await;
            }
            Err(error) => {
                consecutive_failures += 1;
                summary.failed += 1;
                if summary.errors.len() < 5 {
                    summary.errors.push(format!(
                        "{}: {error}",
                        file.path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
                warn!("同步媒体：上传失败 {}: {error}", file.path.display());
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    summary.errors.push(format!(
                        "连续 {consecutive_failures} 张失败，已中止本轮（板端文件保留，稍后重试）"
                    ));
                    break;
                }
            }
        }
    }
    info!("同步媒体：{summary}");
    Ok(summary.to_string())
}

async fn login(
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

/// 上传一张；返回 `created`（false = 中台按 sha256 判定已存在）。
async fn upload_one(
    client: &reqwest::Client,
    api_base: &str,
    token: &str,
    config: &Config,
    file: &MediaFile,
) -> Result<bool, String> {
    if file.size > MAX_IMAGE_BYTES as u64 {
        return Err(format!("超过 8MB 上限（{} 字节）", file.size));
    }
    let bytes = tokio::fs::read(&file.path)
        .await
        .map_err(|error| format!("读取失败: {error}"))?;
    if bytes.is_empty() {
        return Err("文件为空".to_string());
    }
    let name = file
        .path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "frame.jpg".to_string());
    let info = json!({
        // 中台 `mediaInfo` 用 `deviceCode`（与地图回传的 `deviceId` 不同名）；
        // 业务平台会再兜底复制一份，两个字段都在
        "deviceCode": config.device_id,
        "deviceId": config.device_id,
        "planId": file.plan_id,
        "pointIndex": file.point_index,
        "capturedAt": captured_at(&file.path),
        "source": "ai-patrol",
        "remarks": name,
    });
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(name)
        .mime_str(content_type(&file.path))
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
    Ok(value
        .get("data")
        .and_then(|data| data.get("created"))
        .and_then(Value::as_bool)
        .unwrap_or(true))
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
        return; // 这个航点还有没传完的帧（本轮失败或超过 frames 上限）
    }
    let _ = tokio::fs::remove_file(point_dir.join("result.json")).await;
    let _ = tokio::fs::remove_dir(&point_dir).await;
    let _ = tokio::fs::remove_dir(root.join(&file.plan_id)).await;
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
    fn collect_files_respects_layout_and_frames_cap() {
        let root = tmp_root("collect");
        let point = root.join("T-1").join("pt2");
        std::fs::create_dir_all(&point).expect("创建航点目录");
        for name in ["cam0_1.jpg", "cam0_2.jpg", "cam0_3.jpg", "result.json"] {
            std::fs::write(point.join(name), b"x").expect("写文件");
        }
        // 非 `pt<n>` 目录与计划过滤都不该被收
        std::fs::create_dir_all(root.join("T-1").join("preview")).expect("创建脏目录");
        std::fs::write(root.join("T-1").join("preview").join("a.jpg"), b"x").expect("写文件");

        let files = collect_files(&root, Some("T-1"), 2).expect("扫描成功");
        assert_eq!(files.len(), 2, "每航点最多 frames 张");
        assert!(files.iter().all(|file| file.point_index == 2));
        assert_eq!(files[0].plan_id, "T-1");
        assert!(
            files
                .iter()
                .all(|file| file.path.extension().and_then(|e| e.to_str()) == Some("jpg"))
        );

        assert!(
            collect_files(&root, Some("other"), 3)
                .expect("扫描成功")
                .is_empty(),
            "计划过滤"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_files_reports_missing_root() {
        let error = collect_files(Path::new("/definitely/not/here"), None, 3)
            .expect_err("目录不存在要报错");
        assert!(error.contains("不可用"), "{error}");
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
            errors: vec!["a.jpg: 超时".to_string()],
            picture_dir: "/media/media-data/picture".to_string(),
        };
        let text = summary.to_string();
        assert!(text.contains("扫描 4 张"), "{text}");
        assert!(text.contains("新增 2 / 已存在 1"), "{text}");
        assert!(text.contains("a.jpg: 超时"), "{text}");
    }
}
