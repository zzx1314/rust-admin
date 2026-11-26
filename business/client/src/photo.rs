//! 航点拍照：到点调板端 Agent `/api/photo` 抓 JPEG，任务收尾回传平台。
//!
//! 为什么拍照由客户端驱动：拍照的时机只有 5009 的状态机知道（到点 / 离开），
//! 而板端 Agent 只在内网可达（客户端与它同机 `127.0.0.1:8000`）。
//!
//! ```text
//! 路线动作「可见光拍照」→ plan.points[i] 的 photoCamera / photoFrames
//!                                    ↓ 到点（patrol.point_arrived / waiting）
//!                       板端 Agent POST /api/photo（ffmpeg 抓 1~N 张 JPEG）
//!                          → PHOTO_DIR/<计划>/pt<n>/<时间戳>-<相机>.jpg
//!                                    ↓ 任务终态 / 客户端启动
//!            GET /api/photo/list → POST 平台 /api/photos/ingest（multipart）
//!                          → 上传成功才 POST /api/photo/delete 删板端文件
//! ```
//!
//! 为什么照片来自**背包相机**（`dogcam` = 机器狗前置可见光，`cam0~cam3` = 背包 USB）：
//! 本体侧的「任务里的 `1` = 拍照」走的是上装 `/capture_image`，服务未就绪时 `14005`
//! 直接失败，而且本体没实现 5010/5011 记录查询（见 `docs/official-app-parity.md` §5.5）；
//! 背包相机本来就在跑（实时视频 / AI 抽帧同一路），抓一张图不依赖任何新协议。
//! 本体侧的 `tasks=[1]` 行为保持不变，两套互不影响。
//!
//! 三条约定（改这块先看）：
//!
//! 1. **拍照失败绝不影响巡检**：流没在推 / ffmpeg 缺失 / 磁盘满都只打日志，
//!    老版本 Agent（404）记一次 `unsupported` 本轮不再试。
//! 2. **到点那一帧同步等**：5009 每 500ms 一帧，`point_arrived` 按
//!    「(任务, 航点) 已经拍过」去重；抓拍是**同步 await** 的（不像开灯那样 spawn）——
//!    狗可能马上就走，晚一拍拍到的就是下一段路了。
//! 3. **上传成功才删板端文件**：弱网重传是常态，失败的文件留在板端等下一轮
//!    （任务终态 / 客户端启动都会触发一次回传）。

use crate::media_sync;
use reqwest::multipart;
use robodog_device_client::config::Config;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// 板端抓拍请求超时：等的是 RTSP 起流 + 解码首帧（实测 0.5~2s），12s 留足余量。
///
/// 比录像的 3s 大是因为这里**要等文件写出来**（录像只是起一个 ffmpeg 就返回）。
/// 再长就会把 5009 的处理链路拖住，宁可记一次失败。
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(12);
/// 单张照片上传超时（几百 KB，弱网也就几十秒）。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// 单张照片最多试几次（失败留在板端等下一轮）。
const UPLOAD_RETRIES: u32 = 3;

/// 默认抓拍源：机器狗前置可见光（路线动作没配 `photoCamera` 时）
pub const DEFAULT_CAMERA: &str = "dogcam";
/// 单点最多抓几张（与板端 `PHOTO_MAX_FRAMES` 对齐）
pub const MAX_FRAMES: u32 = 5;
/// 默认抓几张（老数据只有「可见光拍照」动作，没配张数）
pub const DEFAULT_FRAMES: u32 = 1;

/// 需要拍照的航点
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoPoint {
    /// 航点下标（0 基，与 `routeData.points` / 5009 `detail_index` 一致）
    pub index: i64,
    pub name: String,
    /// 抓拍源：`dogcam` / `cam0`~`cam3` / `dog-ipcam`
    pub camera: String,
    /// 抓几张（1~[`MAX_FRAMES`]）
    pub frames: u32,
}

/// 一条路线里的拍照配置
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhotoPlan {
    pub points: Vec<PhotoPoint>,
}

impl PhotoPlan {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn point_at(&self, index: i64) -> Option<&PhotoPoint> {
        self.points.iter().find(|point| point.index == index)
    }
}

/// 动作是不是「拍照」（`可见光拍照` / `抓拍`）。
///
/// 与「录像采集」区分开：本体侧的 `wants_photo` 把 `录像/摄像` 也算成拍照动作
/// （它只关心「这个点是不是作业点」），这里**不能**——否则每个录像航点都会多拍一张。
pub fn is_photo_action(action_type: &str) -> bool {
    let text = action_type.trim();
    if text.is_empty() || text.contains("录像") {
        return false;
    }
    text.contains("拍照") || text.contains("抓拍") || text.contains("相片")
}

fn camera_of(action: &Value) -> String {
    action
        .get("photoCamera")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_CAMERA)
        .to_string()
}

fn frames_of(action: &Value) -> u32 {
    let frames = action
        .get("photoFrames")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_FRAMES as i64);
    frames.clamp(1, MAX_FRAMES as i64) as u32
}

/// 解析 `routeData` 里所有「拍照」动作（同一个航点只认第一条）。
pub fn parse_photo_plan(route_data: &Value) -> PhotoPlan {
    let mut plan = PhotoPlan::default();
    let Some(points) = route_data.get("points").and_then(Value::as_array) else {
        return plan;
    };
    for (index, point) in points.iter().enumerate() {
        let Some(actions) = point.get("actions").and_then(Value::as_array) else {
            continue;
        };
        let Some(action) = actions.iter().find(|action| {
            action
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(is_photo_action)
        }) else {
            continue;
        };
        plan.points.push(PhotoPoint {
            index: index as i64,
            name: point
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("航点")
                .to_string(),
            camera: camera_of(action),
            frames: frames_of(action),
        });
    }
    plan
}

/// 板端 `/api/photo/list` 返回的一张待回传照片
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoFile {
    /// `<planId>/pt<n>/<文件名>.jpg`：上传成功后原样回传给 `/api/photo/delete`
    pub name: String,
    pub path: String,
    pub plan_id: String,
    pub point_index: i64,
    pub point_name: String,
    pub camera: String,
    pub size: u64,
    pub captured_at: String,
}

#[derive(Default)]
struct State {
    task_code: Option<String>,
    plan: PhotoPlan,
    /// 这条任务已经处理过的航点（5009 每 500ms 一帧，不去重会连拍）。
    ///
    /// 用集合而不是「上一个点」：返航 / 重发时同一个点会被再报一次，
    /// 单值会把它当成新点重新拍（一个点两张图，运维以为坏了）。
    handled_points: std::collections::HashSet<i64>,
    /// 这条任务已收尾：迟到的到点帧不再抓拍
    finished: bool,
    /// 板端 Agent 不支持拍照（404 / 老版本）：本轮不再尝试
    unsupported: bool,
}

/// 航点拍照运行态（进程内唯一）
pub struct Photographer {
    /// 板端 Agent 基地址（与 `ai_patrol` / `lights` / `record` 共用 `[ai] agent_url`）
    agent: String,
    enabled: bool,
    config: Arc<Config>,
    /// 抓拍用（超时短：它挡着 5009 的到点处理）
    agent_client: reqwest::Client,
    /// 上传用（照片几百 KB，弱网给足）
    upload_client: reqwest::Client,
    state: Mutex<State>,
    /// 同一时间只跑一轮回传（终态 / 启动 / 手动都只是触发，不叠加）
    uploading: AtomicBool,
}

impl Photographer {
    pub fn new(config: Arc<Config>, enabled: bool) -> Self {
        let agent = config.ai_agent_url.trim_end_matches('/').to_string();
        Self {
            agent,
            enabled,
            config,
            agent_client: reqwest::Client::builder()
                .timeout(CAPTURE_TIMEOUT)
                .build()
                .unwrap_or_default(),
            upload_client: crate::tls::http_builder()
                .timeout(UPLOAD_TIMEOUT)
                .build()
                .unwrap_or_default(),
            state: Mutex::new(State::default()),
            uploading: AtomicBool::new(false),
        }
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// 新任务开始：换掉拍照配置，清掉上一轮状态。
    pub async fn start_task(self: &Arc<Self>, task_code: &str, plan: PhotoPlan) {
        if !self.enabled {
            return;
        }
        if !plan.is_empty() {
            info!(
                "任务 {task_code} 含 {} 个拍照航点（{}）",
                plan.points.len(),
                plan.points
                    .iter()
                    .map(|point| format!("{}={}×{}", point.index, point.camera, point.frames))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        let mut state = self.state.lock().await;
        state.task_code = Some(task_code.to_string());
        state.plan = plan;
        state.handled_points.clear();
        state.finished = false;
    }

    /// 到点抓拍（5009 `patrol.point_arrived` / `patrol.waiting`）。
    ///
    /// 同一个点重复调用只拍一次；这个点没有拍照动作就什么都不做。
    pub async fn point_arrived(self: &Arc<Self>, task_code: &str, point_index: i64) {
        if !self.enabled {
            return;
        }
        let point = {
            let state = self.state.lock().await;
            if state.finished
                || state.unsupported
                || state.task_code.as_deref() != Some(task_code)
                || state.handled_points.contains(&point_index)
            {
                return;
            }
            state.plan.point_at(point_index).cloned()
        };
        let Some(point) = point else {
            // 没有拍照动作也记一笔：这个点以后不再重复判断
            self.state.lock().await.handled_points.insert(point_index);
            return;
        };
        // 先落「已拍」再发请求：抓拍是同步 await，5009 的下一帧可能已经排上了，
        // 不加这一句同一秒会连发好几次（每次 1~2s，直接卡住状态处理）
        {
            let mut state = self.state.lock().await;
            if state.finished
                || state.unsupported
                || state.task_code.as_deref() != Some(task_code)
                || state.handled_points.contains(&point_index)
            {
                return;
            }
            state.handled_points.insert(point_index);
        }
        let body = json!({
            "planId": task_code,
            "pointIndex": point_index,
            "pointName": point.name,
            "camera": point.camera,
            "frames": point.frames,
            "reason": "到达航点",
        });
        match self.post("/api/photo", body).await {
            Ok(value) => {
                let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(true);
                if !ok {
                    let error = value
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("板端拒绝拍照");
                    warn!(
                        "航点拍照：任务 {task_code} 航点 {point_index} 未拍（{error}）；巡检继续"
                    );
                    return;
                }
                // 张数先在宏外面算：`info!` 展开时会引入 `tracing::field::Value` 这个 trait，
                // 在宏参数里写 `Value::as_array` 会被解析成那个 trait（E0782）
                let saved = value
                    .get("files")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                info!(
                    "航点拍照：任务 {task_code} 航点 {point_index}（{}）拍下 {saved} 张（{}）",
                    point.name, point.camera
                );
            }
            Err(error) => {
                warn!(
                    "航点拍照：任务 {task_code} 航点 {point_index} 调板端失败（{error}）；巡检继续"
                );
                if error.contains("没有拍照接口") {
                    self.state.lock().await.unsupported = true;
                    warn!(
                        "板端 Agent 未升级拍照接口：本轮任务不再尝试拍照（升级 backpack-agent 后自动恢复）"
                    );
                }
            }
        }
    }

    /// 任务终态 / 取消 / 本地收尾：把板端积压的照片回传平台（后台跑）。
    pub async fn finish_task(self: &Arc<Self>, _task_code: &str, _reason: &str) {
        if !self.enabled {
            return;
        }
        {
            let mut state = self.state.lock().await;
            state.finished = true;
            state.handled_points.clear();
        }
        self.spawn_upload();
    }

    /// 客户端启动 / 指令触发：把板端积压的照片（上次没传完的）回传。
    pub fn sync_pending(self: &Arc<Self>) {
        if !self.enabled {
            return;
        }
        self.spawn_upload();
    }

    fn spawn_upload(self: &Arc<Self>) {
        if self.uploading.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(error) = this.upload_pending().await {
                warn!("航点照片回传失败（板端文件保留，下次任务/重启会重试）: {error}");
            }
            this.uploading.store(false, Ordering::SeqCst);
        });
    }

    async fn upload_pending(self: &Arc<Self>) -> Result<(), String> {
        let files = self.list_photos().await?;
        if files.is_empty() {
            return Ok(());
        }
        let api_base = self.config.resolve_api_base();
        let token = media_sync::login(&self.upload_client, &api_base, &self.config).await?;
        info!(
            "开始回传航点照片：{} 张（合计 {:.1}MB）",
            files.len(),
            files.iter().map(|file| file.size).sum::<u64>() as f64 / 1024.0 / 1024.0
        );
        let mut uploaded = 0usize;
        let mut failed = 0usize;
        for file in files {
            match self.upload_one(&api_base, &token, &file).await {
                Ok(()) => uploaded += 1,
                Err(error) => {
                    failed += 1;
                    warn!("照片上传失败（{}）: {error}", file.name);
                }
            }
        }
        info!("航点照片回传结束：成功 {uploaded} 张，失败 {failed} 张（失败的留在板端）");
        Ok(())
    }

    async fn upload_one(
        &self,
        api_base: &str,
        token: &str,
        file: &PhotoFile,
    ) -> Result<(), String> {
        let bytes = tokio::fs::read(&file.path)
            .await
            .map_err(|error| format!("读取板端照片失败（{}）: {error}", file.path))?;
        let meta = json!({
            "taskCode": file.plan_id,
            "deviceCode": self.config.device_id,
            "planId": file.plan_id,
            "pointIndex": file.point_index,
            "pointName": file.point_name,
            "camera": file.camera,
            "capturedAt": file.captured_at,
        });
        let url = format!("{}/api/photos/ingest", api_base.trim_end_matches('/'));
        let mut last_error = String::new();
        for attempt in 1..=UPLOAD_RETRIES {
            let part = multipart::Part::bytes(bytes.clone())
                .file_name("photo.jpg".to_string())
                .mime_str("image/jpeg")
                .map_err(|error| format!("构造上传体失败: {error}"))?;
            let form = multipart::Form::new()
                .part("file", part)
                .text("meta", meta.to_string());
            match self
                .upload_client
                .post(&url)
                .bearer_auth(token)
                .multipart(form)
                .send()
                .await
            {
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        // 上传成功才删板端文件（失败的要留给下一轮重试）
                        self.delete_on_board(&file.name).await;
                        return Ok(());
                    }
                    last_error = format!("HTTP {status}: {text}");
                }
                Err(error) => last_error = error.to_string(),
            }
            if attempt < UPLOAD_RETRIES {
                let wait = Duration::from_secs(5 * 3u64.pow(attempt - 1));
                tokio::time::sleep(wait).await;
            }
        }
        Err(last_error)
    }

    async fn delete_on_board(&self, name: &str) {
        match self
            .post("/api/photo/delete", json!({ "names": [name] }))
            .await
        {
            Ok(_) => {}
            Err(error) => warn!("删除板端照片失败（{name}）: {error}"),
        }
    }

    async fn list_photos(&self) -> Result<Vec<PhotoFile>, String> {
        let url = format!("{}/api/photo/list", self.agent);
        let resp = self
            .agent_client
            .get(&url)
            .send()
            .await
            .map_err(|error| format!("请求板端照片清单失败: {error}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err("板端 Agent 没有拍照接口（旧版本）".to_string());
        }
        if !status.is_success() {
            return Err(format!("板端照片清单返回 HTTP {status}: {text}"));
        }
        let value: Value =
            serde_json::from_str(&text).map_err(|error| format!("照片清单不是 JSON: {error}"))?;
        Ok(value
            .get("files")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(PhotoFile {
                            name: item.get("name")?.as_str()?.to_string(),
                            path: item.get("path")?.as_str()?.to_string(),
                            plan_id: item.get("planId")?.as_str()?.to_string(),
                            point_index: item
                                .get("pointIndex")
                                .and_then(Value::as_i64)
                                .unwrap_or(-1),
                            point_name: item
                                .get("pointName")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            camera: item
                                .get("camera")
                                .and_then(Value::as_str)
                                .unwrap_or(DEFAULT_CAMERA)
                                .to_string(),
                            size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
                            captured_at: item
                                .get("capturedAt")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let response = self
            .agent_client
            .post(format!("{}{path}", self.agent))
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("{path}: {error}"))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| format!("{path}: {error}"))?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err("板端 Agent 没有拍照接口（旧版本）".to_string());
        }
        if !status.is_success() {
            return Err(format!("{path} HTTP {status}: {text}"));
        }
        serde_json::from_str(&text).map_err(|error| format!("{path} 响应不是 JSON: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::extract::State;
    use axum::routing::{get, post};
    use robodog_device_client::config::{CliArgs, EnvConfig, FileConfig};

    type Calls = Arc<Mutex<Vec<Value>>>;

    fn test_config(agent: &str) -> Config {
        let mut config = Config::resolve(
            &FileConfig::default(),
            &EnvConfig::default(),
            &CliArgs {
                device_id: Some("TEST-DEVICE".to_string()),
                ..CliArgs::default()
            },
            None,
        )
        .expect("测试配置应能解析");
        config.ai_agent_url = agent.to_string();
        config
    }

    fn route() -> Value {
        json!({
            "points": [
                {"name": "P1", "actions": [{"type": "可见光拍照", "param": "1080P"}]},
                {"name": "P2", "actions": [{"type": "录像采集", "recordSeconds": 15}]},
                {
                    "name": "P3",
                    "actions": [{
                        "type": "可见光拍照",
                        "photoCamera": "cam2",
                        "photoFrames": 3
                    }]
                }
            ]
        })
    }

    #[test]
    fn photo_actions_are_parsed_with_camera_and_frames() {
        let plan = parse_photo_plan(&route());
        assert_eq!(plan.points.len(), 2, "录像动作不算拍照：{plan:?}");
        assert_eq!(plan.points[0].index, 0);
        assert_eq!(plan.points[0].camera, DEFAULT_CAMERA, "没配相机用 dogcam");
        assert_eq!(plan.points[0].frames, DEFAULT_FRAMES);
        assert_eq!(plan.points[1].index, 2);
        assert_eq!(plan.points[1].camera, "cam2");
        assert_eq!(plan.points[1].frames, 3);
    }

    #[test]
    fn frames_are_clamped_to_the_board_limit() {
        let plan = parse_photo_plan(&json!({
            "points": [
                {"name": "P1", "actions": [{"type": "可见光拍照", "photoFrames": 99}]},
                {"name": "P2", "actions": [{"type": "可见光拍照", "photoFrames": 0}]}
            ]
        }));
        assert_eq!(plan.points[0].frames, MAX_FRAMES);
        assert_eq!(plan.points[1].frames, 1, "0 张等于不拍，按 1 张处理");
        assert!(!is_photo_action("录像采集"), "录像不是拍照");
        assert!(!is_photo_action("全程录像"));
        assert!(is_photo_action("可见光拍照"));
        assert!(is_photo_action("抓拍"));
    }

    #[test]
    fn route_without_photo_actions_is_empty() {
        let plan = parse_photo_plan(&json!({
            "points": [{"name": "P1", "actions": [{"type": "AI 识别"}]}]
        }));
        assert!(plan.is_empty());
    }

    /// 到点拍一次（重复帧不重拍）→ 没配拍照动作的点不发请求 → 终态触发回传
    #[tokio::test]
    async fn captures_once_per_point_and_uploads_on_finish() {
        let calls: Calls = Default::default();
        let (agent, api) = spawn_agents(calls.clone()).await;
        let mut config = test_config(&agent);
        config.api_base = Some(api);
        let photographer = Arc::new(Photographer::new(Arc::new(config), true));

        photographer
            .start_task("TK-1", parse_photo_plan(&route()))
            .await;
        photographer.point_arrived("TK-1", 0).await;
        photographer.point_arrived("TK-1", 0).await; // 5009 重复帧
        photographer.point_arrived("TK-1", 1).await; // 没有拍照动作
        photographer.point_arrived("TK-1", 0).await;

        let calls_now = calls.lock().await.clone();
        let captures: Vec<&Value> = calls_now
            .iter()
            .filter(|call| call["path"] == "photo")
            .collect();
        assert_eq!(captures.len(), 1, "同一个点只拍一次：{calls_now:?}");
        assert_eq!(captures[0]["body"]["camera"], DEFAULT_CAMERA);
        assert_eq!(captures[0]["body"]["frames"], 1);
        assert_eq!(captures[0]["body"]["planId"], "TK-1");
        assert_eq!(captures[0]["body"]["pointName"], "P1");

        // 终态：上传清单里的照片 → 平台 ingest → 成功后删板端
        photographer.finish_task("TK-1", "任务终态").await;
        let calls_now = wait_for(&calls, 2).await;
        assert!(
            calls_now.iter().any(|call| call["path"] == "ingest"
                && call["body"]["meta"]["taskCode"] == "TK-1"
                && call["body"]["meta"]["pointIndex"] == 0),
            "终态要把照片回传平台：{calls_now:?}"
        );
        assert!(
            calls_now.iter().any(|call| call["path"] == "delete"
                && call["body"]["names"][0] == "TK-1/pt0/20261124-101200-123-dogcam.jpg"),
            "上传成功要删板端文件：{calls_now:?}"
        );
    }

    /// 板端没有拍照接口（老版本）：只记一次 unsupported，不再每帧都试
    #[tokio::test]
    async fn old_agent_disables_photos_for_the_task() {
        let photographer = Arc::new(Photographer::new(
            Arc::new(test_config("http://127.0.0.1:1")),
            true,
        ));
        photographer
            .start_task("TK-1", parse_photo_plan(&route()))
            .await;
        photographer.point_arrived("TK-1", 0).await;
        let state = photographer.state.lock().await;
        assert!(state.unsupported || state.handled_points.contains(&0));
    }

    async fn wait_for(calls: &Calls, expected: usize) -> Vec<Value> {
        for _ in 0..200 {
            if calls.lock().await.len() >= expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        calls.lock().await.clone()
    }

    /// 假板端 Agent（/api/photo、/api/photo/list、/api/photo/delete）+ 假业务平台
    /// （/api/auth/login、/api/photos/ingest）
    async fn spawn_agents(calls: Calls) -> (String, String) {
        let app = axum::Router::new()
            .route(
                "/api/photo",
                post(
                    |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(json!({"path": "photo", "body": body}));
                        Json(json!({"ok": true, "files": [{"name": "x.jpg"}]}))
                    },
                ),
            )
            .route(
                "/api/photo/list",
                get(|State(calls): State<Calls>| async move {
                    calls.lock().await.push(json!({"path": "list"}));
                    // 用临时文件当「板端照片」，upload_one 会读它
                    let dir = std::env::temp_dir().join("robodog-photo-test");
                    std::fs::create_dir_all(&dir).ok();
                    let path = dir.join("20261124-101200-123-dogcam.jpg");
                    std::fs::write(&path, [0xFFu8, 0xD8, 0xFF, 0xE0, 0x00]).ok();
                    Json(json!({"files": [{
                        "name": "TK-1/pt0/20261124-101200-123-dogcam.jpg",
                        "path": path.to_string_lossy(),
                        "planId": "TK-1",
                        "pointIndex": 0,
                        "pointName": "P1",
                        "camera": "dogcam",
                        "size": 5,
                        "capturedAt": "2026-11-24 10:12:00"
                    }]}))
                }),
            )
            .route(
                "/api/photo/delete",
                post(
                    |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(json!({"path": "delete", "body": body}));
                        Json(json!({"ok": true}))
                    },
                ),
            )
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let agent_port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let api = axum::Router::new()
            .route(
                "/api/auth/login",
                post(|| async { Json(json!({"code": 0, "data": {"token": "test-token"}})) }),
            )
            .route(
                "/api/photos/ingest",
                post(
                    |State(calls): State<Calls>,
                     headers: axum::http::HeaderMap,
                     body: axum::body::Bytes| async move {
                        // multipart 里的 meta 是 JSON，这里宽松地把它抠出来即可
                        let text = String::from_utf8_lossy(&body).to_string();
                        let meta = extract_meta(&text);
                        calls.lock().await.push(json!({
                            "path": "ingest",
                            "auth": headers
                                .get(axum::http::header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default(),
                            "body": {"meta": meta},
                        }));
                        Json(json!({"code": 0, "data": {"created": true}}))
                    },
                ),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假平台失败");
        let api_port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, api).await;
        });
        (
            format!("http://127.0.0.1:{agent_port}"),
            format!("http://127.0.0.1:{api_port}"),
        )
    }

    /// multipart 里的 `meta` 部分是一行 JSON：从 `name="meta"` 之后的第一个 `{`
    /// 取到最后一个 `}`（字段顺序由 serde 决定，不能按 key 找开头）
    fn extract_meta(text: &str) -> Value {
        let Some(section) = text.find("name=\"meta\"").map(|index| &text[index..]) else {
            return Value::Null;
        };
        let (Some(start), Some(end)) = (section.find('{'), section.rfind('}')) else {
            return Value::Null;
        };
        serde_json::from_str(&section[start..=end]).unwrap_or(Value::Null)
    }
}
