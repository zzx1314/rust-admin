//! 巡检录像：航点「录像采集」+ 航段「全程录像」。
//!
//! 为什么录像由客户端驱动：录像的起停时机只有 5009 的状态机知道（到点 / 离开 /
//! 终态），而板端 Agent 只在内网可达（客户端与它同机 `127.0.0.1:8000`）。
//!
//! ```text
//! 路线动作：航点「录像采集」→ plan.points[i].seconds   → 到点 Agent 开录，离开停
//! 路线动作：航段「全程录像」→ plan.task                 → 任务一开始就录，终态停（切片）
//!                                                    ↓
//!                               板端 Agent（record.py）ffmpeg → RECORD_DIR/<plan>/…mp4
//!                                                    ↓
//!        平台下发「回传录像」→ GET /api/record/list → POST 平台 /api/recordings/ingest
//!                                                    ↓
//!                               上传成功 → POST /api/record/delete 删板端文件
//! ```
//!
//! **回传是「平台按需」的（2026-11-25 起）**：任务终态 / 取消 / 客户端启动都**不再**
//! 自动往平台传，录像就攒在板端 `RECORD_DIR/<计划>/` 里，等平台下发 `回传录像`
//! 指令再传（与「同步媒体」同一套 `command_progress` / `command_result` 回执）。
//! 原因：现场主用 4G，一条计划动辄几百 MB，任务一结束就抢带宽会把下一轮巡检的
//! 地图 / 状态上报挤掉；而且那会儿多半没人看录像。
//!
//! 三条约定（改这块先看）：
//!
//! 1. **全程录像不跟航点**：配了「全程录像」就在 5009 一出现这条任务时开录，跨航点
//!    一直录到终态 / 取消；航点录像只在「这个点在录」的窗口内有效。
//! 2. **幂等**：5009 每 500ms 一帧，开录请求按「(任务, 航点) 是否已经在录」去重；
//!    全程录像按 `task_active` 去重。Agent 侧也做了一次幂等（重复 start 返回同一个 job）。
//! 3. **录像失败不影响巡检**：Agent 404 / 磁盘不足 / ffmpeg 起不来都只打日志，
//!    记为 `unsupported` 本轮不再重试，绝不往上抛错打断 5009 处理链路。

use crate::media_sync;
use reqwest::multipart;
use robodog_device_client::config::Config;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// 板端 Agent 的轻量请求超时（起停 ffmpeg 是毫秒级；等的只是 HTTP）。
///
/// 这些请求是在 5009 处理链路里**串行 await** 的（不能 spawn：`tokio::spawn` 不保证
/// 「到点开录」和「离开停录」的先后，反了就是「一直录到板端 -t 上限」）。同机 HTTP
/// 正常是毫秒级，所以超时压到 3s：Agent 挂了也只耽误这一帧，不会把状态上报拖 8s。
const AGENT_TIMEOUT: Duration = Duration::from_secs(3);
/// 单段录像上传超时：300MB 在 1Mbps 的 4G 上要 40 分钟，给足但不无限等。
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(1800);
/// 单个文件最多试几次（弱网一次失败很常见，失败的文件留在板端等下一轮）。
const UPLOAD_RETRIES: u32 = 3;
/// 全程录像切片默认值（秒）：与板端 `RECORD_SEGMENT_SECS` 一致，板端才是真上限。
pub const DEFAULT_SEGMENT_SECS: u32 = 300;
/// 航点录像默认时长（秒）：路线动作没配 `recordSeconds` 时用。
pub const DEFAULT_POINT_SECS: u32 = 15;
/// 航点录像时长上下限（与板端 `RECORD_POINT_MAX_SECS` 对齐）。
pub const MIN_POINT_SECS: u32 = 3;
pub const MAX_POINT_SECS: u32 = 300;
/// 全程录像切片上下限。
const MIN_SEGMENT_SECS: u32 = 30;
const MAX_SEGMENT_SECS: u32 = 1800;

/// 路线动作里可选的录像相机（与板端 `record.py::allowed_cameras`、
/// 前端 `utils/constants.ts::RECORD_CAMERAS` 同一份口径）。
///
/// `dogcam` = 机器狗前置（正前），`cam0`~`cam3` = 背包四路 USB
/// （左前 / 右前 / 右后 / 左后），`dog-ipcam` = 红外。
pub const RECORD_CAMERAS: [&str; 6] = ["dogcam", "cam0", "cam1", "cam2", "cam3", "dog-ipcam"];
/// 一次最多录几路（板端 `RECORD_MAX_CAMERAS` 还会再截一次，这里是入口校验）
pub const MAX_CAMERAS: usize = 4;

/// 需要录像的航点
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordPoint {
    /// 航点下标（0 基，与 `routeData.points` / 5009 `detail_index` 一致）
    pub index: i64,
    pub name: String,
    /// 录多久（秒）
    pub seconds: u32,
    /// 录哪几路（`recordCameras`）；空 = 板端默认源（`RECORD_SOURCE_CAM`）
    pub cameras: Vec<String>,
}

/// 全程录像
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordTask {
    /// 板端切片长度（秒）
    pub segment_seconds: u32,
    /// 录哪几路（`recordCameras`）；空 = 板端默认源
    pub cameras: Vec<String>,
}

/// 一条路线里的录像配置
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordPlan {
    pub points: Vec<RecordPoint>,
    pub task: Option<RecordTask>,
}

impl RecordPlan {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.task.is_none()
    }

    pub fn point_at(&self, index: i64) -> Option<&RecordPoint> {
        self.points.iter().find(|point| point.index == index)
    }
}

/// 动作是不是「录像」类（`录像采集` / `全程录像` / `摄像`）。
///
/// 与 `Waypoint::wants_photo` 共用同一批关键词：本体侧的 `tasks=[1]` 仍然会拍照，
/// 录像是在板端额外起一路 ffmpeg，两者不冲突。
pub fn is_record_action(action_type: &str) -> bool {
    let text = action_type.trim();
    text.contains("录像") || text.contains("摄像")
}

fn is_task_record(action_type: &str) -> bool {
    let text = action_type.trim();
    text.contains("全程") || text.contains("连续录像") || text.contains("整段")
}

/// 从执行参数里抠时长：「1080P / 25fps / 15s」→ 15；「30秒」→ 30。
///
/// 前端新版本会把时长写成 `recordSeconds`（数字），老数据只有 `param` 文本，
/// 两种都要认（历史路线不能因为没这个字段就不录）。
pub fn seconds_from_param(param: &str) -> Option<u32> {
    let chars: Vec<char> = param.chars().collect();
    let mut current = String::new();
    for (index, ch) in chars.iter().enumerate() {
        if ch.is_ascii_digit() {
            current.push(*ch);
            continue;
        }
        if current.is_empty() {
            continue;
        }
        let next = chars.get(index).copied();
        let unit_follows =
            ch.is_whitespace() || *ch == 's' || *ch == 'S' || *ch == '秒' || next == Some('s');
        if unit_follows && let Ok(value) = current.parse::<u32>() {
            return Some(value);
        }
        current.clear();
    }
    current.parse::<u32>().ok()
}

fn point_seconds(action: &Value) -> u32 {
    let explicit = action
        .get("recordSeconds")
        .and_then(Value::as_u64)
        .map(|value| value as u32);
    let parsed = action
        .get("param")
        .and_then(Value::as_str)
        .and_then(seconds_from_param);
    explicit
        .or(parsed)
        .unwrap_or(DEFAULT_POINT_SECS)
        .clamp(MIN_POINT_SECS, MAX_POINT_SECS)
}

fn segment_seconds(action: &Value) -> u32 {
    let explicit = action
        .get("recordSegmentSeconds")
        .or_else(|| action.get("segmentSeconds"))
        .and_then(Value::as_u64)
        .map(|value| value as u32);
    let parsed = action
        .get("param")
        .and_then(Value::as_str)
        .and_then(seconds_from_param);
    explicit
        .or(parsed)
        .unwrap_or(DEFAULT_SEGMENT_SECS)
        .clamp(MIN_SEGMENT_SECS, MAX_SEGMENT_SECS)
}

/// 路线动作里选的要录的相机（`recordCameras: ["cam0","dogcam"]`）。
///
/// 兼容两种写法：数组（前端多选）与单个字符串 `recordCamera`（手工改的路线）。
/// 不认识的相机名直接丢（板端也会再挡一次），全丢了就返回空——
/// 空表示「用板端默认源」，历史路线（没这个字段）行为不变。
fn cameras_from_action(action: &Value) -> Vec<String> {
    let mut picked: Vec<String> = Vec::new();
    if let Some(items) = action.get("recordCameras").and_then(Value::as_array) {
        for item in items {
            if let Some(camera) = item.as_str() {
                push_camera(&mut picked, camera);
            }
        }
    }
    if picked.is_empty()
        && let Some(camera) = action.get("recordCamera").and_then(Value::as_str)
    {
        push_camera(&mut picked, camera);
    }
    picked
}

fn push_camera(picked: &mut Vec<String>, camera: &str) {
    let camera = camera.trim();
    if camera.is_empty()
        || picked.iter().any(|item| item == camera)
        || !RECORD_CAMERAS.contains(&camera)
        || picked.len() >= MAX_CAMERAS
    {
        return;
    }
    picked.push(camera.to_string());
}

/// 日志用：`["cam0","cam1"]` → `cam0,cam1`；空列表表示板端默认源。
fn camera_list(cameras: &[String]) -> String {
    if cameras.is_empty() {
        "板端默认".to_string()
    } else {
        cameras.join(",")
    }
}

/// 解析 `routeData` 里的录像动作。
///
/// - `points[].actions[]` 里的录像动作 → 该航点录 `seconds` 秒（`recordSeconds`，
///   老数据从 `param` 文本里读，默认 [`DEFAULT_POINT_SECS`]）；
/// - `legs[].actions[]` 里的「全程录像」→ 整条任务录到底（`recordSegmentSeconds`
///   或 `param` 的秒数 = 板端切片长度）。同一条路线配多段也只起**第一段**（切片 / 相机
///   取那一份）：任务级作业没有「只录这一段」的语义，配多了只会在日志里报一行忽略提示。
pub fn parse_record_plan(route_data: &Value) -> RecordPlan {
    let mut plan = RecordPlan::default();
    if let Some(points) = route_data.get("points").and_then(Value::as_array) {
        for (index, point) in points.iter().enumerate() {
            let Some(actions) = point.get("actions").and_then(Value::as_array) else {
                continue;
            };
            let record = actions.iter().find(|action| {
                action
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(is_record_action)
            });
            let Some(action) = record else {
                continue;
            };
            let name = point
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("航点")
                .to_string();
            plan.points.push(RecordPoint {
                index: index as i64,
                name,
                seconds: point_seconds(action),
                cameras: cameras_from_action(action),
            });
        }
    }
    if let Some(legs) = route_data.get("legs").and_then(Value::as_array) {
        // 配了多段「全程录像」时只认第一段：任务级录像的切片长度 / 相机是一份，
        // 后面的段**不能静默丢掉**——现场会以为「这一段选了另一路相机」生效了。
        let mut ignored: Vec<usize> = Vec::new();
        for (leg_index, leg) in legs.iter().enumerate() {
            let Some(actions) = leg.get("actions").and_then(Value::as_array) else {
                continue;
            };
            if let Some(action) = actions.iter().find(|action| {
                action
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|text| is_record_action(text) && is_task_record(text))
            }) {
                if plan.task.is_some() {
                    ignored.push(leg_index);
                    continue;
                }
                plan.task = Some(RecordTask {
                    segment_seconds: segment_seconds(action),
                    cameras: cameras_from_action(action),
                });
            }
        }
        if !ignored.is_empty() {
            let first = plan
                .task
                .as_ref()
                .map(|task| format!("每段 {}s", task.segment_seconds))
                .unwrap_or_default();
            warn!(
                "路线里配了 {} 段「全程录像」，只认第一段（{first}，相机={}）；\
                 这些航段被忽略：{ignored:?}。全程录像是**任务级**的——\
                 要一次录多路，就把相机勾在同一条动作上",
                ignored.len() + 1,
                plan.task
                    .as_ref()
                    .map(|task| camera_list(&task.cameras))
                    .unwrap_or_default()
            );
        }
    }
    plan
}

/// 板端 `/api/record/list` 返回的一条待回传录像
#[derive(Debug, Clone)]
pub struct RecordingFile {
    /// `<planId>/<文件名>`：上传成功后原样回传给 `/api/record/delete`
    pub name: String,
    pub path: String,
    pub plan_id: String,
    pub scope: String,
    pub point_index: i64,
    pub point_name: String,
    pub size: u64,
    /// 录像源相机（板端边车里的 `camera`；老文件没有这个字段时按 `dogcam`）
    pub camera: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_sec: f64,
}

/// 一段录像的上传结果（平台按 sha256 去重，`Existing` 也算成功）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UploadOutcome {
    /// 平台新入库
    Created,
    /// 平台已有这一段（重传 / 之前传过但没删板端文件）
    Existing,
}

/// 一轮「回传录像」的结果（`Display` 直接作为指令回执文案给设备页看）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncSummary {
    /// 本轮扫描到的板端录像段数
    pub scanned: usize,
    /// 上传成功的段数（含「平台已有」）
    pub uploaded: usize,
    /// 其中平台早就有了（sha256 命中）的段数
    pub existing: usize,
    /// 失败段数（文件留在板端，再点一次会重试）
    pub failed: usize,
    /// 上传成功后删掉的板端文件数
    pub deleted: usize,
    /// 本轮结束后板端还剩多少段
    pub remaining: usize,
    pub total_bytes: u64,
    pub errors: Vec<String>,
}

impl SyncSummary {
    /// 板端录像目录是否已经清空（设备页据此显示「完成」还是「还有剩余」）。
    pub fn drained(&self) -> bool {
        self.remaining == 0
    }

    /// 是否什么都不用传（板端本来就没攒到录像）。
    pub fn nothing_to_do(&self) -> bool {
        self.scanned == 0 && self.failed == 0
    }
}

impl std::fmt::Display for SyncSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.nothing_to_do() {
            return write!(f, "板端录像目录没有待回传的录像");
        }
        write!(
            f,
            "录像回传完成：扫描 {} 段（合计 {:.1}MB），上传成功 {}（新增 {} / 平台已有 {}），失败 {}，已删除板端文件 {}",
            self.scanned,
            self.total_bytes as f64 / 1024.0 / 1024.0,
            self.uploaded,
            self.uploaded.saturating_sub(self.existing),
            self.existing,
            self.failed,
            self.deleted
        )?;
        if self.drained() {
            write!(f, "；板端录像已清空")?;
        } else {
            write!(
                f,
                "；板端仍有 {} 段未回传，请再点一次「回传录像」",
                self.remaining
            )?;
        }
        if !self.errors.is_empty() {
            write!(f, "；前几条错误：{}", self.errors.join(" | "))?;
        }
        Ok(())
    }
}

/// 一轮回传的进度（每传完一段上报一次，供设备页显示「回传中 3/12 段」）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncProgress {
    pub uploaded: usize,
    pub failed: usize,
    pub total: usize,
    pub remaining: usize,
}

/// 进度回调。回传是长任务（4G 下一段几分钟），没法等 `command_result`
/// 才知道进展，所以每传完一段就回调一次。
pub type ProgressFn<'a> = &'a (dyn Fn(SyncProgress) + Send + Sync);

#[derive(Default)]
struct State {
    task_code: Option<String>,
    plan: RecordPlan,
    /// 正在录的航点下标
    point_active: Option<i64>,
    /// 全程录像是否已经起过
    task_active: bool,
    /// 这条任务已收尾：迟到的到点帧不许再起录像
    finished: bool,
    /// 板端 Agent 不支持录像（404 / 老版本）：本轮不再尝试
    unsupported: bool,
}

/// 巡检录像运行态（进程内唯一）
pub struct Recorder {
    agent: String,
    enabled: bool,
    config: Arc<Config>,
    agent_client: reqwest::Client,
    upload_client: reqwest::Client,
    state: Mutex<State>,
    /// 起停串行化：到点开录是后台任务，和紧随其后的「离开航点停录」交错会出现
    /// 「stop 先到、start 后到」→ 录像一直录到板端 `-t` 上限。
    gate: Mutex<()>,
    /// 同一时间只跑一轮上传（终态 / 启动 / 手动都只是触发，不叠加）
    uploading: AtomicBool,
}

/// 起一路录像的参数（相机 / 时长 / 原因都属于「这一次请求」，整包传省得参数一长串）
struct JobRequest<'a> {
    task_code: &'a str,
    scope: &'a str,
    point_index: i64,
    point_name: &'a str,
    seconds: u32,
    cameras: &'a [String],
    reason: &'a str,
}

impl Recorder {
    pub fn new(config: Arc<Config>, enabled: bool) -> Self {
        Self {
            agent: config.ai_agent_url.trim_end_matches('/').to_string(),
            enabled,
            config,
            agent_client: reqwest::Client::builder()
                .timeout(AGENT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            upload_client: reqwest::Client::builder()
                .timeout(UPLOAD_TIMEOUT)
                .build()
                .unwrap_or_default(),
            state: Mutex::new(State::default()),
            gate: Mutex::new(()),
            uploading: AtomicBool::new(false),
        }
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// 新任务开始：换掉录像配置，清掉上一轮的状态。
    ///
    /// 上一个任务如果还留着录像（客户端重启 / 终态帧丢了），先给它发一条 stop：
    /// 不然两路 ffmpeg 会同时往同一块盘写，任务结束也收不干净。
    pub async fn start_task(self: &Arc<Self>, task_code: &str, plan: RecordPlan) {
        if !self.enabled {
            return;
        }
        // 换成新任务 / 同一条计划被重新下发：先把还在录的收掉。
        // 不收的话两路 ffmpeg 会同时往同一块盘写，而且新任务的终态只停得掉新的那一路。
        let stale = {
            let state = self.state.lock().await;
            match state.task_code.as_deref() {
                Some(code) if code == task_code => (state.point_active.is_some()
                    || state.task_active)
                    .then(|| task_code.to_string()),
                Some(_) => state.task_code.clone(),
                None => None,
            }
        };
        if let Some(stale) = stale {
            self.stop_plan(&stale, "新任务下发 / 重新下发").await;
        }
        if !plan.is_empty() {
            info!(
                "任务 {task_code} 含录像动作：{} 个航点录像（{}），全程录像={}",
                plan.points.len(),
                plan.points
                    .iter()
                    .map(|point| format!(
                        "{}={}s[{}]",
                        point.index,
                        point.seconds,
                        camera_list(&point.cameras)
                    ))
                    .collect::<Vec<_>>()
                    .join(","),
                plan.task
                    .as_ref()
                    .map(|task| format!(
                        "每段 {}s，相机={}",
                        task.segment_seconds,
                        camera_list(&task.cameras)
                    ))
                    .unwrap_or_else(|| "无".to_string())
            );
        }
        let mut state = self.state.lock().await;
        state.task_code = Some(task_code.to_string());
        state.plan = plan;
        state.point_active = None;
        state.task_active = false;
        state.finished = false;
    }

    /// 5009 上看到这条任务在跑（非终态）——全程录像在这一刻起，不等到第一个航点。
    ///
    /// 幂等：5009 每 500ms 一帧。
    pub async fn task_running(self: &Arc<Self>, task_code: &str) {
        if !self.enabled {
            return;
        }
        let task = {
            let state = self.state.lock().await;
            if state.finished
                || state.task_active
                || state.unsupported
                || state.task_code.as_deref() != Some(task_code)
            {
                return;
            }
            state.plan.task.clone()
        };
        let Some(task) = task else {
            return;
        };
        let segment = task.segment_seconds;
        let cameras = task.cameras;
        let _gate = self.gate.lock().await;
        {
            let state = self.state.lock().await;
            if state.finished || state.task_active || state.unsupported {
                return;
            }
        }
        match self
            .start_job(JobRequest {
                task_code,
                scope: "task",
                point_index: -1,
                point_name: "",
                seconds: segment,
                cameras: &cameras,
                reason: "任务开始",
            })
            .await
        {
            Ok(()) => {
                self.state.lock().await.task_active = true;
                info!(
                    "任务 {task_code} 全程录像已开始（每段 {segment}s 落盘，相机={}）",
                    camera_list(&cameras)
                );
            }
            Err(error) => self.note_failure(task_code, "全程录像", &error).await,
        }
    }

    /// 到点录像：`patrol.point_arrived` / `patrol.waiting` 且该点配了录像动作。
    ///
    /// 幂等：同一个点在录就不再发（5009 每帧都会调到）。
    pub async fn point_arrived(self: &Arc<Self>, task_code: &str, point_index: i64) {
        if !self.enabled {
            return;
        }
        let point = {
            let state = self.state.lock().await;
            if state.finished
                || state.unsupported
                || state.task_code.as_deref() != Some(task_code)
                || state.point_active == Some(point_index)
            {
                return;
            }
            state.plan.point_at(point_index).cloned()
        };
        let Some(point) = point else {
            return;
        };
        let _gate = self.gate.lock().await;
        {
            let state = self.state.lock().await;
            if state.finished || state.unsupported || state.point_active == Some(point_index) {
                return;
            }
        }
        // 上一个点的录像可能还没停（5009 偶尔跳过「离开」帧）：先收掉再开新的，
        // 板端同一计划只留一路航点录像。
        if self.state.lock().await.point_active.is_some() {
            self.stop_job(task_code, "point", None, "切换到下一个航点")
                .await;
            self.state.lock().await.point_active = None;
        }
        match self
            .start_job(JobRequest {
                task_code,
                scope: "point",
                point_index: point.index,
                point_name: &point.name,
                seconds: point.seconds,
                cameras: &point.cameras,
                reason: "到达航点",
            })
            .await
        {
            Ok(()) => {
                self.state.lock().await.point_active = Some(point.index);
                info!(
                    "任务 {task_code} 航点 {}（{}）开始录像 {}s（相机={}）",
                    point.index,
                    point.name,
                    point.seconds,
                    camera_list(&point.cameras)
                );
            }
            Err(error) => self.note_failure(task_code, "航点录像", &error).await,
        }
    }

    /// 离开航点：停掉航点录像（全程录像不受影响）。
    pub async fn leave_point(self: &Arc<Self>, reason: &str) {
        if !self.enabled {
            return;
        }
        let task_code = {
            let mut state = self.state.lock().await;
            if state.point_active.take().is_none() {
                return;
            }
            state.task_code.clone()
        };
        let Some(task_code) = task_code else {
            return;
        };
        self.stop_job(&task_code, "point", None, reason).await;
    }

    /// 任务终态 / 取消 / 本地收尾：**只停录像，不回传**。
    ///
    /// 录像留在板端 `RECORD_DIR/<计划>/`，等平台下发「回传录像」指令再传
    /// （见 [`Recorder::sync_all`]）。以前这里会顺手 `spawn_upload()`：现场 4G 上
    /// 一场巡检刚结束就开始传几百 MB，跟下一轮的地图 / 状态上报抢带宽。
    pub async fn finish_task(self: &Arc<Self>, task_code: &str, reason: &str) {
        if !self.enabled {
            return;
        }
        {
            let mut state = self.state.lock().await;
            state.finished = true;
            state.point_active = None;
            state.task_active = false;
        }
        // 任务号为空（平台只发了 cancel 没带 taskCode）：没有 planId 可停，
        // 板端两路 ffmpeg 会按自己的 `-t` 上限结束；录好的文件留在板端等平台要。
        if !task_code.trim().is_empty() {
            self.stop_plan(task_code, reason).await;
        }
    }

    async fn stop_plan(self: &Arc<Self>, task_code: &str, reason: &str) {
        // 不带 scope：航点 + 全程两路一起停（任务终态 / 取消）
        let body = json!({ "planId": task_code, "reason": reason });
        if let Err(error) = self.post("/api/record/stop", body).await {
            warn!("任务 {task_code} 停止录像失败（{reason}）: {error}");
        }
    }

    async fn stop_job(
        self: &Arc<Self>,
        task_code: &str,
        scope: &str,
        point_index: Option<i64>,
        reason: &str,
    ) {
        let mut body = json!({ "planId": task_code, "scope": scope, "reason": reason });
        if let Some(index) = point_index {
            body["pointIndex"] = json!(index);
        }
        if let Err(error) = self.post("/api/record/stop", body).await {
            warn!("任务 {task_code} 停止 {scope} 录像失败（{reason}）: {error}");
        }
    }

    async fn start_job(&self, job: JobRequest<'_>) -> Result<(), String> {
        let JobRequest {
            task_code,
            scope,
            point_index,
            point_name,
            seconds,
            cameras,
            reason,
        } = job;
        let mut body = json!({
            "planId": task_code,
            "scope": scope,
            "pointIndex": point_index,
            "pointName": point_name,
            "reason": reason,
        });
        if scope == "task" {
            // 全程录像：`segmentSeconds` = 切片长度；总上限交给板端
            // `RECORD_TASK_MAX_SECS`（不要在这里传一个 300s 的 maxSeconds，
            // 那会把整条任务录到 5 分钟就自己停）
            body["segmentSeconds"] = json!(seconds);
        } else {
            body["seconds"] = json!(seconds);
        }
        // 多选相机：不传就是板端默认源（历史路线行为不变）。
        // 板端每一路一个 ffmpeg，起不来的那一路只回 `errors`，不会整条失败。
        if !cameras.is_empty() {
            body["cameras"] = json!(cameras);
        }
        let value = self.post("/api/record/start", body).await?;
        if value.get("ok").and_then(Value::as_bool) == Some(false) {
            return Err(value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("板端拒绝录像")
                .to_string());
        }
        // 部分相机没起来：只记日志（其余几路照录，不能让巡检中断）
        for key in ["errors", "rejected"] {
            let Some(items) = value.get(key).and_then(Value::as_array) else {
                continue;
            };
            if !items.is_empty() {
                warn!(
                    "任务 {task_code} {scope} 录像有相机没起录（{key}）：{}",
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
        Ok(())
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{path}", self.agent);
        let resp = self
            .agent_client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("请求板端 {path} 失败: {error}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err("板端 Agent 没有录像接口（旧版本）".to_string());
        }
        if !status.is_success() {
            return Err(format!("板端 {path} 返回 HTTP {status}: {text}"));
        }
        serde_json::from_str(&text).map_err(|error| format!("板端 {path} 响应不是 JSON: {error}"))
    }

    /// 录像失败只打日志：老板子 / 磁盘满 / ffmpeg 缺失都不该影响巡检本身。
    async fn note_failure(&self, task_code: &str, what: &str, error: &str) {
        warn!("任务 {task_code} {what}失败（巡检继续，不影响状态上报）: {error}");
        if error.contains("没有录像接口") {
            self.state.lock().await.unsupported = true;
            warn!(
                "板端 Agent 未升级录像接口：本轮任务不再尝试录像（升级 backpack-agent 后自动恢复）"
            );
        }
    }

    /// 平台下发「回传录像」：把板端积压的录像（`RECORD_DIR`）按需回传。
    ///
    /// 与「同步媒体」同一套语义：一轮把清单里的段全部传一遍，**上传成功才删板端
    /// 文件**（失败的留着，下次再点一次「回传录像」）。不主动跑：任务终态 / 客户端
    /// 启动都不再调用它，只有平台指令（`handlers::command`）会调。
    pub async fn sync_all(
        self: &Arc<Self>,
        progress: Option<ProgressFn<'_>>,
    ) -> Result<SyncSummary, String> {
        if !self.enabled {
            return Err("录像未启用（--no-record / ROBODOG_RECORD_ENABLED=0）".to_string());
        }
        // 同一时间只跑一轮：重复下发不会让板端两份 ffmpeg + 两份上传抢带宽
        if self.uploading.swap(true, Ordering::SeqCst) {
            return Err("上一轮录像回传还在跑，请等它结束（进度见设备页状态）".to_string());
        }
        let outcome = self.run_sync(progress).await;
        self.uploading.store(false, Ordering::SeqCst);
        outcome
    }

    async fn run_sync(&self, progress: Option<ProgressFn<'_>>) -> Result<SyncSummary, String> {
        let files = self.list_recordings().await?;
        let total = files.len();
        let total_bytes: u64 = files.iter().map(|file| file.size).sum();
        if files.is_empty() {
            info!("回传录像：板端没有待回传的录像（{}）", self.agent);
            return Ok(SyncSummary::default());
        }
        let api_base = self.config.resolve_api_base();
        let token = media_sync::login(&self.upload_client, &api_base, &self.config).await?;
        info!(
            "开始回传巡检录像：{total} 段（合计 {:.1}MB）",
            total_bytes as f64 / 1024.0 / 1024.0
        );
        // 第一帧进度：让设备页立刻看到总量（4G 上一段要几分钟）
        if let Some(report) = progress {
            report(SyncProgress {
                total,
                remaining: total,
                ..Default::default()
            });
        }
        let mut summary = SyncSummary {
            scanned: total,
            total_bytes,
            ..Default::default()
        };
        for (index, file) in files.iter().enumerate() {
            match self.upload_one(&api_base, &token, file).await {
                Ok(UploadOutcome::Created) => {
                    summary.uploaded += 1;
                    summary.deleted += 1;
                }
                Ok(UploadOutcome::Existing) => {
                    // 平台按 sha256 去重：这段早就传过，板端这份可以删
                    summary.existing += 1;
                    summary.deleted += 1;
                }
                Err(error) => {
                    summary.failed += 1;
                    if summary.errors.len() < 5 {
                        summary.errors.push(format!("{}: {error}", file.name));
                    }
                    warn!("录像上传失败（{}）: {error}", file.name);
                }
            }
            if let Some(report) = progress {
                report(SyncProgress {
                    total,
                    uploaded: summary.uploaded,
                    failed: summary.failed,
                    remaining: total - (index + 1),
                });
            }
        }
        // 回读一次清单：剩余以板端实际文件为准（失败的 / 新录进来的都算）
        summary.remaining = self
            .list_recordings()
            .await
            .map(|rest| rest.len())
            .unwrap_or(total - summary.deleted);
        info!("巡检录像回传结束：{}；失败的留在板端", summary);
        Ok(summary)
    }

    async fn upload_one(
        &self,
        api_base: &str,
        token: &str,
        file: &RecordingFile,
    ) -> Result<UploadOutcome, String> {
        let bytes = tokio::fs::read(&file.path)
            .await
            .map_err(|error| format!("读取板端录像失败（{}）: {error}", file.path))?;
        let meta = json!({
            "taskCode": file.plan_id,
            "deviceCode": self.config.device_id,
            "planId": file.plan_id,
            "pointIndex": file.point_index,
            "pointName": file.point_name,
            "scope": file.scope,
            "camera": file.camera,
            "startedAt": file.started_at,
            "endedAt": file.ended_at,
            "durationSec": file.duration_sec,
        });
        let url = format!("{}/api/recordings/ingest", api_base.trim_end_matches('/'));
        let mut last_error = String::new();
        for attempt in 1..=UPLOAD_RETRIES {
            let part = multipart::Part::bytes(bytes.clone())
                .file_name("recording.mp4".to_string())
                .mime_str("video/mp4")
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
                        // 上传成功才删板端文件（失败的要留给 `回传录像` 再试一次）
                        let created = serde_json::from_str::<Value>(&text)
                            .ok()
                            .and_then(|value| value.get("data")?.get("created")?.as_bool())
                            .unwrap_or(true);
                        self.delete_on_board(&file.name).await;
                        return Ok(if created {
                            UploadOutcome::Created
                        } else {
                            UploadOutcome::Existing
                        });
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
            .post("/api/record/delete", json!({ "names": [name] }))
            .await
        {
            Ok(_) => {}
            Err(error) => warn!("删除板端录像失败（{name}）: {error}"),
        }
    }

    async fn list_recordings(&self) -> Result<Vec<RecordingFile>, String> {
        let url = format!("{}/api/record/list", self.agent);
        let resp = self
            .agent_client
            .get(&url)
            .send()
            .await
            .map_err(|error| format!("请求板端录像清单失败: {error}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("板端录像清单返回 HTTP {status}: {text}"));
        }
        let value: Value =
            serde_json::from_str(&text).map_err(|error| format!("录像清单不是 JSON: {error}"))?;
        let files = value
            .get("files")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        Some(RecordingFile {
                            name: item.get("name")?.as_str()?.to_string(),
                            path: item.get("path")?.as_str()?.to_string(),
                            plan_id: item.get("planId")?.as_str()?.to_string(),
                            scope: item
                                .get("scope")
                                .and_then(Value::as_str)
                                .unwrap_or("point")
                                .to_string(),
                            point_index: item
                                .get("pointIndex")
                                .and_then(Value::as_i64)
                                .unwrap_or(-1),
                            point_name: item
                                .get("pointName")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
                            // 老板子没有 `camera`：那些录像只可能是默认源 dogcam
                            camera: item
                                .get("camera")
                                .and_then(Value::as_str)
                                .filter(|camera| !camera.is_empty())
                                .unwrap_or("dogcam")
                                .to_string(),
                            started_at: item
                                .get("startedAt")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            ended_at: item
                                .get("endedAt")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            duration_sec: item
                                .get("durationSec")
                                .and_then(Value::as_f64)
                                .unwrap_or(0.0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::extract::State;
    use axum::routing::{get, post};
    use robodog_device_client::config::{CliArgs, EnvConfig, FileConfig};
    use serde_json::json;

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

    #[test]
    fn parses_point_and_task_recordings_from_route_data() {
        let route = json!({
            "points": [
                {"name": "P1", "actions": [{"type": "可见光拍照", "param": "1080P"}]},
                {"name": "P2", "actions": [{"type": "录像采集", "param": "1080P / 25fps / 15s"}]},
                {"name": "P3", "actions": [
                    {"type": "可见光拍照"},
                    {"type": "录像采集", "recordSeconds": 45,
                     "recordCameras": ["cam0", "cam1", "dogcam"]}
                ]}
            ],
            "legs": [
                {"actions": [{"type": "连续测温"}]},
                {"actions": [{"type": "全程录像", "param": "1080P / 25fps / 连续"}]}
            ]
        });
        let plan = parse_record_plan(&route);
        assert_eq!(
            plan.points,
            vec![
                RecordPoint {
                    index: 1,
                    name: "P2".to_string(),
                    seconds: 15,
                    // 老数据没有 recordCameras：空 = 板端默认源
                    cameras: vec![]
                },
                RecordPoint {
                    index: 2,
                    name: "P3".to_string(),
                    seconds: 45,
                    cameras: vec!["cam0".to_string(), "cam1".to_string(), "dogcam".to_string()]
                },
            ]
        );
        assert_eq!(
            plan.task,
            Some(RecordTask {
                segment_seconds: DEFAULT_SEGMENT_SECS,
                cameras: vec![]
            }),
            "全程录像的 param 里没有秒数时用默认切片"
        );
    }

    /// 配了多段「全程录像」：只认第一段（任务级录像的切片 / 相机就一份），
    /// 后面的段要**报一行日志**——不然现场会以为「这一段选了另一路相机」生效了。
    #[test]
    fn only_the_first_task_recording_segment_is_used() {
        let plan = parse_record_plan(&json!({
            "points": [{"name": "P1", "actions": []}],
            "legs": [
                {"actions": [{"type": "全程录像", "recordSegmentSeconds": 60,
                             "recordCameras": ["cam0"]}]},
                {"actions": [{"type": "全程录像", "recordSegmentSeconds": 600,
                             "recordCameras": ["dogcam"]}]}
            ]
        }));
        let task = plan.task.expect("应该有全程录像");
        assert_eq!(task.segment_seconds, 60, "取第一段的切片长度");
        assert_eq!(task.cameras, vec!["cam0".to_string()], "取第一段的相机");
    }

    /// 相机多选：去重、白名单、上限；兼容单个 `recordCamera` 字符串。
    ///
    /// 不认识的相机名直接丢，全丢完就退到「板端默认源」——否则路线里一个打字
    /// 错误会让整条录像静默消失（现场应该只是少录一路）。
    #[test]
    fn cameras_are_filtered_deduped_and_capped() {
        let plan = parse_record_plan(&json!({
            "points": [
                {"name": "P1", "actions": [{"type": "录像采集",
                    "recordCameras": ["cam0", "cam0", "cam9", "cam1", "cam2", "cam3", "dogcam"]}]},
                {"name": "P2", "actions": [{"type": "录像采集", "recordCamera": "cam2"}]},
                {"name": "P3", "actions": [{"type": "录像采集", "recordCameras": ["nope"]}]}
            ],
            "legs": [{"actions": [{"type": "全程录像", "recordCameras": ["dog-ipcam", "cam1"]}]}]
        }));
        assert_eq!(
            plan.points[0].cameras,
            vec!["cam0", "cam1", "cam2", "cam3"],
            "去重 + 丢不认识的 + 截到上限：{:?}",
            plan.points[0].cameras
        );
        assert_eq!(plan.points[1].cameras, vec!["cam2".to_string()]);
        assert!(
            plan.points[2].cameras.is_empty(),
            "全是不认识的相机时退到板端默认源：{:?}",
            plan.points[2].cameras
        );
        assert_eq!(
            plan.task.expect("应该有全程录像").cameras,
            vec!["dog-ipcam".to_string(), "cam1".to_string()]
        );
    }

    #[test]
    fn route_without_record_actions_is_empty() {
        let route = json!({
            "points": [{"name": "P1", "actions": [{"type": "可见光拍照"}]}],
            "legs": [{"actions": [{"type": "静默通行"}]}]
        });
        assert!(parse_record_plan(&route).is_empty());
        assert!(parse_record_plan(&json!({})).is_empty());
    }

    #[test]
    fn seconds_come_from_text_param_or_default_and_are_clamped() {
        assert_eq!(seconds_from_param("1080P / 25fps / 15s"), Some(15));
        assert_eq!(seconds_from_param("30 秒"), Some(30));
        assert_eq!(seconds_from_param("连续"), None);
        // 太短 / 太长都夹回合理区间（板端也有同样上下限，这里是双保险）
        let plan = parse_record_plan(&json!({
            "points": [
                {"name": "P1", "actions": [{"type": "录像采集", "recordSeconds": 1}]},
                {"name": "P2", "actions": [{"type": "录像采集", "recordSeconds": 9999}]}
            ]
        }));
        assert_eq!(plan.points[0].seconds, MIN_POINT_SECS);
        assert_eq!(plan.points[1].seconds, MAX_POINT_SECS);
    }

    type Calls = Arc<tokio::sync::Mutex<Vec<Value>>>;

    async fn spawn_agent(calls: Calls) -> String {
        let app = axum::Router::new()
            .route(
                "/api/record/start",
                post(
                    |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(json!({"path": "start", "body": body}));
                        Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/record/stop",
                post(
                    |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(json!({"path": "stop", "body": body}));
                        Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/record/list",
                get(|State(calls): State<Calls>| async move {
                    calls.lock().await.push(json!({"path": "list"}));
                    Json(json!({"files": []}))
                }),
            )
            .route(
                "/api/record/delete",
                post(|Json(body): Json<Value>| async move {
                    Json(json!({"ok": true, "deleted": body["names"].as_array().map(Vec::len)}))
                }),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    fn plan() -> RecordPlan {
        RecordPlan {
            points: vec![RecordPoint {
                index: 1,
                name: "P2".to_string(),
                seconds: 12,
                cameras: vec!["cam0".to_string(), "dogcam".to_string()],
            }],
            task: Some(RecordTask {
                segment_seconds: 120,
                cameras: vec!["cam1".to_string()],
            }),
        }
    }

    /// 全程录像 + 航点录像：各起一次（5009 每帧重复调用），离开航点只停航点那路。
    #[tokio::test]
    async fn starts_once_into_the_agent_and_stops_per_scope() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let recorder = Arc::new(Recorder::new(Arc::new(test_config(&agent)), true));

        recorder.start_task("TK-1", plan()).await;
        recorder.task_running("TK-1").await;
        recorder.task_running("TK-1").await;
        recorder.point_arrived("TK-1", 1).await;
        recorder.point_arrived("TK-1", 1).await;
        // 没配录像的航点不发起录
        recorder.point_arrived("TK-1", 0).await;
        recorder.leave_point("已离开航点").await;

        let calls = calls.lock().await.clone();
        let starts: Vec<&Value> = calls
            .iter()
            .filter(|call| call["path"] == "start")
            .collect();
        assert_eq!(starts.len(), 2, "全程 + 航点各一次：{calls:?}");
        assert_eq!(starts[0]["body"]["scope"], "task");
        assert_eq!(
            starts[0]["body"]["segmentSeconds"], 120,
            "全程录像的切片长度要下发，且不能带 maxSeconds（否则整条任务只录 120s）"
        );
        assert!(
            starts[0]["body"].get("maxSeconds").is_none(),
            "maxSeconds 由板端兜底，客户端不要传：{calls:?}"
        );
        assert_eq!(starts[1]["body"]["scope"], "point");
        assert_eq!(starts[1]["body"]["pointIndex"], 1);
        assert_eq!(starts[1]["body"]["seconds"], 12);
        // 多选相机要原样下发（板端每一路一个 ffmpeg）
        assert_eq!(
            starts[0]["body"]["cameras"],
            json!(["cam1"]),
            "全程录像的相机要下发：{calls:?}"
        );
        assert_eq!(
            starts[1]["body"]["cameras"],
            json!(["cam0", "dogcam"]),
            "航点录像的相机要下发：{calls:?}"
        );
        let stops: Vec<&Value> = calls.iter().filter(|call| call["path"] == "stop").collect();
        assert_eq!(stops.len(), 1, "离开航点只停航点录像：{calls:?}");
        assert_eq!(stops[0]["body"]["scope"], "point");
    }

    /// 终态后迟到的到点帧不许再起录像（与灯光同一条收口规则）。
    #[tokio::test]
    async fn late_arrival_after_finish_is_ignored() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let recorder = Arc::new(Recorder::new(Arc::new(test_config(&agent)), true));

        recorder.start_task("TK-1", plan()).await;
        recorder.finish_task("TK-1", "任务终态").await;
        recorder.point_arrived("TK-1", 1).await;

        let calls = calls.lock().await.clone();
        assert!(
            calls.iter().all(|call| call["path"] != "start"),
            "终态后不该再起录像：{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|call| call["path"] == "stop" && call["body"].get("scope").is_none()),
            "终态要按计划整条停一次：{calls:?}"
        );
    }

    /// 板端没有录像接口（老版本）时只记一次日志，不再每帧都试。
    #[tokio::test]
    async fn old_agent_disables_recording_for_the_task() {
        let recorder = Arc::new(Recorder::new(
            Arc::new(test_config("http://127.0.0.1:1")),
            true,
        ));
        recorder.start_task("TK-1", plan()).await;
        recorder.point_arrived("TK-1", 1).await;
        // 第二次不再发请求（连接被拒会立刻返回；这里只看状态，不引入额外依赖）
        let state = recorder.state.lock().await;
        assert!(state.unsupported || state.point_active.is_none());
    }

    /// 任务终态**不再**主动回传：录像就留在板端，等平台下发「回传录像」。
    ///
    /// 回归点：以前终态会 `spawn_upload()`，现场 4G 上刚跑完就开始传几百 MB。
    /// 这里用假 Agent 的 `/api/record/list` 计数（回传第一步就是列清单）。
    #[tokio::test]
    async fn finish_task_keeps_files_on_the_board_without_uploading() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let recorder = Arc::new(Recorder::new(Arc::new(test_config(&agent)), true));

        recorder.start_task("TK-1", plan()).await;
        recorder.finish_task("TK-1", "任务终态").await;
        // 回传是 spawn 出去的：给一个宽裕的窗口，确认它确实没有发生
        tokio::time::sleep(Duration::from_millis(200)).await;

        let calls = calls.lock().await.clone();
        assert!(
            calls.iter().all(|call| call["path"] != "list"),
            "终态不该列板端录像清单（回传只能由平台指令触发）：{calls:?}"
        );
    }

    /// 「回传录像」：从板端清单逐段上传，成功才删板端文件，并把进度回调出去。
    #[tokio::test]
    async fn sync_all_uploads_then_deletes_and_reports_progress() {
        // 板端真文件（客户端要读盘上传）
        let dir = std::env::temp_dir().join(format!("rec-sync-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录失败");
        let path = dir.join("task-1.mp4");
        std::fs::write(&path, b"fake-mp4-bytes").expect("写临时录像失败");

        let agent_calls: Calls = Default::default();
        let remaining: Calls = Default::default();
        remaining.lock().await.push(json!({
            "name": format!("TK-1/{}", path.file_name().unwrap().to_string_lossy()),
            "path": path.to_string_lossy(),
            "planId": "TK-1",
            "scope": "task",
            "pointIndex": -1,
            "pointName": "",
            "size": 14,
            "startedAt": "2026-11-25 09:43:19",
            "endedAt": "2026-11-25 09:51:22",
            "durationSec": 100.6,
        }));
        let agent = spawn_sync_agent(agent_calls.clone(), remaining.clone()).await;
        let platform_calls: Calls = Default::default();
        let platform = spawn_platform(platform_calls.clone()).await;

        let mut config = test_config(&agent);
        config.api_base = Some(platform);
        let recorder = Arc::new(Recorder::new(Arc::new(config), true));

        let progress: Calls = Default::default();
        let report = |p: SyncProgress| {
            let progress = progress.clone();
            tokio::spawn(async move {
                progress
                    .lock()
                    .await
                    .push(json!({"uploaded": p.uploaded, "total": p.total}));
            });
        };
        let summary = recorder.sync_all(Some(&report)).await.expect("回传应成功");
        assert_eq!(summary.scanned, 1);
        assert_eq!(summary.uploaded, 1, "一段应上传成功：{summary}");
        assert_eq!(summary.existing, 0);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.deleted, 1, "上传成功要删板端文件：{summary}");
        assert_eq!(summary.remaining, 0);
        assert!(summary.drained());

        let agent_calls = agent_calls.lock().await.clone();
        let deletes: Vec<&Value> = agent_calls
            .iter()
            .filter(|call| call["path"] == "delete")
            .collect();
        assert_eq!(
            deletes.len(),
            1,
            "删板端文件走 /api/record/delete：{agent_calls:?}"
        );
        assert_eq!(
            deletes[0]["body"]["names"][0].as_str(),
            Some(format!("TK-1/{}", path.file_name().unwrap().to_string_lossy()).as_str()),
            "回传的 name 要原样带回板端"
        );

        let platform_calls = platform_calls.lock().await.clone();
        assert!(
            platform_calls.iter().any(|call| call["path"] == "ingest"),
            "要打到平台 /api/recordings/ingest：{platform_calls:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let progress = progress.lock().await.clone();
        assert!(
            progress
                .iter()
                .any(|p| p["total"] == 1 && p["uploaded"] == 0),
            "开传前要先报一次总量：{progress:?}"
        );
        assert!(
            progress.iter().any(|p| p["uploaded"] == 1),
            "每传完一段要报进度：{progress:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 已启用时重复下发：第二轮直接拒绝（不让两份上传抢带宽）。
    #[tokio::test]
    async fn sync_all_refuses_to_run_twice() {
        let recorder = Arc::new(Recorder::new(
            Arc::new(test_config("http://127.0.0.1:1")),
            true,
        ));
        recorder.uploading.store(true, Ordering::SeqCst);
        let error = recorder.sync_all(None).await.expect_err("第二轮应被拒绝");
        assert!(error.contains("还在跑"), "{error}");
        recorder.uploading.store(false, Ordering::SeqCst);
        // 未启用的实例也不该跑
        let disabled = Arc::new(Recorder::new(
            Arc::new(test_config("http://127.0.0.1:1")),
            false,
        ));
        let error = disabled.sync_all(None).await.expect_err("未启用应拒绝");
        assert!(error.contains("未启用"), "{error}");
    }

    /// 假 Agent：清单来自可变状态（/api/record/delete 会把条目摘掉）。
    async fn spawn_sync_agent(calls: Calls, files: Calls) -> String {
        let app = axum::Router::new()
            .route(
                "/api/record/list",
                get(|State((_calls, files)): State<(Calls, Calls)>| async move {
                    Json(json!({"files": files.lock().await.clone()}))
                }),
            )
            .route(
                "/api/record/delete",
                post(
                    |State((calls, files)): State<(Calls, Calls)>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(json!({"path": "delete", "body": body.clone()}));
                        let names: Vec<String> = body["names"]
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|name| name.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut files = files.lock().await;
                        files.retain(|file| {
                            !names
                                .iter()
                                .any(|name| file["name"].as_str() == Some(name.as_str()))
                        });
                        Json(json!({"ok": true}))
                    },
                ),
            )
            .with_state((calls, files));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    /// 假平台：登录 + 录像入库（回 `created=true`）。
    async fn spawn_platform(calls: Calls) -> String {
        let app = axum::Router::new()
            .route(
                "/api/auth/login",
                post(|| async { Json(json!({"code": 10200, "data": {"token": "t"}})) }),
            )
            .route(
                "/api/recordings/ingest",
                post(
                    |State(calls): State<Calls>, _body: axum::body::Bytes| async move {
                        calls.lock().await.push(json!({"path": "ingest"}));
                        Json(json!({"code": 10200, "data": {"created": true}}))
                    },
                ),
            )
            .with_state(calls);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定假平台失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }
}
