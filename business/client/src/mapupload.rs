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

use crate::zhpro::mapping::MappingSaved;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};
use std::time::Duration;
use tracing::{info, warn};

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
    Err(format!("无法从本体获取地图包（{last_error}）"))
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
}
