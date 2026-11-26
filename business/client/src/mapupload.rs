//! 建图结果回传中台（A 方案）。
//!
//! 链路：
//!
//! ```text
//! 智航本体 (4001 保存成功)
//!   └─ HTTP GET  http://<robot>:<file-port>/...   拉取地图包
//!        └─ POST /api/map-versions/ingest         交给业务服务
//!             └─ 转发中台 POST /api/dogMap/upload  → dog_map
//!                  └─ POST /api/map-versions/import-backend → map_versions
//! ```
//!
//! 客户端只负责「取包 + 交给业务服务」，中台的账号与导流逻辑留在服务端，
//! 避免把中台凭证分发到每台机器狗上。

use crate::zhpro::mapping::{MappingSaved, RobotMap};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{Cursor, Write};
use std::time::Duration;
use tracing::{info, warn};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// 回传结果
#[derive(Debug, Clone)]
pub enum UploadOutcome {
    /// 按配置跳过（如 `--no-upload-map`、未给园区）
    Skipped(String),
    Uploaded {
        map_id: String,
    },
    Failed {
        reason: String,
    },
}

/// 回传配置
#[derive(Debug, Clone)]
pub struct IngestConfig {
    /// 业务服务 REST 基地址，如 `http://127.0.0.1:3100`
    pub api_base: String,
    pub username: String,
    pub password: String,
    /// 本机设备编号，随地图一起上报用于确定中台归属用户
    pub device_id: String,
    /// 目标园区
    pub park_id: Option<i64>,
    /// 覆盖地图名称
    pub map_name: Option<String>,
    /// 是否导入后顺手绑定为园区基线
    pub bind: bool,
    /// 刷新模式：中台/控制台显式点「同步地图」时为 `true`。
    ///
    /// - `false`（默认）：`sourceRef` 已入库的图直接跳过，**只补平台没有的图**；
    /// - `true`：不预检已同步列表，每张图都交给服务端判「内容变没变」，
    ///   内容有变化则覆盖中台同一条 `dog_map`（已发布/部分标定的版本会被拒绝）。
    pub refresh: bool,
}

/// 从本体拉取地图包。
///
/// 智航 Pro 的地图目录结构为 `<map_root>/history_map/<目录名>/`，内含
/// `model.pgm` / `model.yaml` 等文件。本体的文件服务约定为把**整个目录**打包成
/// zip 暴露在同一路径下，因此这里按以下顺序探测：
///
/// 1. `<base>/history_map/<name>.zip`
/// 2. `<base>/<name>.zip`
/// 3. `<base>/history_map/<name>?format=zip`（部分实现用 query 指定打包）
///
/// 全部失败时返回错误，由调用方决定是否降级为「仅记录路径」。
pub async fn fetch_map_package(
    http: &reqwest::Client,
    file_base: &str,
    map_dir_name: &str,
    robot_map_path: Option<&str>,
) -> Result<Vec<u8>, String> {
    // 部分固件把地图目录直接打成 zip 暴露，先按老约定探测
    match fetch_zip_package(http, file_base, map_dir_name).await {
        Ok(bytes) => Ok(bytes),
        Err(zip_error) => {
            // 现场 M1 的 robot_http_server（http-fileserver 3.0.0）没有 zip 端点：
            // 只有 `/api/list?path=` 列目录 + `/files/<path>` 单文件下载，
            // 所以按目录递归取文件、在客户端现场打包。
            match fetch_package_via_fileserver(http, file_base, map_dir_name, robot_map_path).await
            {
                Ok(bytes) => Ok(bytes),
                Err(file_error) => Err(format!(
                    "无法从本体获取地图包（zip 探测: {zip_error}；文件服务打包: {file_error}）"
                )),
            }
        }
    }
}

/// 老约定：地图目录被文件服务打成 zip
async fn fetch_zip_package(
    http: &reqwest::Client,
    file_base: &str,
    map_dir_name: &str,
) -> Result<Vec<u8>, String> {
    let base = file_base.trim_end_matches('/');
    // 文档 §4「文件服务」：HTTP 端口 12000，L2 根 /userdata/alg_data、M1 根 /ota/alg_data；
    // 文档 §2.2.4.1「4013 地图列表」：单张地图路径为
    // <map_dir_prefix>/history_map/<地图目录名>，map_dir_prefix 即 /ota/alg_data/map。
    // 前两条按文档绝对路径拼，后四条兼容用户已把根目录写进 --robot-file-base 的情况。
    let candidates = [
        format!("{base}/ota/alg_data/map/history_map/{map_dir_name}.zip"),
        format!("{base}/userdata/alg_data/map/history_map/{map_dir_name}.zip"),
        format!("{base}/history_map/{map_dir_name}.zip"),
        format!("{base}/{map_dir_name}.zip"),
        format!("{base}/history_map/{map_dir_name}?format=zip"),
        format!("{base}/{map_dir_name}?format=zip"),
    ];

    let mut last_error = String::from("未知错误");
    for url in candidates {
        match http.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(bytes) if is_zip(&bytes) => {
                    info!("已从本体取到地图包: {url} ({} 字节)", bytes.len());
                    return Ok(bytes.to_vec());
                }
                Ok(bytes) => {
                    warn!("{url} 返回内容不是 zip（{} 字节），继续探测", bytes.len());
                    last_error = format!("{url} 返回的不是 zip");
                }
                Err(e) => {
                    last_error = format!("{url} 读取失败: {e}");
                }
            },
            Ok(resp) => {
                last_error = format!("{url} → HTTP {}", resp.status());
            }
            Err(e) => {
                last_error = format!("{url} 请求失败: {e}");
            }
        }
    }
    Err(last_error)
}

/// 单文件打包上限：地图目录里 `model_map.bin` 200MB+、`map.bin` 127MB，回传没有意义
const MAX_PACK_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// 单次打包总字节上限
const MAX_PACK_TOTAL_BYTES: u64 = 200 * 1024 * 1024;
/// 单次打包文件数上限（防异常目录把回传拖死）
const MAX_PACK_FILES: usize = 64;
/// 目录黑名单：`map.static` 是建图过程的逐帧点云（实测单个地图目录 1700+ 个 pcd、
/// 几百 MB），回传用不到，必须跳过。
const SKIP_DIR_PREFIXES: [&str; 2] = ["map.static", "map_edit.bak"];
/// 目录递归深度上限（`<地图目录>/{map.static,map_edit,nav_paths}/…`）
const MAX_PACK_DEPTH: usize = 3;

/// 文件服务（robot_http_server）的目录项
#[derive(Debug, Clone, Deserialize)]
struct FileEntry {
    #[serde(default)]
    name: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    size: u64,
    /// 相对文件服务根目录（`/ota/alg_data`）的路径
    #[serde(default)]
    path: String,
}

#[derive(Debug, Default, Deserialize)]
struct ListResponse {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    entries: Vec<FileEntry>,
}

/// 一个待打包的文件
#[derive(Debug, Clone, PartialEq, Eq)]
struct PackedFile {
    /// zip 内的相对路径（相对地图目录）
    rel: String,
    /// 下载用的相对文件服务根目录路径
    path: String,
}

/// 文件服务没有 zip 端点时：列目录 → 递归收集 → 下载 → 本机打 zip
async fn fetch_package_via_fileserver(
    http: &reqwest::Client,
    file_base: &str,
    map_dir_name: &str,
    robot_map_path: Option<&str>,
) -> Result<Vec<u8>, String> {
    let base = file_base.trim_end_matches('/').to_string();
    let mut api_paths: Vec<String> = Vec::new();
    if let Some(path) = robot_map_path {
        let stripped = strip_alg_root(path);
        if !stripped.is_empty() {
            api_paths.push(stripped);
        }
    }
    // 4013 的 map_dir_prefix 固定为 `<root>/map`，兜底用文档约定
    let fallback = format!("/map/history_map/{map_dir_name}");
    if !api_paths.iter().any(|path| path == &fallback) {
        api_paths.push(fallback);
    }

    let mut last_error = String::from("未知错误");
    for api_path in api_paths {
        match collect_pack_files(http, &base, &api_path).await {
            Ok(files) if !files.is_empty() => {
                info!(
                    "文件服务目录 {api_path} 收集到 {} 个文件，开始下载打包",
                    files.len()
                );
                return pack_files(http, &base, &files).await;
            }
            Ok(_) => last_error = format!("{api_path} 下没有可打包的文件"),
            Err(error) => last_error = format!("{api_path}: {error}"),
        }
    }
    Err(last_error)
}

/// 去掉本体绝对路径里的 `alg_data` 根前缀，得到文件服务的相对路径
fn strip_alg_root(path: &str) -> String {
    let trimmed = path.trim().trim_end_matches('/');
    for root in ["/ota/alg_data", "/userdata/alg_data"] {
        if let Some(rest) = trimmed.strip_prefix(root) {
            return rest.to_string();
        }
    }
    trimmed.to_string()
}

/// `GET {base}/api/list?path=<相对根目录路径>`
async fn list_dir(
    http: &reqwest::Client,
    base: &str,
    api_path: &str,
) -> Result<Vec<FileEntry>, String> {
    let url = format!("{base}/api/list?path={api_path}");
    let resp = http
        .get(&url)
        .send()
        .await
        .map_err(|error| format!("{url} 请求失败: {error}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{url} → HTTP {status}: {}", text.trim()));
    }
    let list: ListResponse = serde_json::from_str(&text)
        .map_err(|error| format!("{url} 返回的不是目录列表 JSON: {error}"))?;
    if !list.ok {
        return Err(format!("{url} 返回 ok=false"));
    }
    Ok(list.entries)
}

/// 广度收集地图目录下的文件（深度受限，超大文件跳过）
async fn collect_pack_files(
    http: &reqwest::Client,
    base: &str,
    api_path: &str,
) -> Result<Vec<PackedFile>, String> {
    let mut collected: Vec<PackedFile> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut queue: Vec<(String, String, usize)> = vec![(api_path.to_string(), String::new(), 0)];

    'walk: while let Some((dir_api, rel_prefix, depth)) = queue.pop() {
        for entry in list_dir(http, base, &dir_api).await? {
            if entry.name.is_empty() {
                continue;
            }
            let rel = format!("{rel_prefix}{}", entry.name);
            if entry.kind == "dir" {
                if SKIP_DIR_PREFIXES
                    .iter()
                    .any(|prefix| entry.name.starts_with(prefix))
                {
                    info!("跳过目录 {rel}（逐帧点云等回传不需要的数据）");
                    continue;
                }
                if depth >= MAX_PACK_DEPTH {
                    warn!("目录层级超过 {MAX_PACK_DEPTH}，跳过 {rel}");
                    continue;
                }
                let child = if entry.path.is_empty() {
                    format!("{}/{}", dir_api.trim_end_matches('/'), entry.name)
                } else {
                    format!("/{}", entry.path.trim_start_matches('/'))
                };
                queue.push((child, format!("{rel}/"), depth + 1));
                continue;
            }
            if entry.size > MAX_PACK_FILE_BYTES {
                info!(
                    "跳过超大文件 {rel}（{} MB > 上限 {} MB）",
                    entry.size / 1024 / 1024,
                    MAX_PACK_FILE_BYTES / 1024 / 1024
                );
                continue;
            }
            if collected.len() >= MAX_PACK_FILES {
                warn!("打包文件数达到上限 {MAX_PACK_FILES}，其余忽略");
                break 'walk;
            }
            if total_bytes + entry.size > MAX_PACK_TOTAL_BYTES {
                warn!(
                    "打包总量超过上限 {} MB，其余忽略",
                    MAX_PACK_TOTAL_BYTES / 1024 / 1024
                );
                break 'walk;
            }
            total_bytes += entry.size;
            collected.push(PackedFile {
                rel,
                path: entry.path.clone(),
            });
        }
    }

    collected.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(collected)
}

/// 逐个下载并写入内存 zip
async fn pack_files(
    http: &reqwest::Client,
    base: &str,
    files: &[PackedFile],
) -> Result<Vec<u8>, String> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut raw_bytes = 0usize;

    for file in files {
        let url = format!("{base}/files/{}", file.path.trim_start_matches('/'));
        let resp = http
            .get(&url)
            .send()
            .await
            .map_err(|error| format!("{url} 请求失败: {error}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("{url} → HTTP {status}"));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|error| format!("{url} 下载失败: {error}"))?;
        raw_bytes += bytes.len();
        writer
            .start_file(file.rel.clone(), options)
            .map_err(|error| format!("写入 zip（{}）失败: {error}", file.rel))?;
        writer
            .write_all(&bytes)
            .map_err(|error| format!("写入 zip（{}）失败: {error}", file.rel))?;
        info!("打包 {}（{} 字节）", file.rel, bytes.len());
    }

    let cursor = writer
        .finish()
        .map_err(|error| format!("收尾 zip 失败: {error}"))?;
    let package = cursor.into_inner();
    info!(
        "本机打包完成：{} 个文件，原始 {} 字节 → zip {} 字节",
        files.len(),
        raw_bytes,
        package.len()
    );
    Ok(package)
}

fn is_zip(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[0] == b'P' && bytes[1] == b'K'
}

/// 登录业务服务换取 JWT
async fn login(http: &reqwest::Client, cfg: &IngestConfig) -> Result<String, String> {
    let url = format!("{}/api/auth/login", cfg.api_base.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .json(&json!({ "username": cfg.username, "password": cfg.password }))
        .send()
        .await
        .map_err(|e| format!("登录业务服务失败: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("登录业务服务失败 HTTP {status}: {text}"));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("登录响应不是 JSON: {e}"))?;
    value
        .get("data")
        .and_then(|d| d.get("token"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("登录响应缺少 token: {text}"))
}

/// 把地图包交给业务服务导流到中台
pub async fn ingest_map(
    http: &reqwest::Client,
    cfg: &IngestConfig,
    saved: &MappingSaved,
    package: &[u8],
) -> UploadOutcome {
    let Some(device_map_name) = saved.dir_name() else {
        return UploadOutcome::Failed {
            reason: "4001 未返回地图目录名，无法回传".to_string(),
        };
    };
    let map_name = cfg.map_name.clone().unwrap_or(device_map_name);

    let token = match login(http, cfg).await {
        Ok(token) => token,
        Err(e) => return UploadOutcome::Failed { reason: e },
    };

    let info = json!({
        "mapName": map_name,
        "parkId": cfg.park_id,
        "deviceId": cfg.device_id,
        "bind": cfg.bind,
        // 服务端据这个字段决定「命中就跳过」还是「覆盖中台同一条记录」
        "mode": if cfg.refresh { "refresh" } else { "incremental" },
        "mapType": "2D栅格",
        "remarks": format!(
            "机器人建图自动回传 · 本体路径 {} · {}",
            saved.map_path.as_deref().unwrap_or("-"),
            saved.message
        ),
        "source": "robot-mapping",
        "sourceRef": saved.map_path,
    });

    let part = match Part::bytes(package.to_vec())
        .file_name(format!("{map_name}.zip"))
        .mime_str("application/zip")
    {
        Ok(part) => part,
        Err(e) => {
            return UploadOutcome::Failed {
                reason: format!("构造上传体失败: {e}"),
            };
        }
    };
    let form = Form::new()
        .part("file", part)
        .text("mapInfo", info.to_string());

    let url = format!(
        "{}/api/map-versions/ingest",
        cfg.api_base.trim_end_matches('/')
    );
    let resp = http
        .post(&url)
        .bearer_auth(&token)
        .multipart(form)
        .send()
        .await;
    let resp = match resp {
        Ok(resp) => resp,
        Err(e) => {
            return UploadOutcome::Failed {
                reason: format!("上传业务服务失败: {e}"),
            };
        }
    };

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return UploadOutcome::Failed {
            reason: format!("业务服务返回 HTTP {status}: {text}"),
        };
    }

    match serde_json::from_str::<Value>(&text) {
        Ok(value) => {
            let skipped = value
                .get("data")
                .and_then(|d| d.get("skipped"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if skipped {
                return UploadOutcome::Skipped("sourceRef 已存在，跳过重复同步".to_string());
            }
            let map_id = value
                .get("data")
                .and_then(|d| d.get("mapId"))
                .and_then(Value::as_str)
                .or_else(|| {
                    value
                        .get("data")
                        .and_then(|d| d.get("mapVersionId"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default()
                .to_string();
            if map_id.is_empty() {
                info!("地图已提交业务服务，响应: {text}");
            }
            UploadOutcome::Uploaded { map_id }
        }
        Err(_) => UploadOutcome::Uploaded {
            map_id: text.chars().take(80).collect(),
        },
    }
}

/// 地图同步结果（中台设备页手动触发 / 设备定时轮询共用）
#[derive(Debug, Default, Clone)]
pub struct SyncReport {
    pub total: usize,
    /// 新增或已覆盖的图
    pub uploaded: usize,
    pub skipped: usize,
    /// 刷新模式下「本体与平台内容一致」的张数（包含在 `skipped` 里）
    pub unchanged: usize,
    pub failed: usize,
    pub errors: Vec<String>,
    pub refresh: bool,
    /// 预检就失败了（业务服务 / 中台不可达），一张图都没处理：
    /// 定时轮询不为它刷事件，免得后台每次重试都在事件流里留一条红字。
    pub aborted: bool,
}

impl SyncReport {
    pub fn summary(&self) -> String {
        let mut summary = if self.refresh {
            format!(
                "地图刷新完成：共 {} 张，新增/更新 {}，无需更新 {}，失败 {}",
                self.total, self.uploaded, self.unchanged, self.failed
            )
        } else {
            format!(
                "地图同步完成：共 {} 张，新增 {}，跳过 {}，失败 {}",
                self.total, self.uploaded, self.skipped, self.failed
            )
        };
        if !self.errors.is_empty() {
            summary.push_str(&format!("；失败原因：{}", self.errors.join("；")));
        }
        summary
    }

    /// 是否有实际动作（轮询时用它决定要不要往事件流里发消息，避免每 10 分钟刷一条）
    pub fn has_changes(&self) -> bool {
        self.uploaded > 0 || (self.failed > 0 && !self.aborted)
    }
}

/// 查询业务库中这台设备已经同步过的机器人地图路径（`source_ref`）。
///
/// 地图先跟设备、再跟园区：设备没绑定园区也能判重，避免重复下载大文件。
pub async fn synced_sources(
    http: &reqwest::Client,
    cfg: &IngestConfig,
) -> Result<Vec<String>, String> {
    let token = login(http, cfg).await?;
    let url = format!(
        "{}/api/map-versions/synced-sources?deviceId={}",
        cfg.api_base.trim_end_matches('/'),
        cfg.device_id
    );
    let resp = http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("查询已同步地图列表失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("查询已同步地图列表失败 HTTP {status}: {text}"));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("已同步地图列表响应不是 JSON: {e}"))?;
    Ok(value
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

/// 同步地图：按 4012 返回的地图列表逐张处理。
///
/// - 增量（`cfg.refresh == false`，设备定时轮询用）：只补业务库没有的图，
///   先拉 `/synced-sources` 判重，已同步的直接跳过，**绝不覆盖已有记录**；
/// - 刷新（`cfg.refresh == true`，中台/控制台点「同步地图」用）：不预检已同步列表，
///   每张图都交给服务端比「内容指纹」——内容变了才覆盖中台同一条 `dog_map`，
///   没变仍计入「未变化」而不浪费一次中台上传。
///
/// 园区不是必需的：设备没绑定园区时地图先按「设备地图」入库（`parkId` 为空），
/// 设备绑定人之后在控制台创建园区时再把自己的设备地图挑一张绑成基线。
pub async fn sync_robot_maps(
    http: &reqwest::Client,
    cfg: &IngestConfig,
    file_base: &str,
    maps: &[RobotMap],
) -> SyncReport {
    let mut report = SyncReport {
        total: maps.len(),
        refresh: cfg.refresh,
        ..Default::default()
    };
    // 刷新模式不预检：已同步列表只能告诉「平台有没有这张图」，
    // 回答不了「本体上的图变没变」，还是要逐张比对内容指纹。
    let synced = if cfg.refresh {
        Vec::new()
    } else {
        match synced_sources(http, cfg).await {
            Ok(synced) => synced,
            Err(error) => {
                report.failed = maps.len();
                report.aborted = true;
                report.errors.push(error);
                return report;
            }
        }
    };

    for map in maps {
        if synced.iter().any(|source| source == &map.path) {
            report.skipped += 1;
            info!("地图 {} 已同步过，跳过", map.path);
            continue;
        }
        let package = match fetch_map_package(http, file_base, &map.map_id, Some(&map.path)).await {
            Ok(package) => package,
            Err(error) => {
                report.failed += 1;
                report.errors.push(format!("{}: {error}", map.map_id));
                continue;
            }
        };
        let saved = MappingSaved {
            success: true,
            code: 0,
            message: if cfg.refresh {
                "中台手动同步（全量刷新）"
            } else {
                "中台手动同步"
            }
            .to_string(),
            map_path: Some(map.path.clone()),
            map_name: Some(map.map_id.clone()),
            raw: json!({ "source": "manual-sync" }),
        };
        // 手动同步不改写园区当前基线，也不使用全局覆盖名
        let mut map_cfg = cfg.clone();
        map_cfg.map_name = None;
        map_cfg.bind = false;
        match ingest_map(http, &map_cfg, &saved, &package).await {
            UploadOutcome::Uploaded { .. } => report.uploaded += 1,
            UploadOutcome::Skipped(reason) => {
                report.skipped += 1;
                info!("地图 {} 无需上传: {reason}", map.map_id);
                if cfg.refresh {
                    report.unchanged += 1;
                }
            }
            UploadOutcome::Failed { reason } => {
                report.failed += 1;
                report.errors.push(format!("{}: {reason}", map.map_id));
            }
        }
    }
    report
}

/// 构造一个适合长时间传输地图的 HTTP 客户端
pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .unwrap_or_default()
}

/// 本体文件服务基地址：显式配置优先，否则由 `--robot` 推导
pub fn resolve_file_base(robot: &str, explicit: Option<&str>, port: u16) -> String {
    if let Some(base) = explicit {
        return base.trim_end_matches('/').to_string();
    }
    // 从 ws://host:10010 或裸 host 里取出 host
    let host = robot
        .trim_start_matches("ws://")
        .trim_start_matches("wss://")
        .split('/')
        .next()
        .unwrap_or(robot)
        .split(':')
        .next()
        .unwrap_or(robot);
    format!("http://{host}:{port}")
}

/// 从业务平台下载最新地图包（`GET /api/map-versions/{id}/package`）。
///
/// 中台编辑地图后，业务平台会先把最新包刷新到自己的副本里，再由这里拉取。
pub async fn download_map_package(
    http: &reqwest::Client,
    cfg: &IngestConfig,
    map_id: &str,
) -> Result<Vec<u8>, String> {
    let token = login(http, cfg).await?;
    let url = format!(
        "{}/api/map-versions/{}/package",
        cfg.api_base.trim_end_matches('/'),
        map_id
    );
    let resp = http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|error| format!("{url} 请求失败: {error}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("下载地图包失败 HTTP {status}: {}", text.trim()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|error| format!("下载地图包失败: {error}"))?;
    if !is_zip(&bytes) {
        return Err(format!("下载到的地图包不是 zip（{} 字节）", bytes.len()));
    }
    Ok(bytes.to_vec())
}

/// 条目是否位于 `map_edit/` 目录（遥控器编辑后的地图）。
fn is_map_edit_path(name: &str) -> bool {
    name.split('/')
        .any(|segment| segment.eq_ignore_ascii_case("map_edit"))
}

/// 从地图包里取出本体需要的两个文件：`map.pgm` 与 `map.yaml`。
///
/// 中台/ROS 包常见是 `model.pgm` + `model.yaml`，本体只认 `map.pgm` + `map.yaml`，
/// 因此这里重命名，并把 yaml 的 `image:` 改写成 `map.pgm`（否则载图会找不到栅格）。
///
/// 机器人本体回传的包会同时带根目录 `map.pgm`（原始地图）与 `map_edit/map.pgm`
/// （遥控器编辑后的地图）。这里必须取 `map_edit/` 那一份——平台编辑与下发都只
/// 针对编辑版，根目录的原始图应该保留。
pub fn extract_robot_map_files(package: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    use std::io::Read;

    let mut archive = zip::ZipArchive::new(Cursor::new(package))
        .map_err(|error| format!("地图包解析失败: {error}"))?;
    let mut pgm_edit: Option<String> = None;
    let mut pgm_fallback: Option<String> = None;
    let mut yaml_edit: Option<String> = None;
    let mut yaml_fallback: Option<String> = None;
    let mut edit_info_edit: Option<String> = None;
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let lower = name.to_ascii_lowercase();
        let edited = is_map_edit_path(&lower);
        if lower.ends_with(".pgm") {
            if edited {
                pgm_edit.get_or_insert(name);
            } else {
                pgm_fallback.get_or_insert(name);
            }
        } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
            if edited {
                yaml_edit.get_or_insert(name);
            } else {
                yaml_fallback.get_or_insert(name);
            }
        } else if lower.rsplit('/').next() == Some("edit_info.json") && edited {
            edit_info_edit.get_or_insert(name);
        }
    }

    let pgm_name = pgm_edit
        .or(pgm_fallback)
        .ok_or_else(|| "地图包内没有 .pgm，无法写回本体".to_string())?;
    let yaml_name = yaml_edit
        .or(yaml_fallback)
        .ok_or_else(|| "地图包内没有 .yaml，无法写回本体".to_string())?;

    let mut pgm = Vec::new();
    archive
        .by_name(&pgm_name)
        .map_err(|error| format!("读取地图包内 {pgm_name} 失败: {error}"))?
        .read_to_end(&mut pgm)
        .map_err(|error| format!("读取地图包内 PGM 失败: {error}"))?;
    let mut yaml = String::new();
    archive
        .by_name(&yaml_name)
        .map_err(|error| format!("读取地图包内 {yaml_name} 失败: {error}"))?
        .read_to_string(&mut yaml)
        .map_err(|error| format!("读取地图包内 YAML 失败: {error}"))?;

    let files = vec![
        ("map.pgm".to_string(), pgm),
        (
            "map.yaml".to_string(),
            rewrite_yaml_image(&yaml).into_bytes(),
        ),
    ];
    let mut files = files;
    // 平台的橡皮擦笔画（后端已写进包内的 `map_edit/edit_info.json`）：
    // 单拿出来交给 `push_map_to_robot` 与本体现有的编辑记录**合并**后写入，
    // 不能像栅格那样直接覆盖——那是遥控器的编辑历史。
    if let Some(name) = edit_info_edit {
        let mut bytes = Vec::new();
        if let Ok(mut entry) = archive.by_name(&name)
            && entry.read_to_end(&mut bytes).is_ok()
            && !bytes.is_empty()
        {
            files.push(("edit_info.json".to_string(), bytes));
        }
    }
    Ok(files)
}

/// 把 ROS yaml 的 `image:` 指到本体的 `map.pgm`（其余字段原样保留）。
fn rewrite_yaml_image(yaml: &str) -> String {
    let mut out = String::new();
    let mut found = false;
    for line in yaml.lines() {
        if line.trim_start().starts_with("image:") {
            out.push_str("image: map.pgm");
            found = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        out = format!("image: map.pgm\n{out}");
    }
    out
}

/// 编辑记录里登记的擦除圆上限：防止反复下发把文件撑大（超了就停止追加）。
const MAX_EDIT_INFO_CIRCLES: usize = 20_000;
/// 编辑记录体积上限：超了就不写，保留本体原文件。
const MAX_EDIT_INFO_BYTES: usize = 8 * 1024 * 1024;

/// 擦除圆的去重键：保留两位小数后相同的圆视为同一条（同一笔反复下发不累积）。
fn circle_key(x: f64, y: f64, r: f64) -> (i64, i64, i64) {
    (
        (x * 100.0).round() as i64,
        (y * 100.0).round() as i64,
        (r * 100.0).round() as i64,
    )
}

fn circle_of(value: &Value) -> Option<(i64, i64, i64)> {
    let x = value.get("x")?.as_f64()?;
    let y = value.get("y")?.as_f64()?;
    let r = value.get("r")?.as_f64()?;
    Some(circle_key(x, y, r))
}

/// 遥控器「没有编辑」时的空 edit_info（与它自己写出来的结构一致）。
const EMPTY_EDIT_INFO: &[u8] = b"{\n  \"erase_areas\": [],\n  \"forbidden_areas\": [],\n  \"fences\": [],\n  \"edit_ops\": []\n}";

/// 包里的编辑版栅格是不是就是建图原图（中台点了「恢复原图」之后下发的那份）。
///
/// 判据：`map_edit/` 下的 pgm 与根目录 pgm（`map.pgm` / `model.pgm`）**逐字节相同**，
/// 且 `edit_info.json` 里没有任何擦除圆 / 编辑动作。
///
/// 为真时客户端要**覆盖**本体的编辑记录（而不是合并）——否则遥控器下次保存会按
/// 旧 ops 把已经作废的擦除又画回来，「恢复原图」等于没做。
pub fn package_uses_original_raster(package: &[u8]) -> bool {
    use std::io::Read;

    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(package)) else {
        return false;
    };
    let mut edit_pgm: Option<String> = None;
    let mut root_pgm: Option<String> = None;
    let mut edit_info: Option<String> = None;
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let lower = name.to_ascii_lowercase();
        let base = lower
            .rsplit('/')
            .next()
            .unwrap_or(lower.as_str())
            .to_string();
        if base == "edit_info.json" {
            if is_map_edit_path(&lower) {
                edit_info.get_or_insert(name);
            }
            continue;
        }
        if !lower.ends_with(".pgm") {
            continue;
        }
        if is_map_edit_path(&lower) {
            edit_pgm.get_or_insert(name);
        } else if base == "map.pgm" || base == "model.pgm" {
            root_pgm.get_or_insert(name);
        }
    }
    let (Some(edit_pgm), Some(root_pgm)) = (edit_pgm, root_pgm) else {
        return false;
    };
    let mut read = |name: &str| -> Option<Vec<u8>> {
        let mut out = Vec::new();
        archive.by_name(name).ok()?.read_to_end(&mut out).ok()?;
        Some(out)
    };
    let (Some(edited), Some(original)) = (read(&edit_pgm), read(&root_pgm)) else {
        return false;
    };
    if edited != original {
        return false;
    }
    let Some(info) = edit_info.as_deref().and_then(&mut read) else {
        // 没有编辑记录 = 没编辑过，同样是“原图”
        return true;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&info) else {
        return false;
    };
    let empty = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_array)
            .is_none_or(|items| items.is_empty())
    };
    empty("erase_areas") && empty("edit_ops") && empty("forbidden_areas") && empty("fences")
}

/// 把平台这次的擦除圆合并进本体的 `map_edit/edit_info.json`。
///
/// 遥控器的编辑记录结构（逆向自它自己写的文件）：
/// `{ erase_areas: [{x,y,r}], forbidden_areas: [], fences: [], edit_ops: [{type,circles}] }`，
/// 单位是栅格像素。它保存地图时按「根目录 `map.pgm` + 这些擦除圆」重算栅格，
/// 所以平台的擦除必须记进来，否则它一保存就把平台的编辑整体冲掉
/// （2026-11-25 现场 11:14 / 18:29 / 19:10 / 19:24 四次复现）。
///
/// 规则：
/// - **追加不覆盖**：本体已有的圆（遥控器自己的）与 `fences` / `forbidden_areas`
///   等平台不产生的字段一律原样保留；
/// - 没有新增内容时原样返回本体字节（调用方会据此跳过写入）；
/// - 本体那份解析不了 → 返回 `None`，保留原文件（宁可不同步也不能写坏它）。
pub fn merge_edit_info(existing: Option<&[u8]>, incoming: &[u8]) -> Option<Vec<u8>> {
    let incoming_value: Value = serde_json::from_slice(incoming).ok()?;
    let incoming_object = incoming_value.as_object()?;
    // 本体上还没有这份编辑记录（这张图没在遥控器里编辑过）：平台这次的擦除
    // 也要建档，否则遥控器以后一保存照样把平台的编辑冲掉。
    let creating = existing.is_none_or(|bytes| bytes.is_empty());
    let base_value = match existing {
        Some(bytes) if !bytes.is_empty() => {
            let parsed: Value = serde_json::from_slice(bytes).ok()?;
            if !parsed.is_object() {
                return None;
            }
            parsed
        }
        _ => incoming_value.clone(),
    };
    let mut root = base_value.as_object()?.clone();
    for key in ["erase_areas", "forbidden_areas", "fences", "edit_ops"] {
        match root.get(key) {
            None => {
                root.insert(key.to_string(), Value::Array(Vec::new()));
            }
            Some(Value::Array(_)) => {}
            // 结构对不上就不碰
            Some(_) => return None,
        }
    }

    let mut known: std::collections::HashSet<(i64, i64, i64)> = root
        .get("erase_areas")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(circle_of).collect())
        .unwrap_or_default();

    let has_incoming = incoming_object
        .get("erase_areas")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
        || incoming_object
            .get("edit_ops")
            .and_then(Value::as_array)
            .is_some_and(|ops| !ops.is_empty());

    let mut added: Vec<Value> = Vec::new();
    for circle in incoming_object
        .get("erase_areas")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(key) = circle_of(circle) else {
            continue;
        };
        if known.contains(&key) {
            continue;
        }
        if known.len() + added.len() >= MAX_EDIT_INFO_CIRCLES {
            break;
        }
        known.insert(key);
        added.push(circle.clone());
    }

    let existing_ops: std::collections::HashSet<String> = root
        .get("edit_ops")
        .and_then(Value::as_array)
        .map(|ops| ops.iter().map(Value::to_string).collect())
        .unwrap_or_default();
    let mut new_ops: Vec<Value> = Vec::new();
    for op in incoming_object
        .get("edit_ops")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if existing_ops.contains(&op.to_string()) {
            continue;
        }
        new_ops.push(op.clone());
    }

    if added.is_empty() && new_ops.is_empty() {
        if creating && has_incoming {
            return serde_json::to_vec_pretty(&Value::Object(root.clone()))
                .ok()
                .filter(|bytes| bytes.len() <= MAX_EDIT_INFO_BYTES);
        }
        return existing.map(<[u8]>::to_vec);
    }

    if let Some(Value::Array(areas)) = root.get_mut("erase_areas") {
        areas.extend(added);
    }
    if let Some(Value::Array(ops)) = root.get_mut("edit_ops") {
        ops.extend(new_ops);
    }

    let merged = serde_json::to_vec_pretty(&Value::Object(root)).ok()?;
    if merged.len() > MAX_EDIT_INFO_BYTES {
        return None;
    }
    Some(merged)
}

/// 本体地图目录名合法性：只允许单段目录名，避免写到 `history_map` 之外。
fn sanitize_map_dir(map_dir: &str) -> Result<String, String> {
    let dir = map_dir
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim();
    if dir.is_empty() || dir == "." || dir == ".." || dir.contains('\0') {
        return Err(format!("本体地图目录名无效：{map_dir}"));
    }
    if dir.contains('\\') || dir.contains("..") {
        return Err(format!("本体地图目录名不安全：{map_dir}"));
    }
    Ok(dir.to_string())
}

fn files_url(base: &str, rel: &str) -> String {
    format!(
        "{}/files/{}",
        base.trim_end_matches('/'),
        rel.trim_start_matches('/')
    )
}

/// 读本体文件服务上的单个文件；不存在/失败返回 None。
async fn get_robot_file(http: &reqwest::Client, base: &str, rel: &str) -> Option<Vec<u8>> {
    let resp = http.get(files_url(base, rel)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.bytes().await.ok().map(|bytes| bytes.to_vec())
}

/// 写本体文件服务并按读回内容校验。
///
/// 现场固件的上传接口没有正式文档，这里按「多形状探测 + 读回校验」处理：
/// 任一形状写成功后，都会用 `GET /files/<rel>` 读回比对，只有内容一致才算成功；
/// 不一致继续换下一种形状。返回的 `Err` 里带每种形状的失败原因，便于现场排障。
async fn upload_robot_file(
    http: &reqwest::Client,
    base: &str,
    rel: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let rel = rel.trim_start_matches('/');
    let file_name = rel.rsplit('/').next().unwrap_or("map.pgm");
    let dir = rel.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    let url_files = files_url(base, rel);
    let url_upload = format!("{}/upload", base.trim_end_matches('/'));
    let mut errors: Vec<String> = Vec::new();

    // ① 文档约定的 POST /upload（multipart）：字段名现场固件未公开，多带几个别名
    let mut form = reqwest::multipart::Form::new()
        .text("path", rel.to_string())
        .text("dir", format!("/{dir}"))
        .text("name", file_name.to_string())
        .text("filename", file_name.to_string());
    if let Ok(part) = reqwest::multipart::Part::bytes(bytes.to_vec())
        .file_name(file_name.to_string())
        .mime_str("application/octet-stream")
    {
        form = form.part("file", part);
    }
    match http.post(&url_upload).multipart(form).send().await {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success()
                && get_robot_file(http, base, rel).await.as_deref() == Some(bytes)
            {
                info!("地图文件已写入本体（POST /upload）: {rel}");
                return Ok(());
            }
            errors.push(format!("POST /upload → HTTP {status} {}", text.trim()));
        }
        Err(error) => errors.push(format!("POST /upload 请求失败: {error}")),
    }

    // ② PUT /files/<rel>（静态文件服务常见写法）
    match http
        .put(&url_files)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .body(bytes.to_vec())
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status();
            let _ = resp.text().await;
            if status.is_success()
                && get_robot_file(http, base, rel).await.as_deref() == Some(bytes)
            {
                info!("地图文件已写入本体（PUT /files）: {rel}");
                return Ok(());
            }
            errors.push(format!("PUT /files → HTTP {status}"));
        }
        Err(error) => errors.push(format!("PUT /files 请求失败: {error}")),
    }

    // ③ POST /files/<rel>（部分实现只开放 POST）
    match http.post(&url_files).body(bytes.to_vec()).send().await {
        Ok(resp) => {
            let status = resp.status();
            let _ = resp.text().await;
            if status.is_success()
                && get_robot_file(http, base, rel).await.as_deref() == Some(bytes)
            {
                info!("地图文件已写入本体（POST /files）: {rel}");
                return Ok(());
            }
            errors.push(format!("POST /files → HTTP {status}"));
        }
        Err(error) => errors.push(format!("POST /files 请求失败: {error}")),
    }

    Err(format!(
        "写本体文件 {rel} 失败（已尝试 3 种上传形状：{}）",
        errors.join("；")
    ))
}

/// 把地图包里的 PGM / YAML 写回本体地图目录，并读回校验。
///
/// 目标目录：本体上存在 `map_edit/` 时写 `map_edit/map.pgm` + `map_edit/map.yaml`
/// （遥控器编辑后的地图），根目录的原始 `map.pgm` / `map.yaml` 保持不动；
/// 没有 `map_edit/` 的老固件才退回根目录。`map.pcd` / `map.geojson` / `nav_paths`
/// 这些定位与路网数据与栅格噪点无关，始终原样保留，避免破坏已有路线。
pub async fn push_map_to_robot(
    http: &reqwest::Client,
    file_base: &str,
    map_dir: &str,
    package: &[u8],
) -> Result<String, String> {
    let dir = sanitize_map_dir(map_dir)?;
    // 目录必须已经存在于本体：宁可报错，也不能凭猜测往 history_map 里造目录
    let api_path = format!("/map/history_map/{dir}");
    list_dir(http, file_base, &api_path)
        .await
        .map_err(|error| format!("本体上没有地图目录 {dir}（{error}），无法写回"))?;

    // 本体有 map_edit/ 时只改编辑版，绝不覆盖根目录的建图原始地图
    let has_map_edit = list_dir(http, file_base, &format!("{api_path}/map_edit"))
        .await
        .is_ok();
    let target_dir = if has_map_edit {
        format!("map/history_map/{dir}/map_edit")
    } else {
        format!("map/history_map/{dir}")
    };
    let target_label = if has_map_edit {
        format!("{dir}/map_edit")
    } else {
        dir.clone()
    };

    let files = extract_robot_map_files(package)?;
    // `edit_info.json` 不能像栅格那样直接覆盖：它是遥控器的编辑历史
    // （它保存地图时按「根 map.pgm + 这些擦除圆」重算栅格）。这里把平台
    // 这次的擦除圆追加进去，否则遥控器一保存就把平台的编辑整体冲掉。
    // 中台「恢复原图」下发的那份：编辑版栅格 == 建图原图 → 编辑记录要**覆盖**成空，
    // 不然遥控器下次保存会按旧 ops 把作废的擦除画回来。
    let reset_edit_info = has_map_edit && package_uses_original_raster(package);
    let mut payloads: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, bytes) in files {
        if name != "edit_info.json" {
            payloads.push((name, bytes));
            continue;
        }
        if !has_map_edit {
            // 老固件没有 `map_edit/`：编辑记录没有落点，不往根目录塞
            continue;
        }
        if reset_edit_info {
            info!("本体编辑记录将按「恢复原图」清空（不再合并旧擦除）");
            payloads.push((name, bytes));
            continue;
        }
        let rel = format!("{target_dir}/edit_info.json");
        let existing = get_robot_file(http, file_base, &rel).await;
        match merge_edit_info(existing.as_deref(), &bytes) {
            Some(merged) => payloads.push((name, merged)),
            None => warn!("本体 {rel} 无法解析，跳过编辑记录合并（保留原文件不动）"),
        }
    }
    if reset_edit_info && !payloads.iter().any(|(name, _)| name == "edit_info.json") {
        payloads.push(("edit_info.json".to_string(), EMPTY_EDIT_INFO.to_vec()));
    }
    let files = payloads;
    let mut written: Vec<String> = Vec::new();
    let mut originals: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    for (name, bytes) in &files {
        let rel = format!("{target_dir}/{name}");
        let original = get_robot_file(http, file_base, &rel).await;
        originals.push((rel.clone(), original.clone()));
        if original.as_deref() == Some(bytes.as_slice()) {
            info!("本体地图文件已是最新，跳过: {rel}");
            continue;
        }
        if let Err(error) = upload_robot_file(http, file_base, &rel, bytes).await {
            // 回滚已经写过的文件，避免本体上 PGM / YAML 版本不一致
            for (rel, original) in originals.iter().rev() {
                if let Some(original) = original
                    && let Err(rollback) = upload_robot_file(http, file_base, rel, original).await
                {
                    warn!("回滚 {rel} 失败: {rollback}");
                }
            }
            return Err(error);
        }
        written.push(format!("{name}({} 字节)", bytes.len()));
    }

    if written.is_empty() {
        Ok(format!("本体地图 {target_label} 已是最新，无需写入"))
    } else {
        Ok(format!(
            "本体地图 {target_label} 已更新：{}",
            written.join("、")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_base_wins_and_trims_slash() {
        assert_eq!(
            resolve_file_base("192.168.1.9", Some("http://files.local/maps/"), 8080),
            "http://files.local/maps"
        );
    }

    #[test]
    fn strips_alg_data_root_for_fileserver_paths() {
        assert_eq!(
            strip_alg_root("/ota/alg_data/map/history_map/2026_11_10_19_37_25"),
            "/map/history_map/2026_11_10_19_37_25"
        );
        assert_eq!(
            strip_alg_root("/userdata/alg_data/map/history_map/m1/"),
            "/map/history_map/m1"
        );
        // 已经是相对路径时原样返回
        assert_eq!(strip_alg_root("map/history_map/m1"), "map/history_map/m1");
    }

    #[test]
    fn derives_base_from_ws_url() {
        assert_eq!(
            resolve_file_base("ws://192.168.1.9:10010/control", None, 12000),
            "http://192.168.1.9:12000"
        );
    }

    #[test]
    fn derives_base_from_bare_host() {
        assert_eq!(
            resolve_file_base("192.168.1.9", None, 12000),
            "http://192.168.1.9:12000"
        );
    }

    #[test]
    fn derives_base_from_host_with_port() {
        assert_eq!(
            resolve_file_base("192.168.1.9:10010", None, 9000),
            "http://192.168.1.9:9000"
        );
    }

    /// ROS 包里的 model.* 必须重命名成本体认的 map.*，且 yaml 的 image 跟着改
    #[test]
    fn extract_renames_ros_files_to_body_names() {
        use std::io::Write;

        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut buffer);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file("model.pgm", options).unwrap();
            writer.write_all(b"P5\n2 2\n255\n\x00\xfe\xfe\xfe").unwrap();
            writer.start_file("model.yaml", options).unwrap();
            writer
                .write_all(b"image: model.pgm\nresolution: 0.05\nnegate: 0\n")
                .unwrap();
            writer.finish().unwrap();
        }

        let files = extract_robot_map_files(&buffer.into_inner()).expect("package must parse");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "map.pgm");
        assert_eq!(files[1].0, "map.yaml");
        let yaml = String::from_utf8(files[1].1.clone()).unwrap();
        assert!(yaml.contains("image: map.pgm"));
        assert!(!yaml.contains("model.pgm"));
        // 其余字段原样保留
        assert!(yaml.contains("resolution: 0.05"));
    }

    #[test]
    fn extract_prefers_map_edit_over_original() {
        use std::io::Write;

        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut buffer);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file("map.pgm", options).unwrap();
            writer.write_all(b"P5\n2 2\n255\n\xff\xff\xff\xff").unwrap();
            writer.start_file("map.yaml", options).unwrap();
            writer
                .write_all(b"image: map.pgm\nresolution: 0.05\narc: []\n")
                .unwrap();
            writer.start_file("map_edit/map.pgm", options).unwrap();
            writer.write_all(b"P5\n2 2\n255\n\x00\xfe\xfe\xfe").unwrap();
            writer.start_file("map_edit/map.yaml", options).unwrap();
            writer
                .write_all(b"image: map.pgm\nresolution: 0.05\n")
                .unwrap();
            writer.finish().unwrap();
        }

        let files = extract_robot_map_files(&buffer.into_inner()).expect("package must parse");
        assert_eq!(files[0].0, "map.pgm");
        assert_eq!(files[0].1, b"P5\n2 2\n255\n\x00\xfe\xfe\xfe");
        // yaml 也取编辑版：根目录的 arc 字段不应被带进编辑版
        let yaml = String::from_utf8(files[1].1.clone()).unwrap();
        assert!(yaml.contains("image: map.pgm"));
        assert!(!yaml.contains("arc:"));
    }

    #[test]
    fn map_edit_detection_ignores_backup_dirs() {
        assert!(is_map_edit_path("map_edit/map.pgm"));
        assert!(is_map_edit_path("foo/MAP_EDIT/map.yaml"));
        assert!(!is_map_edit_path("map_edit.bak/map.pgm"));
        assert!(!is_map_edit_path("map.pgm"));
    }

    #[test]
    fn rejects_package_without_pgm() {
        use std::io::Write;

        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            writer.start_file("model.yaml", options).unwrap();
            writer.write_all(b"image: model.pgm\n").unwrap();
            writer.finish().unwrap();
        }
        assert!(extract_robot_map_files(&buffer.into_inner()).is_err());
    }

    #[test]
    fn sanitizes_map_dir_to_single_segment() {
        assert_eq!(
            sanitize_map_dir("/ota/alg_data/map/history_map/2026_11_10_16_15_06/").unwrap(),
            "2026_11_10_16_15_06"
        );
        assert!(sanitize_map_dir("").is_err());
        assert!(sanitize_map_dir("..").is_err());
        assert!(sanitize_map_dir("../..").is_err());
        // 多层路径只取最后一段，不能借此写穿 history_map
        assert_eq!(sanitize_map_dir("a/../b").unwrap(), "b");
    }

    /// 包里的 `map_edit/edit_info.json` 要一起取出来（交给 push 时合并）
    #[test]
    fn extract_carries_edit_info_from_map_edit() {
        use std::io::Write;

        let info = br#"{"erase_areas":[{"x":1.0,"y":2.0,"r":3.0}],"forbidden_areas":[],"fences":[],"edit_ops":[]}"#;
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            writer.start_file("map_edit/map.pgm", options).unwrap();
            writer.write_all(b"P5\n2 2\n255\n\x00\xfe\xfe\xfe").unwrap();
            writer.start_file("map_edit/map.yaml", options).unwrap();
            writer.write_all(b"image: map.pgm\n").unwrap();
            writer
                .start_file("map_edit/edit_info.json", options)
                .unwrap();
            writer.write_all(info).unwrap();
            writer.finish().unwrap();
        }

        let files = extract_robot_map_files(&buffer.into_inner()).expect("package must parse");
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["map.pgm", "map.yaml", "edit_info.json"]);
        let (_, bytes) = files
            .iter()
            .find(|(name, _)| name == "edit_info.json")
            .unwrap();
        assert_eq!(bytes.as_slice(), info);
    }

    const REMOTE_EDIT_INFO: &[u8] = br#"{
  "erase_areas": [{"x": 1.0, "y": 1.0, "r": 0.5}],
  "forbidden_areas": [],
  "fences": [{"id": "fence-1"}],
  "edit_ops": [],
  "extra": "keep-me"
}"#;

    #[test]
    fn merge_edit_info_appends_circles_and_keeps_remote_fields() {
        let incoming = br#"{
  "erase_areas": [{"x": 2.0, "y": 2.0, "r": 1.0}, {"x": 3.0, "y": 3.0, "r": 1.0}],
  "forbidden_areas": [],
  "fences": [],
  "edit_ops": [{"type": "erase", "circles": [{"x": 2.0, "y": 2.0, "r": 1.0}]}]
}"#;
        let merged = merge_edit_info(Some(REMOTE_EDIT_INFO), incoming).expect("must merge");
        let value: Value = serde_json::from_slice(&merged).unwrap();
        let areas = value["erase_areas"].as_array().unwrap();
        assert_eq!(areas.len(), 3, "遥控器原有的 1 个圆 + 本次 2 个圆");
        assert_eq!(areas[0]["x"], 1.0);
        assert_eq!(areas[2]["y"], 3.0);
        // 遥控器的围栏与未知字段原样保留
        assert_eq!(value["fences"].as_array().unwrap().len(), 1);
        assert_eq!(value["extra"], "keep-me");
        assert_eq!(value["edit_ops"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn merge_edit_info_is_idempotent() {
        let incoming = br#"{"erase_areas":[{"x":2.0,"y":2.0,"r":1.0}],"forbidden_areas":[],"fences":[],"edit_ops":[]}"#;
        let first = merge_edit_info(Some(REMOTE_EDIT_INFO), incoming).expect("must merge");
        let second = merge_edit_info(Some(&first), incoming).expect("must merge");
        assert_eq!(first, second, "同一笔反复下发不能重复累积");
    }

    #[test]
    fn merge_edit_info_creates_when_body_has_none() {
        let incoming = br#"{"erase_areas":[{"x":2.0,"y":2.0,"r":1.0}],"forbidden_areas":[],"fences":[],"edit_ops":[]}"#;
        let merged = merge_edit_info(None, incoming).expect("must create");
        let value: Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(value["erase_areas"].as_array().unwrap().len(), 1);
        assert!(value["forbidden_areas"].as_array().unwrap().is_empty());
    }

    /// 本体那份读不懂时不能动它（宁可不同步也不能写坏遥控器的编辑记录）
    #[test]
    fn merge_edit_info_refuses_broken_body_file() {
        let incoming = br#"{"erase_areas":[{"x":2.0,"y":2.0,"r":1.0}],"forbidden_areas":[],"fences":[],"edit_ops":[]}"#;
        assert!(merge_edit_info(Some(b"{ not json"), incoming).is_none());
        assert!(merge_edit_info(Some(b"[1,2,3]"), incoming).is_none());
        // 字段结构对不上（erase_areas 不是数组）也不碰
        assert!(merge_edit_info(Some(br#"{"erase_areas": 1}"#), incoming).is_none());
    }

    /// 没有新增内容时原样返回本体字节，避免每次下发都重写文件
    #[test]
    fn merge_edit_info_returns_body_bytes_when_nothing_new() {
        let incoming = br#"{"erase_areas":[{"x":1.00,"y":1.001,"r":0.5}],"forbidden_areas":[],"fences":[],"edit_ops":[]}"#;
        let merged = merge_edit_info(Some(REMOTE_EDIT_INFO), incoming).expect("must merge");
        assert_eq!(merged, REMOTE_EDIT_INFO);
    }

    fn build_zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        use std::io::Write;
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    const EMPTY_INFO: &[u8] =
        br#"{"erase_areas":[],"forbidden_areas":[],"fences":[],"edit_ops":[]}"#;

    /// 中台「恢复原图」下发的包：编辑版栅格 == 建图原图 且没有编辑记录
    #[test]
    fn package_is_original_when_edit_equals_root() {
        let pgm = b"P5\n2 2\n255\n\xff\xff\xff\xff".to_vec();
        let package = build_zip(&[
            ("map.pgm", pgm.clone()),
            ("map_edit/map.pgm", pgm),
            ("map_edit/edit_info.json", EMPTY_INFO.to_vec()),
        ]);
        assert!(package_uses_original_raster(&package));
    }

    #[test]
    fn package_is_not_original_when_raster_differs() {
        let package = build_zip(&[
            ("map.pgm", b"P5\n2 2\n255\n\xff\xff\xff\xff".to_vec()),
            (
                "map_edit/map.pgm",
                b"P5\n2 2\n255\n\x00\xfe\xfe\xfe".to_vec(),
            ),
            ("map_edit/edit_info.json", EMPTY_INFO.to_vec()),
        ]);
        assert!(!package_uses_original_raster(&package));
    }

    /// 栅格一样但编辑记录里还有擦除圆 → 不是“原图”，仍然要合并
    #[test]
    fn package_is_not_original_when_edits_remain() {
        let pgm = b"P5\n2 2\n255\n\xff\xff\xff\xff".to_vec();
        let info = br#"{"erase_areas":[{"x":1.0,"y":1.0,"r":8.0}],"forbidden_areas":[],"fences":[],"edit_ops":[]}"#;
        let package = build_zip(&[
            ("map.pgm", pgm.clone()),
            ("map_edit/map.pgm", pgm),
            ("map_edit/edit_info.json", info.to_vec()),
        ]);
        assert!(!package_uses_original_raster(&package));
    }

    /// 没有 map_edit/（手工上传的 ROS 包）永远不算“恢复原图”
    #[test]
    fn package_without_edit_raster_is_not_original() {
        let package = build_zip(&[
            ("model.pgm", b"P5\n2 2\n255\n\xff\xff\xff\xff".to_vec()),
            ("model.yaml", b"image: model.pgm\n".to_vec()),
        ]);
        assert!(!package_uses_original_raster(&package));
    }
}
