//! 航点 AI 识别（设备端）
//!
//! 为什么放在客户端而不是业务平台：
//!
//! - 板端 Agent（背包 FastAPI，`http://127.0.0.1:8000`）只在内网可达。正式环境的
//!   业务平台在公网，服务端直连 Agent 必然超时（现场表现是「配了 AI 识别却从来没
//!   告警」）；而本客户端就跑在板子（`.246`）上，`127.0.0.1` 一定通。
//! - 到点判定本来就在本地：5009 快照的 `stage` / `detail.last_arrived_point` 就是
//!   客户端翻译成业务进度的，顺手拿来做触发点。
//!
//! 职责边界（**决策仍在服务端**，避免两处阈值/场景规则各说一套）：
//!
//! 1. 到点：本模块探测 Agent 的模型目录与运行态，把「动作参数 + 模型目录」上报
//!    平台（`ai_point`）；
//! 2. 平台用同一套 `resolve_scene` / `parse_conf` / 窗口时长规则算出场景、类别、
//!    置信度、窗口秒数，下发 `ai_start`；
//! 3. 本模块照着启停 Agent、读 SSE，把**超过阈值且本窗口没报过**的类别逐条上报
//!    （`ai_detect`），窗口结束再上报 `ai_end`；
//! 4. 告警落库、企业微信推送仍由服务端做。
//!
//! 已知边界（与服务端版本一致）：
//!
//! - Agent 是单路单任务，窗口进程内唯一：新窗口顶替旧窗口；操作员在「实时视频」
//!   页手动开的识别只做热更新（`/api/detect/config`），窗口结束**不会**把它停掉；
//! - Agent 连不上时只打 WARN 并跳过，巡检照常跑完（识别失败不影响任务）。

use crate::{WsMessage, send};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::Instant;
use tracing::{debug, info, warn};

/// 探测用的短超时：Agent 与客户端同机，3s 足够；超时就按「没接背包」处理
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// 证据图体积上限（**原图字节**）：企业微信图片消息的硬限制是 2MB，超了整条被拒。
/// 板端 `dogcam_*.jpg`（1080p 抓拍）通常几百 KB，真超了就不带图（卡片里仍有路径）。
const EVIDENCE_IMAGE_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// 存证图的「新鲜度」余量：只有 mtime 在本轮窗口开始（减这个余量）之后的图才认。
/// 板端写图和 SSE 事件之间差几百毫秒，留 5s 免得刚写下的图被当成上一轮的。
const EVIDENCE_MTIME_SLACK: Duration = Duration::from_secs(5);
/// 检出「等证据图」的上限：命中帧不是检出那一刻就落盘的，板端要先连采
/// `DETECT_PLAN_COUNT_SAMPLE`（默认 3）帧命中、画框存证，再发带 `evidence` 的
/// `plan_hit`。2026-11-18 现场那次（垃圾桶模型 0.05 阈值）：第一帧检出 18:54:48.8，
/// 命中帧不连续，第二/三帧到 18:54:53.0 才凑够，图 18:54:53.7 落盘——比检出晚
/// **4.6s**。所以命中先挂起等 `plan_hit`，最多等这么久；还是等不到就先发卡片保告警。
const EVIDENCE_PENDING_WAIT: Duration = Duration::from_secs(8);
/// 板端存证根目录（`detect_jobs`）的默认位置：Agent 把命中帧写在
/// `<jobs_dir>/<planId>/pt<航点下标>/` 下。
///
/// 板子上的部署路径不止一种（官方镜像 `/root/backpack_dashboard/backend`；
/// 现场那台 `.246` 2026-11-20 从 `/root/261113/backpack_dashboard/backend` 迁到了
/// `/opt/xjpt/backpack_dashboard/backend`，旧路径已删），所以这里只是「先试的候选」，
/// 真正以板端 `plan_hit.evidence` 反推出来的目录为准（见 [`jobs_root_from_evidence`]）；
/// 也可以直接配 `[ai] jobs_dir` / `ROBODOG_AI_JOBS_DIR` 钉死。
const DEFAULT_JOBS_DIRS: [&str; 3] = [
    "/root/backpack_dashboard/backend/detect_jobs",
    "/opt/xjpt/backpack_dashboard/backend/detect_jobs",
    "/root/261113/backpack_dashboard/backend/detect_jobs",
];
/// 计划跑完之后等窗口收尾的重试节奏：任务终态时最后一个窗口可能还在收尾
/// （停识别 → 板端 `_plan_flush` 落盘 → 扫盘补图，见 [`AiPatrol::finish`]），
/// 那段代码还在读存证图，删早了会把还没推出去的图毁掉。
const PLAN_CLEANUP_RETRY: Duration = Duration::from_secs(5);
/// 计划收尾最多等这么久（24 × 5s）：窗口一直不收尾就先留着，交给下次启动的
/// [`AiPatrol::sweep_stale_plans`]。
const PLAN_CLEANUP_ATTEMPTS: usize = 24;
/// 启动时兜底清理的年龄阈值：`detect_jobs` 下超过这么久没动过的计划目录，说明
/// 「那次计划收尾没跑到」（客户端被 kill / 断电 / 老版本只删推过图的航点）。
/// 这里全是临时存证（画框 JPG + `result.json`，图已经进过企业微信群），留 3 天
/// 只是为了出问题还能人工翻一翻。
const STALE_PLAN_AGE: Duration = Duration::from_secs(3 * 24 * 3600);
/// 启停识别要等模型加载（板端实测 1~3s），给足余量
const ACTION_TIMEOUT: Duration = Duration::from_secs(60);
/// SSE 单条缓冲上限：Agent 事件每条几十字节，超过说明流里没有 `\n\n`，丢弃重来
const SSE_BUF_LIMIT: usize = 64 * 1024;

/// 路线里一个航点的「AI 识别」动作（来自 `task_dispatch.routeData.points[]`）
#[derive(Debug, Clone, PartialEq)]
pub struct AiPoint {
    /// 航点下标（0-based，与 `routeData.points` 一致）
    pub index: i64,
    pub name: String,
    pub dwell: i64,
    /// 动作 `param`：历史数据 / 手填的中文提示（「灭火器 · 灭火器」）
    pub hint: String,
    /// 动作 `scene`：编辑器从「实时视频 → AI 识别」模型目录里选的模型包 id
    pub scene: Option<String>,
    /// 动作 `labels`：勾选的识别类别（Agent 的 label `name`）
    pub labels: Vec<String>,
    /// 动作 `threshold` 原文（「置信度 ≥ 0.35」），由服务端解析
    pub threshold: String,
    /// 动作 `expectCount`：检查目标数量（数量任务必填；存在性任务不参与判定）
    pub expect_count: Option<i64>,
    /// 任务类型：`exist` = 检测是否存在 / `count` = 检测数量是否正确
    pub check_type: String,
    /// 动作 `alertWhenMissing`：`exist` 任务未检出时也告警（「应该有却没有」）
    ///
    /// 客户端不判定告警，只把配置转给平台（告警文案/去重都在服务端）。
    pub alert_when_missing: bool,
}

/// 解析 `routeData`（`{"points":[...]}`）里所有带 AI 识别动作的航点。
///
/// 没有 AI 动作的航点不进结果——到点时查不到就说明「这个点不用识别」。
pub fn parse_ai_points(route_data: &Value) -> Vec<AiPoint> {
    let Some(points) = route_data.get("points").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (index, point) in points.iter().enumerate() {
        let Some(action) = point
            .get("actions")
            .and_then(Value::as_array)
            .and_then(|actions| actions.iter().find(|action| is_ai_action(action)))
        else {
            continue;
        };
        out.push(AiPoint {
            index: index as i64,
            name: point
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("航点")
                .to_string(),
            dwell: point.get("dwell").and_then(Value::as_i64).unwrap_or(0),
            hint: action
                .get("param")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            scene: action
                .get("scene")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            labels: action
                .get("labels")
                .and_then(Value::as_array)
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            threshold: action
                .get("threshold")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            expect_count: parse_expect_count(action.get("expectCount")),
            check_type: normalize_check_type(
                action.get("checkType").and_then(Value::as_str),
                parse_expect_count(action.get("expectCount")),
            ),
            alert_when_missing: action
                .get("alertWhenMissing")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    out
}

/// 动作类型是不是「AI 识别」：与业务平台 `ai_patrol::is_ai_action` 同一规则
/// （含 `AI` 或 `识别`；「红外测温」这类不受影响）。
fn is_ai_action(action: &Value) -> bool {
    let kind = action
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    kind.to_ascii_uppercase().contains("AI") || kind.contains("识别")
}

/// 从 `ai_points` 里取指定下标的 AI 动作
pub fn ai_point_at(points: &[AiPoint], index: i64) -> Option<&AiPoint> {
    points.iter().find(|point| point.index == index)
}

/// 5009 阶段是否仍在航点上：与业务平台 `ai_patrol::is_point_stage` 同一规则。
///
/// 窗口时长跟着停留时间算（不再截断成 30 s），所以「离开航点」必须由阶段来收口：
/// `patrol.navigating` / `patrol.returning*` 这些一出现就关窗，否则相机会把路过的
/// 东西当成这个点的目标。
pub fn is_point_stage(stage: &str) -> bool {
    matches!(
        stage,
        "patrol.point_arrived"
            | "patrol.waiting"
            | "patrol.photographing"
            | "patrol.point_completed"
    )
}

/// 5009 阶段是不是「在航点上」：与业务平台 `ai_patrol::is_arrival_stage` 同一规则。
///
/// 同一个点会连报多帧（`point_arrived` 一次，`waiting` 每秒数次），
/// 去重交给 [`AiPatrol::point_arrived`] 里的 `requested` 记录。
pub fn is_arrival_stage(stage: &str) -> bool {
    matches!(stage, "patrol.point_arrived" | "patrol.waiting")
}

/// 平台下发的开窗指令（`ai_start`）
#[derive(Debug, Clone, PartialEq)]
pub struct AiStart {
    pub window_id: String,
    /// `start` = 新开识别；`config` = 已有识别在跑，只热更新参数
    pub action: String,
    pub task: String,
    /// 业务任务号 / 航点下标（窗口归属，用于 5009 高频帧去重与迟到事件丢弃）
    pub task_code: String,
    pub point_index: i64,
    /// 计划 id（= 业务任务号）：板端拿它建存证目录 `detect_jobs/<planId>/pt<下标>/`
    pub plan_id: String,
    /// 检查目标数量（数量任务必填）
    pub expect_count: Option<i64>,
    /// 任务类型：`exist` / `count`（旧平台不下发时按 expectCount 推断）
    pub check_type: String,
    pub labels: Vec<String>,
    pub conf: f64,
    pub cam: String,
    pub fps: u32,
    pub secs: u64,
    /// 板端媒体目录（中台参数配置 `media_picture_dir`，经业务平台下发）。
    /// 板端按它落图，「同步媒体」按同一目录回传；缺省时用板端 `MEDIA_DATA_DIR`。
    pub media_dir: Option<String>,
    /// 每个航点固定保存几帧（中台 `media_frames`，1~10）
    pub media_frames: Option<u32>,
    /// 开窗前 Agent 是否已在识别（操作员手动开的）：是则窗口结束不停它
    pub was_running: bool,
}

impl AiStart {
    pub fn from_value(value: &Value) -> Option<Self> {
        let window_id = value.get("windowId").and_then(Value::as_str)?.to_string();
        if window_id.trim().is_empty() {
            return None;
        }
        Some(Self {
            window_id,
            action: value
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("start")
                .to_string(),
            task: value
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            task_code: value
                .get("taskCode")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            point_index: value
                .get("pointIndex")
                .and_then(Value::as_i64)
                .unwrap_or(-1),
            // 计划 id 以平台下发的 `planId` 为准，老平台没这个字段时退回 taskCode
            plan_id: value
                .get("planId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    value
                        .get("taskCode")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_default(),
            expect_count: parse_expect_count(value.get("expectCount")),
            check_type: normalize_check_type(
                value.get("checkType").and_then(Value::as_str),
                parse_expect_count(value.get("expectCount")),
            ),
            labels: value
                .get("labels")
                .and_then(Value::as_array)
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            conf: value.get("conf").and_then(Value::as_f64).unwrap_or(0.5),
            cam: value
                .get("cam")
                .and_then(Value::as_str)
                .unwrap_or("dogcam")
                .to_string(),
            fps: value.get("fps").and_then(Value::as_u64).unwrap_or(3) as u32,
            media_dir: value
                .get("mediaDir")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string),
            media_frames: value
                .get("mediaFrames")
                .and_then(Value::as_u64)
                .map(|frames| (frames as u32).clamp(1, 10)),
            secs: value.get("secs").and_then(Value::as_u64).unwrap_or(15),
            was_running: value
                .get("wasRunning")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// 窗口收尾要补的活（[`AiPatrol::consume`] → [`AiPatrol::finish`]）。
///
/// 板端命中帧不是检出那一刻就落盘：`_plan_frame` 要连采 3 帧命中才画框存证。命中帧
/// 稀疏时（2026-11-18 19:35 那次只命中 1 帧）采样要到窗口收尾 `_plan_flush` 才结束，
/// 所以图只能在停识别之后才拿到，收尾时按下面两种情况补：
///
/// - `pending`：卡片都还没发（命中后 8s 内没等到图），补发时直接带上图；
/// - `image_missing`：卡片早就发了（8s 兜底），图只能单独再发一条（`ai_evidence`）。
#[derive(Default)]
struct Leftover {
    /// 还没上报的检出（label, 置信度）
    pending: Vec<(String, f64)>,
    /// 有卡片发出时没带上图：收尾拿到图就补发一张
    image_missing: bool,
}

/// 当前窗口（进程内唯一：Agent 单路推理）
struct Window {
    id: String,
    task_code: String,
    /// 计划 id（= 业务任务号）：板端拿它建存证目录 `detect_jobs/<planId>/pt<下标>/`。
    /// 老平台不下发 `planId` 时等于 `task_code`（见 [`AiStart::from_value`]）。
    plan_id: String,
    point_index: i64,
    /// 停止信号：`notify_one` 留 permit，窗口任务即使在 `chunk().await` 里也能立刻醒
    cancel: Arc<Notify>,
}

/// 待清理的板端存证目标：（计划 id, 航点下标）
type CleanupTarget = (String, i64);

/// 航点 AI 识别运行态
pub struct AiPatrol {
    /// 板端 Agent 基地址（默认 `http://127.0.0.1:8000`）
    agent: String,
    enabled: bool,
    probe: reqwest::Client,
    action: reqwest::Client,
    stream: reqwest::Client,
    window: Mutex<Option<Window>>,
    /// 已经为哪个 (任务, 航点) 发过 `ai_point`：5009 每 500ms 一帧，
    /// 没有这个去重就会把同一个点请求几十次。窗口建立或换点/结束时清掉。
    requested: Mutex<Option<(String, i64)>>,
    /// 板端存证根目录（`detect_jobs`）：检出告警要的证据图从这里读。
    ///
    /// 三类来源，优先级从高到低：显式配置（`[ai] jobs_dir` / `ROBODOG_AI_JOBS_DIR`）→
    /// 板端 `plan_hit.evidence` 反推出来的真实根 → 内置默认候选
    /// （[`DEFAULT_JOBS_DIRS`]）。都没有就不带图，告警照发。
    /// 显式配置的值不会被板端事件覆盖（[`AiPatrol::jobs_dir_pinned`]）。
    jobs_dir: Mutex<Option<String>>,
    /// `jobs_dir` 是不是运维显式配的：配了就**不**被板端事件学到的新路径覆盖
    /// （现场可能把存证目录 bind-mount 到别处，客户端比板端日志更可信）
    jobs_dir_pinned: bool,
    /// 平台已经推完图、等窗口收尾才删的板端存证（计划 id, 航点下标）。
    ///
    /// 为什么不当场删：告警推送往往在窗口中间就完成了，那时板端可能还在写图，
    /// 客户端收尾时也要再扫一次存证目录补图（`ai_evidence`）。删早了会把还没推出去的
    /// 那张图毁掉——图只在板端，删了不可逆。所以窗口开着就先记账，收尾后再删
    /// （见 [`AiPatrol::cleanup_evidence`] / [`AiPatrol::finish`]）。
    pending_cleanup: Mutex<Vec<CleanupTarget>>,
    /// 已经跑完（终态 / 取消）、等窗口收尾后要**整个**删掉的计划 id。
    ///
    /// 只删「推过图的航点」（`ai_cleanup`）清不干净：数量任务的检出只落任务时间线、
    /// 一个都没命中的航点只留一份 `result.json`、图被拒 / 设备不在线也一样，这些点
    /// 永远等不到 `ai_cleanup`，`detect_jobs/<计划>/` 就一直留在板子上。所以任务结束时
    /// 把整个计划目录删掉（见 [`AiPatrol::finish_plan`]）；新一轮同名计划开始时把它
    /// 从这份名单里摘掉，别把新跑出来的存证顺手删了。
    ending_plans: Mutex<Vec<String>>,
}

impl AiPatrol {
    /// 不带存证目录的构造（给单元测试用；进程里走 [`AiPatrol::with_jobs_dir`]）
    #[cfg(test)]
    pub fn new(agent: impl Into<String>, enabled: bool) -> Self {
        Self::with_jobs_dir(agent, enabled, None)
    }

    /// 同 [`AiPatrol::new`]，额外指定板端存证目录（`[ai] jobs_dir`）
    pub fn with_jobs_dir(
        agent: impl Into<String>,
        enabled: bool,
        jobs_dir: Option<String>,
    ) -> Self {
        let agent = agent.into().trim().trim_end_matches('/').to_string();
        let jobs_dir = jobs_dir
            .map(|dir| dir.trim().trim_end_matches('/').to_string())
            .filter(|dir| !dir.is_empty());
        Self {
            agent,
            enabled,
            probe: reqwest::Client::builder()
                .timeout(PROBE_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            action: reqwest::Client::builder()
                .timeout(ACTION_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            // SSE 是长连接：只限制建连时间，不能设总超时
            stream: reqwest::Client::builder()
                .connect_timeout(PROBE_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            window: Mutex::new(None),
            requested: Mutex::new(None),
            jobs_dir_pinned: jobs_dir.is_some(),
            jobs_dir: Mutex::new(jobs_dir),
            pending_cleanup: Mutex::new(Vec::new()),
            ending_plans: Mutex::new(Vec::new()),
        }
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// 当前是否有识别窗口（测试与排查用）
    #[cfg(test)]
    async fn status_active(&self) -> bool {
        self.window.lock().await.is_some()
    }

    /// 到点：探测 Agent → 上报 `ai_point`，等平台用同一套规则算好参数再回 `ai_start`。
    ///
    /// 探测失败（没接背包 / Agent 挂了）只打 WARN 并返回：不往平台发消息，
    /// 免得每跑一条普通路线就在事件流里留一条「AI 跳过」的噪音。
    pub async fn point_arrived(
        self: &Arc<Self>,
        out_tx: &mpsc::Sender<WsMessage>,
        task_code: &str,
        point: &AiPoint,
    ) {
        if !self.enabled {
            return;
        }
        {
            // 窗口已经在这个点上（或请求已发出）：5009 每 500ms 一帧，不能反复开窗
            let current = self.window.lock().await;
            if let Some(window) = current.as_ref()
                && window.task_code == task_code
                && window.point_index == point.index
            {
                return;
            }
            let mut guard = self.requested.lock().await;
            if guard.as_ref() == Some(&(task_code.to_string(), point.index)) {
                return;
            }
            *guard = Some((task_code.to_string(), point.index));
        }

        let tasks = match self.fetch_json("/api/detect/tasks").await {
            Ok(value) => value,
            Err(error) => {
                warn!(
                    "航点 AI 识别：连不上板端 Agent（{}）：{error}；跳过航点 {}「{}」",
                    self.agent, point.index, point.name
                );
                // 允许下一个点到点再试（Agent 可能只是刚重启）
                *self.requested.lock().await = None;
                return;
            }
        };
        let status = self.fetch_json("/api/detect/status").await.ok();
        let running = status
            .as_ref()
            .and_then(|value| value.get("running"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let cam = status
            .as_ref()
            .and_then(|value| value.get("default_cam"))
            .and_then(Value::as_str)
            .unwrap_or("dogcam")
            .to_string();

        info!(
            "航点 AI 识别：请求开窗 任务 {task_code} 航点 {}「{}」场景={:?} 类别={:?} 板端在跑={running} 相机={cam}",
            point.index, point.name, point.scene, point.labels
        );
        send(
            out_tx,
            "ai_point",
            json!({
                "taskCode": task_code,
                "pointIndex": point.index,
                "pointName": point.name,
                "dwell": point.dwell,
                "hint": point.hint,
                "scene": point.scene,
                "labels": point.labels,
                "threshold": point.threshold,
                // 任务类型 + 检查数量：服务端会以路线数据为准，这里带上便于
                // 旧版路线（没有 checkType 字段）也能核对
                "checkType": point.check_type,
                "expectCount": point.expect_count,
                "alertWhenMissing": point.alert_when_missing,
                "tasks": tasks,
                "agentRunning": running,
                "agentCam": cam,
            }),
        )
        .await;
    }

    /// 平台下发 `ai_start`：启停 Agent 并开始收事件
    pub async fn start(self: &Arc<Self>, out_tx: &mpsc::Sender<WsMessage>, start: AiStart) {
        if !self.enabled {
            return;
        }
        let cancel = Arc::new(Notify::new());
        // 同一计划号又开窗了（任务号复用 / 重跑）：上一次收尾还没删的整个计划目录
        // 现在装着**这一轮**的存证，不能再删（见 [`AiPatrol::ending_plans`]）。
        self.ending_plans
            .lock()
            .await
            .retain(|plan| plan != &start.plan_id);
        {
            let mut guard = self.window.lock().await;
            // 抢占：Agent 单路，先让旧窗口收尾（它自己判断要不要停 Agent）
            if let Some(previous) = guard.take() {
                previous.cancel.notify_one();
            }
            *guard = Some(Window {
                id: start.window_id.clone(),
                task_code: start.task_code.clone(),
                plan_id: start.plan_id.clone(),
                point_index: start.point_index,
                cancel: cancel.clone(),
            });
        }

        let this = self.clone();
        let out = out_tx.clone();
        tokio::spawn(async move {
            this.run_window(out, start, cancel).await;
        });
    }

    /// 停掉当前窗口（任务终态 / 暂停 / 取消时由主循环调用）
    pub async fn stop(&self, reason: &str) {
        let taken = self.window.lock().await.take();
        if let Some(window) = taken {
            info!("航点 AI 识别窗口关闭（{reason}）：窗口 {}", window.id);
            window.cancel.notify_one();
        }
        *self.requested.lock().await = None;
    }

    /// 窗口主流程：启停 Agent → 收 SSE → 逐类别上报 → 收尾
    async fn run_window(
        self: Arc<Self>,
        out_tx: mpsc::Sender<WsMessage>,
        start: AiStart,
        cancel: Arc<Notify>,
    ) {
        let body = json!({
            "task": start.task,
            "labels": if start.labels.is_empty() { Value::Null } else { json!(start.labels) },
            "conf": start.conf,
            "cams": [start.cam],
            "fps": start.fps,
            // 航点计划：板端据此把证据图 + result.json 落到
            // detect_jobs/<plan_id>/pt<航点下标>/，并按 expect_count 核对数量
            "plan_id": start.plan_id,
            "point_index": start.point_index,
            "check_type": start.check_type,
            "expect_count": start.expect_count,
            // 中台参数配置里的板端媒体目录 / 固定帧数（缺省由板端 config.py 兜底）
            "media_dir": start.media_dir,
            "save_frames": start.media_frames,
        });
        let path = if start.action == "config" {
            "/api/detect/config"
        } else {
            "/api/detect/start"
        };
        // 证据图的「新鲜度」基准就是本轮窗口开始（板端写完图再发 SSE，留余量）
        let since = SystemTime::now() - EVIDENCE_MTIME_SLACK;
        match self.post_json(path, &body).await {
            Ok(_) => info!(
                "航点 AI 识别开始：场景={} 类别={} 阈值={:.2} 相机={} 窗口={}s",
                start.task,
                if start.labels.is_empty() {
                    "全部".to_string()
                } else {
                    start.labels.join("/")
                },
                start.conf,
                start.cam,
                start.secs
            ),
            Err(error) => {
                warn!(
                    "航点 AI 识别：启动失败（场景 {}，{path}）: {error}",
                    start.task
                );
                // 启动就失败：窗口没真正开起来，别让「未命中」被算成数量不一致
                self.finish(
                    &out_tx,
                    &start,
                    &format!("启动失败: {error}"),
                    0,
                    false,
                    Leftover::default(),
                    since,
                )
                .await;
                return;
            }
        }

        let (hits, stopped_early, leftover) =
            self.consume(&out_tx, &start, &cancel, since).await;
        let reason = if stopped_early {
            "任务结束 / 被新航点顶替".to_string()
        } else {
            format!("窗口到期（{}s）", start.secs)
        };
        // `expired = true` 只代表「窗口正常到期」：数量任务这时一个都没找到，
        // 服务端才把它算成数量不一致（0 ≠ N）；被任务结束/新航点打断不算账。
        // （平台另外还会把「停满 dwell 后离开」当看满——窗口是 dwell+8，狗停到 dwell
        // 就走，只认到期会让「没找到也告警」永远报不出来，见 server/src/ai_patrol.rs。）
        self.finish(
            &out_tx,
            &start,
            &reason,
            hits,
            !stopped_early,
            leftover,
            since,
        )
        .await;
    }

    /// 读 SSE，把超过阈值的类别按「本窗口首次出现」上报。
    ///
    /// 返回（上报类别数, 是否被提前打断, 收尾要补的活）。
    ///
    /// `leftover` 交给 [`Self::finish`] 停掉板端后再补：板端 flush 计划时才把采样中的
    /// 命中落盘，那时才拿得到图——挂起的补上卡片，已经发过卡片的单独补一张图（`ai_evidence`）。
    async fn consume(
        &self,
        out_tx: &mpsc::Sender<WsMessage>,
        start: &AiStart,
        cancel: &Arc<Notify>,
        since: SystemTime,
    ) -> (usize, bool, Leftover) {
        let response = self
            .stream
            .get(format!("{}/api/events", self.agent))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await;
        let mut response = match response {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                warn!("航点 AI 识别：Agent 拒绝事件流 HTTP {}", response.status());
                return (0, false, Leftover::default());
            }
            Err(error) => {
                warn!("航点 AI 识别：连不上 Agent 事件流: {error}");
                return (0, false, Leftover::default());
            }
        };

        let notified = cancel.notified();
        tokio::pin!(notified);
        let deadline = Instant::now() + Duration::from_secs(start.secs.max(1));
        let mut buffer = String::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut hits = 0usize;
        // 数量核对告警每个窗口只上报一条（板端 `plan_hit` 也只发一次，双保险）
        let alerted = AtomicBool::new(false);
        // 等着带上证据图的检出（`label`, 置信度），见 [`EVIDENCE_PENDING_WAIT`] 的注释
        let mut pending: Vec<(String, f64)> = Vec::new();
        // 本窗口已拿到的命中帧（板端路径, base64）：同一窗口后来再命中别的类别直接复用
        let mut evidence: Option<(String, String)> = None;
        // 挂起检出的兜底上报时刻：一直等不到证据图也要把告警发出去
        let mut pending_deadline: Option<Instant> = None;
        // 有没有「卡片已经发出去、但没带上图」的检出：窗口收尾时补一张图（`ai_evidence`）
        let mut image_missing = false;

        let stopped_early = loop {
            let flush_at = pending_deadline.unwrap_or(deadline);
            tokio::select! {
                _ = &mut notified => break true,
                _ = tokio::time::sleep_until(deadline) => break false,
                // 兜底：板端迟迟不给证据图（老板端 / 画框失败）就先发卡片保告警
                _ = tokio::time::sleep_until(flush_at) => {
                    if pending.is_empty() {
                        // `flush_at` 就是窗口到期时刻，与上面那个分支同义
                        break false;
                    }
                    debug!(
                        "航点 AI 识别：{}s 内没等到证据图，先上报 {} 类检出",
                        EVIDENCE_PENDING_WAIT.as_secs(),
                        pending.len()
                    );
                    if let Some(found) = self.window_evidence(start, since, 1).await {
                        evidence = Some(found);
                    }
                    image_missing |= evidence.is_none();
                    for (label, conf) in std::mem::take(&mut pending) {
                        send_detect(out_tx, start, &label, conf, evidence.as_ref()).await;
                    }
                    pending_deadline = None;
                }
                chunk = response.chunk() => {
                    match chunk {
                        Ok(Some(bytes)) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));
                            if buffer.len() > SSE_BUF_LIMIT {
                                warn!("航点 AI 识别：SSE 缓冲超过上限，丢弃重来");
                                buffer.clear();
                                continue;
                            }
                            for payload in take_sse_events(&mut buffer) {
                                let Ok(event) = serde_json::from_str::<Value>(&payload) else {
                                    continue;
                                };
                                // 板端已判定「数量与检查目标数量不一致」：转发成 ai_alert
                                if let Some(mut alert) = count_alert(&event, start)
                                    && !alerted.swap(true, Ordering::Relaxed)
                                {
                                    let expect =
                                        alert.get("expectCount").and_then(|v| v.as_i64()).unwrap_or(0);
                                    let found =
                                        alert.get("foundCount").and_then(|v| v.as_i64()).unwrap_or(0);
                                    // 证据图只存在于板端磁盘，平台拿不到（Agent 没有取文件接口），
                                    // 只有本客户端能读。企业微信图片消息要的是字节不是路径，
                                    // 所以读出来随告警一起上报，由平台校验后发 msgtype=image。
                                    if let Some(image) = evidence_image_base64(&alert).await {
                                        alert["evidenceImage"] = json!(image);
                                    }
                                    warn!(
                                        "航点 AI 识别：数量核对不一致（检查 {expect} / 检出 {found}），上报告警"
                                    );
                                    send(out_tx, "ai_alert", alert).await;
                                }
                                // 板端 `plan_hit` 里的证据图路径能反推出存证根目录：记下来，
                                // 下一轮（甚至本轮后续航点）扫目录时就不用猜 `detect_jobs` 在哪
                                if let Some(root) = jobs_root_from_evidence(&event) {
                                    self.learn_jobs_root(root).await;
                                }
                                // 板端 `plan_hit` 才是「命中帧已画框落盘」的信号（检出那一刻
                                // 还在采样）：拿它把挂起的检出补上图再上报。路径优先用事件里
                                // 的 `evidence`，读不到（bind-mount / 换了盘符）就退回扫存证目录。
                                if event.get("type").and_then(Value::as_str) == Some("plan_hit")
                                    && !pending.is_empty()
                                {
                                    evidence = match plan_hit_evidence(&event).await {
                                        Some(found) => Some(found),
                                        None => self.window_evidence(start, since, 4).await,
                                    };
                                    if evidence.is_none() {
                                        debug!(
                                            "航点 AI 识别：plan_hit 没有可读的证据图，{} 类检出只发卡片",
                                            pending.len()
                                        );
                                        image_missing = true;
                                    }
                                    for (label, conf) in std::mem::take(&mut pending) {
                                        send_detect(out_tx, start, &label, conf, evidence.as_ref())
                                            .await;
                                    }
                                    pending_deadline = None;
                                }
                                for (label, conf) in take_hits(&event, start.conf, &mut seen) {
                                    hits += 1;
                                    debug!("航点 AI 识别：检出 {label}（{conf:.2}）");
                                    // 证据图这时通常还没落盘（见 [`EVIDENCE_PENDING_WAIT`]）。
                                    // 先探一次盘（`plan_hit` 后来的那些命中，图已经在盘上），
                                    // 探不到就先挂起等 `plan_hit`——群里那条卡片晚几秒，
                                    // 但后面跟得上证据图。
                                    if evidence.is_none() {
                                        evidence = self.window_evidence(start, since, 1).await;
                                    }
                                    if let Some(found) = evidence.as_ref() {
                                        send_detect(out_tx, start, &label, conf, Some(found)).await;
                                        continue;
                                    }
                                    pending.push((label, conf));
                                    pending_deadline.get_or_insert_with(|| {
                                        Instant::now() + EVIDENCE_PENDING_WAIT
                                    });
                                }
                            }
                        }
                        Ok(None) => break false,
                        Err(error) => {
                            warn!("航点 AI 识别：事件流中断: {error}");
                            break false;
                        }
                    }
                }
            }
        };
        (
            hits,
            stopped_early,
            Leftover {
                pending,
                image_missing,
            },
        )
    }

    /// 收尾：按需停 Agent →（补发挂起的检出 / 补一张证据图）→ 上报 `ai_end` → 释放窗口槽位
    #[allow(clippy::too_many_arguments)]
    async fn finish(
        &self,
        out_tx: &mpsc::Sender<WsMessage>,
        start: &AiStart,
        reason: &str,
        hits: usize,
        expired: bool,
        leftover: Leftover,
        since: SystemTime,
    ) {
        // 与平台侧 `replaced_by_other` 同一语义：窗口已被**新窗口**顶替时不停 Agent
        // （新窗口可能继承着操作员手动开的识别）；被 stop()/任务终态摘掉时窗口为空，
        // 这时 `replaced = false`，必须把 Agent 停掉，否则 NPU 一直占着。
        let replaced = {
            let guard = self.window.lock().await;
            guard
                .as_ref()
                .map(|window| window.id != start.window_id)
                .unwrap_or(false)
        };
        if start.was_running {
            info!("航点 AI 识别窗口结束（保留操作员已开启的识别）：{reason}");
            // 识别留着，但计划必须摘掉：否则之后画面再命中会继续写这个航点的
            // 存证目录、并可能重复上报数量告警。
            if !replaced
                && let Err(error) = self.post_json("/api/detect/plan/clear", &json!({})).await
            {
                debug!("航点 AI 识别：清除板端计划失败（不影响后续巡检）: {error}");
            }
        } else if !replaced
            && let Err(error) = self.post_json("/api/detect/stop", &json!({})).await
        {
            warn!("航点 AI 识别：停识别失败: {error}");
        }
        // 到窗口结束还没等到 `plan_hit` 的检出（命中后板端还在采样）：停识别会让板端
        // `_plan_flush` 把采样中的命中画框存证，所以**先停再扫**一次盘，往往能补上图。
        // 停之前扫是白扫——那时图根本还没写。
        if !leftover.pending.is_empty() || leftover.image_missing {
            let evidence = self.window_evidence(start, since, 4).await;
            // 已经发过卡片的（`image_missing`）由后面单独补一张图，所以这里补发的卡片
            // 就不再带图了——否则同一窗口会在群里冒出两张一样的命中帧。
            let attach = if leftover.image_missing {
                None
            } else {
                evidence.as_ref()
            };
            if !leftover.pending.is_empty() && attach.is_none() {
                debug!(
                    "航点 AI 识别：窗口收尾时仍没有证据图，{} 类检出只发卡片",
                    leftover.pending.len()
                );
            }
            for (label, conf) in leftover.pending {
                send_detect(out_tx, start, &label, conf, attach).await;
            }
            if leftover.image_missing {
                match evidence {
                    Some((path, image)) => {
                        debug!(
                            "航点 AI 识别：卡片已发、图才落盘，单独补一张证据图（{path}）"
                        );
                        send_evidence(out_tx, start, &path, &image).await;
                    }
                    None => debug!("航点 AI 识别：窗口收尾时仍没有证据图，不再补图"),
                }
            }
        }
        info!("航点 AI 识别结束：场景={} {reason}，上报 {hits} 类目标", start.task);

        send(
            out_tx,
            "ai_end",
            json!({
                "windowId": start.window_id,
                "task": start.task,
                "reason": reason,
                "hits": hits,
                // 窗口是否正常到期：数量任务未命中时服务端据此判「数量不一致」
                "expired": expired,
            }),
        )
        .await;

        if !replaced {
            *self.window.lock().await = None;
        }
        // 平台已经把这些航点的证据图推进群了：窗口收尾（含停识别后的补图扫描）
        // 之后才真删板端存证。还属于「当前活动窗口」的条目留着，不能提前删。
        let active = self
            .window
            .lock()
            .await
            .as_ref()
            .map(|window| (window.plan_id.clone(), window.point_index));
        let (ready, keep): (Vec<CleanupTarget>, Vec<CleanupTarget>) = {
            let mut pending = self.pending_cleanup.lock().await;
            std::mem::take(&mut *pending)
                .into_iter()
                .partition(|item| active.as_ref() != Some(item))
        };
        if !keep.is_empty() {
            *self.pending_cleanup.lock().await = keep;
        }
        for (plan_id, point_index) in ready {
            self.remove_point_dir(&plan_id, point_index, "窗口收尾")
                .await;
        }
    }

    /// 存证根目录（`detect_jobs`）：显式配置优先，否则试内置默认位置并记住能用的那个。
    async fn jobs_root(&self) -> Option<String> {
        if let Some(dir) = self.jobs_dir.lock().await.clone() {
            return Some(dir);
        }
        for candidate in DEFAULT_JOBS_DIRS {
            if tokio::fs::metadata(candidate).await.is_ok() {
                *self.jobs_dir.lock().await = Some(candidate.to_string());
                debug!("航点 AI 识别：命中帧存证目录 {candidate}");
                return Some(candidate.to_string());
            }
        }
        None
    }

    /// 板端 `plan_hit.evidence` 反推出的真实存证根：比猜默认路径可靠，学到就钉住。
    async fn learn_jobs_root(&self, root: String) {
        if self.jobs_dir_pinned {
            return;
        }
        let mut guard = self.jobs_dir.lock().await;
        if guard.as_deref() != Some(root.as_str()) {
            debug!("航点 AI 识别：从板端事件学到存证目录 {root}");
            *guard = Some(root);
        }
    }

    /// 平台把证据图推进企业微信群之后，删掉板端这一轮的存证
    /// （`detect_jobs/<计划>/pt<航点>/` 里的画框 JPG + `result.json`）。
    ///
    /// 为什么要删：板端每命中一次写一张图，巡检一天几十轮，板子存储很快被图片吃满；
    /// 而图进了群之后没有第二次用途（平台读不到板端磁盘）。
    ///
    /// 窗口还开着就先记账（[`AiPatrol::pending_cleanup`]）：那时板端可能还在写图，
    /// 客户端收尾时也要再扫一次存证目录补图（`ai_evidence`），删早了会把还没推出去的
    /// 那张图毁掉。窗口收尾后由 [`AiPatrol::finish`] 统一删。
    pub async fn cleanup_evidence(&self, plan_id: &str, point_index: i64, reason: &str) {
        let plan_id = plan_id.trim();
        if !safe_plan_id(plan_id) || point_index < 0 {
            debug!(
                "航点 AI 识别：存证清理载荷不合法（plan={plan_id:?} point={point_index}），忽略"
            );
            return;
        }
        {
            let guard = self.window.lock().await;
            if let Some(window) = guard.as_ref()
                && window.plan_id == plan_id
                && window.point_index == point_index
            {
                debug!(
                    "航点 AI 识别：窗口还没收尾，存证 {plan_id}/pt{point_index} 等收尾后再删（{reason}）"
                );
                self.pending_cleanup
                    .lock()
                    .await
                    .push((plan_id.to_string(), point_index));
                return;
            }
        }
        self.remove_point_dir(plan_id, point_index, reason).await;
    }

    /// 真删 `<存证根>/<计划>/pt<n>/`（图和 `result.json` 都在这一层）。
    async fn remove_point_dir(&self, plan_id: &str, point_index: i64, reason: &str) {
        let Some(root) = self.jobs_root().await else {
            debug!("航点 AI 识别：找不到存证根目录，跳过清理 {plan_id}/pt{point_index}");
            return;
        };
        let Some(dir) = point_dir(&root, plan_id, point_index) else {
            return;
        };
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => info!("航点 AI 识别：已清理板端存证 {}（{reason}）", dir.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                debug!("航点 AI 识别：板端存证已经不在了（{}）", dir.display())
            }
            Err(error) => warn!(
                "航点 AI 识别：清理板端存证失败（{}）: {error}",
                dir.display()
            ),
        }
        // 计划目录空了就顺手删掉，别在 detect_jobs 下留一堆空壳（非空时 remove_dir 会失败）
        let _ = tokio::fs::remove_dir(Path::new(&root).join(plan_id.trim())).await;
    }

    /// 计划（任务）跑完了（终态 / 取消）：把板端**整个计划目录**删掉。
    ///
    /// 为什么不能只靠 [`AiPatrol::cleanup_evidence`]：那条是「这一张证据图已经进群，
    /// 删这个航点」，只有真的推过图的航点才有。数量任务的检出只写任务时间线（不建单
    /// 不推群）、一个都没命中的航点只留一份 `result.json`、图被企业微信拒收 / 设备当时
    /// 不在线——这些都等不到 `ai_cleanup`，`detect_jobs/<计划>/` 于是长期堆在板子上。
    /// 而任务都跑完了，这些图已经没有任何用途（图只在板端、平台读不到）。
    ///
    /// 窗口还在收尾时不能当场删（收尾要停识别、板端 `_plan_flush` 落盘、再扫盘补图，
    /// 删早了会把还没推出去的那张图毁掉），所以先记账、起个短任务等它跑完再删。
    pub async fn finish_plan(self: &Arc<Self>, plan_id: &str, reason: &str) {
        let plan_id = plan_id.trim();
        if !safe_plan_id(plan_id) {
            debug!("航点 AI 识别：计划 id 不合法（{plan_id:?}），跳过存证清理");
            return;
        }
        {
            let mut ending = self.ending_plans.lock().await;
            if !ending.iter().any(|plan| plan == plan_id) {
                ending.push(plan_id.to_string());
            }
        }
        if self.plan_window_active(plan_id).await {
            debug!("航点 AI 识别：计划 {plan_id} 的窗口还在收尾，存证等它跑完再删（{reason}）");
            let this = self.clone();
            let plan_id = plan_id.to_string();
            let reason = reason.to_string();
            tokio::spawn(async move { this.reap_plan_dir(plan_id, reason).await });
            return;
        }
        self.ending_plans
            .lock()
            .await
            .retain(|plan| plan != plan_id);
        self.remove_plan_dir(plan_id, reason).await;
    }

    /// 计划目录是不是正被某个窗口用着（`pt<n>/` 还在写、还要扫图）
    async fn plan_window_active(&self, plan_id: &str) -> bool {
        self.window
            .lock()
            .await
            .as_ref()
            .map(|window| window.plan_id == plan_id)
            .unwrap_or(false)
    }

    /// 等这一轮窗口收尾，再删整个计划目录（[`AiPatrol::finish_plan`] 的延迟分支）。
    async fn reap_plan_dir(&self, plan_id: String, reason: String) {
        for _ in 0..PLAN_CLEANUP_ATTEMPTS {
            // 新一轮同名计划开始了：这次跑出来的存证不能删
            if !self
                .ending_plans
                .lock()
                .await
                .iter()
                .any(|plan| plan == &plan_id)
            {
                return;
            }
            if !self.plan_window_active(&plan_id).await {
                break;
            }
            tokio::time::sleep(PLAN_CLEANUP_RETRY).await;
        }
        if self.plan_window_active(&plan_id).await {
            warn!("航点 AI 识别：计划 {plan_id} 的窗口迟迟不收尾，存证先留着（下次启动兜底清理）");
            return;
        }
        self.ending_plans
            .lock()
            .await
            .retain(|plan| plan != &plan_id);
        self.remove_plan_dir(&plan_id, &reason).await;
    }

    /// 真删 `<存证根>/<计划>/`（整个计划，含从没推过图的航点）
    async fn remove_plan_dir(&self, plan_id: &str, reason: &str) {
        if !safe_plan_id(plan_id) {
            return;
        }
        let Some(root) = self.jobs_root().await else {
            debug!("航点 AI 识别：找不到存证根目录，跳过清理计划 {plan_id}");
            return;
        };
        let dir = Path::new(&root).join(plan_id.trim());
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => info!(
                "航点 AI 识别：已清理板端计划存证 {}（{reason}）",
                dir.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                debug!("航点 AI 识别：板端计划存证已经不在了（{}）", dir.display())
            }
            Err(error) => warn!(
                "航点 AI 识别：清理板端计划存证失败（{}）: {error}",
                dir.display()
            ),
        }
    }

    /// 客户端启动时的兜底清理：`detect_jobs` 下超过 [`STALE_PLAN_AGE`] 没动过的
    /// 目录直接删掉（返回删了几个）。
    ///
    /// 正常路径是 [`AiPatrol::finish_plan`] 在任务结束时清；这里兜的是「那次收尾没
    /// 跑到」——客户端中途被 kill / 断电 / 还是老版本（只删推过图的航点），板子上的
    /// `detect_jobs` 只增不减，很快就会把盘写满。
    pub async fn sweep_stale_plans(&self) -> usize {
        self.sweep_plans_older_than(STALE_PLAN_AGE).await
    }

    /// 按年龄清残留（`age` 只是给单测用的入口）
    async fn sweep_plans_older_than(&self, age: Duration) -> usize {
        let Some(root) = self.jobs_root().await else {
            return 0;
        };
        let now = SystemTime::now();
        let Ok(mut entries) = tokio::fs::read_dir(&root).await else {
            return 0;
        };
        let mut removed = 0usize;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !entry.file_type().await.map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !safe_plan_id(&name) {
                continue;
            }
            // 正在跑 / 刚跑完还没收尾的计划不能动
            if self.plan_window_active(&name).await
                || self
                    .ending_plans
                    .lock()
                    .await
                    .iter()
                    .any(|plan| plan == &name)
            {
                continue;
            }
            let Some(last_touch) = dir_last_touch(&path).await else {
                continue;
            };
            if now.duration_since(last_touch).unwrap_or_default() < age {
                continue;
            }
            match tokio::fs::remove_dir_all(&path).await {
                Ok(()) => {
                    removed += 1;
                    info!("航点 AI 识别：清理残留的计划存证 {}", path.display());
                }
                Err(error) => warn!(
                    "航点 AI 识别：清理残留的计划存证失败（{}）: {error}",
                    path.display()
                ),
            }
        }
        if removed > 0 {
            info!("航点 AI 识别：启动兜底清理了 {removed} 个残留的计划存证目录");
        }
        removed
    }

    /// 本轮窗口的一张命中帧：（板端路径, base64）。
    ///
    /// 这是「拿不到 `plan_hit` 的 path 时」的退路：扫板端存证目录
    /// `<存证根>/<计划 id>/pt<航点下标>/`，取「窗口开始之后写下的」最新一张。
    /// `attempts` 是重试次数（每次间隔 300ms）：窗口收尾时给几次就够，兜底上报可以只给 1。
    /// 失败一律 `None`：**不带图不能影响告警**。
    async fn window_evidence(
        &self,
        start: &AiStart,
        since: SystemTime,
        attempts: usize,
    ) -> Option<(String, String)> {
        let plan = start.plan_id.trim();
        if plan.is_empty() || attempts == 0 {
            return None;
        }
        for attempt in 0..attempts {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            let root = self.jobs_root().await?;
            let dir = Path::new(&root)
                .join(plan)
                .join(format!("pt{}", start.point_index));
            let Some(path) = newest_evidence_file(&dir, since).await else {
                continue;
            };
            if let Some(image) = evidence_image_from_path(&path).await {
                return Some((path, image));
            }
        }
        None
    }

    async fn fetch_json(&self, path: &str) -> Result<Value, String> {
        let response = self
            .probe
            .get(format!("{}{path}", self.agent))
            .send()
            .await
            .map_err(|error| format!("请求失败: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "上游返回 {status}: {}",
                text.chars().take(160).collect::<String>()
            ));
        }
        serde_json::from_str(&text).map_err(|error| format!("响应不是合法 JSON: {error}"))
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value, String> {
        let response = self
            .action
            .post(format!("{}{path}", self.agent))
            .json(body)
            .send()
            .await
            .map_err(|error| format!("请求失败: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if payload.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(payload);
        }
        let reason = payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(&text)
            .to_string();
        Err(if reason.trim().is_empty() {
            format!("上游返回 {status}")
        } else {
            reason
        })
    }
}

/* ------------------------------- 纯函数 ------------------------------- */

/// 动作里的「检查目标数量」：数字或数字字符串（编辑器是输入框），
/// 空 / 0 / 非法一律当「不核对数量」（`None`），上限 999 防脏数据。
fn parse_expect_count(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let number = value
        .as_i64()
        .or_else(|| value.as_f64().map(|raw| raw as i64))
        .or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .and_then(|text| text.parse::<f64>().ok())
                .map(|raw| raw as i64)
        })?;
    (number >= 1).then(|| number.min(999))
}

/// 任务类型归一化：`exist`（检测是否存在）/ `count`（检测数量是否正确）。
///
/// 兼容三种来源：新编辑器写 `checkType`；老数据只有 `expectCount`（给了数量就按
/// 数量任务算）；两者都没有就是存在性任务。中文别名也认，避免手改路由数据时踩坑。
fn normalize_check_type(explicit: Option<&str>, expect: Option<i64>) -> String {
    let text = explicit.map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return if expect.is_some() { "count" } else { "exist" }.to_string();
    }
    let lower = text.to_ascii_lowercase();
    if lower == "count" || lower == "quantity" || text.contains("数量") {
        "count".to_string()
    } else {
        "exist".to_string()
    }
}

/// 板端 `plan_hit` 事件 → `ai_alert` 载荷。
///
/// 只有 `countAlert` 为真时才转发（板端已经把「没配检查数量」的情况过滤掉了，
/// 这里再判一次，免得将来事件字段改动就把普通命中当成告警刷上去）。
fn count_alert(event: &Value, start: &AiStart) -> Option<Value> {
    if event.get("type").and_then(Value::as_str) != Some("plan_hit") {
        return None;
    }
    if event.get("count_alert").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    Some(json!({
        "windowId": start.window_id,
        "taskCode": start.task_code,
        "planId": start.plan_id,
        "pointIndex": start.point_index,
        "expectCount": event
            .get("expect_count")
            .and_then(Value::as_i64)
            .or(start.expect_count),
        "foundCount": event.get("found_count").and_then(Value::as_i64).unwrap_or(0),
        "evidence": event.get("evidence").and_then(Value::as_str).unwrap_or_default(),
    }))
}

/* ------------------------------- 证据图 ------------------------------- */

/// 上报一条检出（`ai_detect`）。
///
/// `evidence` = （板端路径, base64）：拿到命中帧就带图，没拿到就只发卡片
/// （见 [`EVIDENCE_PENDING_WAIT`]——先挂起等板端落盘，等不到也不能把告警拖没了）。
async fn send_detect(
    out_tx: &mpsc::Sender<WsMessage>,
    start: &AiStart,
    label: &str,
    conf: f64,
    evidence: Option<&(String, String)>,
) {
    let mut detect = json!({
        "windowId": start.window_id,
        "task": start.task,
        "planId": start.plan_id,
        "pointIndex": start.point_index,
        "label": label,
        "conf": conf,
    });
    if let Some((path, image)) = evidence {
        detect["evidence"] = json!(path);
        detect["evidenceImage"] = json!(image);
    }
    send(out_tx, "ai_detect", detect).await;
}

/// 补发一张命中帧（`ai_evidence`）：卡片已经发过了，平台只发 `msgtype=image`。
///
/// 只在「告警早就发出去、板端到窗口收尾才把图落盘」时用（见 [`Leftover`]）：
/// 平台不会重发卡片，所以群里看到的就是「卡片在前、图在后」，与正常情况一致。
async fn send_evidence(
    out_tx: &mpsc::Sender<WsMessage>,
    start: &AiStart,
    path: &str,
    image: &str,
) {
    send(
        out_tx,
        "ai_evidence",
        json!({
            "windowId": start.window_id,
            "taskCode": start.task_code,
            "planId": start.plan_id,
            "pointIndex": start.point_index,
            "evidence": path,
            "evidenceImage": image,
        }),
    )
    .await;
}

/// 板端 `plan_hit.evidence` 指向的命中帧：（路径, base64）。
///
/// `plan_hit` 是板端「已经把这一帧画框存证」的信号，文件在这条事件之前就写好了，
/// 所以正常情况下一次就能读到。读不到（bind-mount / 换盘符）返回 `None`，
/// 由调用方退回扫存证目录（`AiPatrol::window_evidence`）。
async fn plan_hit_evidence(event: &Value) -> Option<(String, String)> {
    let path = event
        .get("evidence")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())?
        .to_string();
    let image = evidence_image_from_path(&path).await?;
    Some((path, image))
}

/// 把「数量核对不一致」告警里的证据图读成 base64（供平台发企业微信图片消息）。
///
/// 图片在板端本地磁盘（`detect_jobs/<plan>/pt<n>/dogcam_1.jpg`），平台读不到，
/// 只有本客户端（与 Agent 同机）能读。失败一律返回 `None`——**不带图不能影响告警**：
/// 卡片里仍然有路径，现场可 ssh 上去看。
pub(crate) async fn evidence_image_base64(alert: &Value) -> Option<String> {
    let path = alert
        .get("evidence")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())?;
    evidence_image_from_path(path).await
}

/// 读文件 → 体积/格式校验 → base64（无换行）。
///
/// 只认 JPEG / PNG（按文件头，不信扩展名）：企业微信图片消息只收这两种，
/// 且原图 ≤ 2MB。其余情况打一行 debug 就放弃，不刷 WARN（每轮巡检都会重试）。
async fn evidence_image_from_path(path: &str) -> Option<String> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;

    if let Ok(meta) = tokio::fs::metadata(path).await
        && meta.len() > EVIDENCE_IMAGE_MAX_BYTES
    {
        debug!(
            "航点 AI 识别：证据图 {} 字节，超过企业微信 2MB 上限，不带图（{path}）",
            meta.len()
        );
        return None;
    }
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) => {
            // 客户端不在板端 / 路径不在本机 / 无权限：都不带图，卡片里仍有路径
            debug!("航点 AI 识别：证据图读不到（{path}）: {error}");
            return None;
        }
    };
    if bytes.len() > EVIDENCE_IMAGE_MAX_BYTES as usize
        || !(bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]))
    {
        debug!("航点 AI 识别：证据图不是 JPEG/PNG 或超过 2MB，不带图（{path}）");
        return None;
    }
    debug!(
        "航点 AI 识别：证据图随告警上报（{} 字节，{path}）",
        bytes.len()
    );
    Some(BASE64.encode(&bytes))
}

/// 从板端 `plan_hit.evidence`（`<存证根>/<计划>/pt<n>/dogcam_1.jpg`）反推存证根目录。
///
/// 板子上的路径前缀不统一（`/root/backpack_dashboard/backend`、
/// `/opt/xjpt/backpack_dashboard/backend`、`/root/261113/backpack_dashboard/backend` …），
/// 拿板端自己写出来的路径当唯一真相，
/// 比硬编码候选靠谱。形如不对就返回 `None`（宁可带不了图，也不要拼出错路径）。
fn jobs_root_from_evidence(event: &Value) -> Option<String> {
    if event.get("type").and_then(Value::as_str) != Some("plan_hit") {
        return None;
    }
    let evidence = event
        .get("evidence")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())?;
    let point_dir = Path::new(evidence).parent()?;
    // 目录名必须是 `pt<n>`：否则说明板端换了存证布局，反推出来的根不可信
    if !point_dir
        .file_name()
        .map(|name| name.to_string_lossy().starts_with("pt"))
        .unwrap_or(false)
    {
        return None;
    }
    let root = point_dir.parent()?.parent()?;
    let root = root.to_string_lossy().trim_end_matches('/').to_string();
    Some(root).filter(|root| !root.is_empty())
}

/// 计划 id 能不能安全地拼进存证路径。
///
/// 删目录不可逆，所以对平台下发的 `planId` 只做「拒绝」不做替换：只认字母 / 数字 /
/// 汉字 / `-` / `_`，其余（空、路径分隔符、`..`、控制字符）一律不删。
/// 正常任务号（`TK-1112-11`）和板端 `evidence.py::_safe_job_id` 的产物都落在这个范围里。
fn safe_plan_id(plan_id: &str) -> bool {
    let plan_id = plan_id.trim();
    !plan_id.is_empty()
        && plan_id
            .chars()
            .all(|ch| ch.is_alphanumeric() || ch == '-' || ch == '_')
}

/// 板端存证目录：`<存证根>/<计划>/pt<航点下标>`（与 `evidence.py::plan_dir` 对齐）。
fn point_dir(root: &str, plan_id: &str, point_index: i64) -> Option<std::path::PathBuf> {
    if !safe_plan_id(plan_id) || point_index < 0 {
        return None;
    }
    Some(
        Path::new(root)
            .join(plan_id.trim())
            .join(format!("pt{point_index}")),
    )
}

/// 存证目录「最后一次动过」的时间：取自己、直接子目录、以及子目录里文件的 mtime 最大值。
///
/// 为什么不只看目录自己的 mtime：计划目录（`detect_jobs/<计划>/`）只在增删 `pt<n>/`
/// 时变，同一计划每轮巡检都是在同样的航点上**覆盖写**图（`pt<n>/` 和 `<计划>/` 的
/// mtime 都不动），只看目录会把「今天还在用」的计划当成几天前的残留删掉。
async fn dir_last_touch(dir: &Path) -> Option<SystemTime> {
    let newest_of = |a: Option<SystemTime>, b: SystemTime| Some(a.map_or(b, |prev| prev.max(b)));
    let mut newest = tokio::fs::metadata(dir)
        .await
        .ok()
        .and_then(|meta| meta.modified().ok());
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return newest;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(Ok(time)) = entry.metadata().await.map(|meta| meta.modified()) {
            newest = newest_of(newest, time);
        }
        if !entry
            .file_type()
            .await
            .map(|kind| kind.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        if let Ok(mut inner) = tokio::fs::read_dir(entry.path()).await {
            while let Ok(Some(file)) = inner.next_entry().await {
                if let Ok(Ok(time)) = file.metadata().await.map(|meta| meta.modified()) {
                    newest = newest_of(newest, time);
                }
            }
        }
    }
    newest
}

/// 存证目录里「本轮写下的」最新一张图（JPG/PNG）。///
/// 只认 mtime ≥ `since` 的文件：同一航点每轮巡检覆盖同名文件，不卡时间就会把
/// **上一轮**的图当成这一帧的证据发给群里（比没图更糟）。
async fn newest_evidence_file(dir: &Path, since: SystemTime) -> Option<String> {
    let mut entries = tokio::fs::read_dir(dir).await.ok()?;
    let mut best: Option<(SystemTime, String)> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !(name.ends_with(".jpg") || name.ends_with(".jpeg") || name.ends_with(".png")) {
            continue;
        }
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if meta.len() > EVIDENCE_IMAGE_MAX_BYTES {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if modified < since {
            continue;
        }
        if best.as_ref().map(|(at, _)| modified > *at).unwrap_or(true) {
            best = Some((modified, path.to_string_lossy().to_string()));
        }
    }
    best.map(|(_, path)| path)
}

/// 从 SSE 缓冲里取出完整事件（以空行分隔），残留部分留在缓冲里等下一块
pub(crate) fn take_sse_events(buffer: &mut String) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(index) = buffer.find("\n\n") {
        let block: String = buffer.drain(..index + 2).collect();
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                let payload = rest.trim();
                if !payload.is_empty() {
                    out.push(payload.to_string());
                }
            }
        }
    }
    out
}

/// 一帧 `detect` 事件里「超过阈值、且本窗口还没报过」的类别。
///
/// 阈值过滤与去重都在客户端做：SSE 帧率是 3~6fps，原样转发 30s 窗口会往平台
/// 推上百条同样的框；平台只负责落告警，不再重复判断。
fn take_hits(event: &Value, threshold: f64, seen: &mut HashSet<String>) -> Vec<(String, f64)> {
    if event.get("type").and_then(Value::as_str) != Some("detect") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let Some(boxes) = event.get("boxes").and_then(Value::as_array) else {
        return out;
    };
    for item in boxes {
        let Some(label) = item.get("label").and_then(Value::as_str) else {
            continue;
        };
        let Some(conf) = item.get("conf").and_then(Value::as_f64) else {
            continue;
        };
        if conf < threshold || seen.contains(label) {
            continue;
        }
        seen.insert(label.to_string());
        out.push((label.to_string(), conf));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> Value {
        json!({
            "points": [
                {
                    "name": "新航点 1",
                    "dwell": 100,
                    "actions": [{
                        "type": "AI 识别",
                        "scene": "firextinguisher",
                        "labels": ["extinguisher"],
                        "threshold": "置信度 ≥ 0.35",
                        "param": "灭火器 · 灭火器"
                    }]
                },
                {"name": "新航点 2", "actions": [{"type": "可见光拍照", "param": "1080P"}]},
                {"name": "新航点 3", "dwell": 6, "actions": []}
            ]
        })
    }

    #[test]
    fn ai_points_read_from_route_data() {
        let points = parse_ai_points(&route());
        assert_eq!(points.len(), 1);
        let point = &points[0];
        assert_eq!(point.index, 0);
        assert_eq!(point.name, "新航点 1");
        assert_eq!(point.dwell, 100);
        assert_eq!(point.scene.as_deref(), Some("firextinguisher"));
        assert_eq!(point.labels, vec!["extinguisher".to_string()]);
        assert_eq!(point.threshold, "置信度 ≥ 0.35");
        assert_eq!(point.hint, "灭火器 · 灭火器");
        assert_eq!(point.expect_count, None, "路线里没配就是 None（不核对数量）");
        assert!(ai_point_at(&points, 1).is_none(), "拍照点不该触发识别");
        assert!(ai_point_at(&points, 0).is_some());
    }

    #[test]
    fn expect_count_is_parsed_leniently_but_never_zero() {
        // 编辑器写数字，历史数据/手改可能是字符串
        assert_eq!(parse_expect_count(Some(&json!(3))), Some(3));
        assert_eq!(parse_expect_count(Some(&json!("4"))), Some(4));
        assert_eq!(parse_expect_count(Some(&json!(" 5 "))), Some(5));
        // 留空 / 0 / 负数 / 非法 = 不核对数量
        assert_eq!(parse_expect_count(None), None);
        assert_eq!(parse_expect_count(Some(&Value::Null)), None);
        assert_eq!(parse_expect_count(Some(&json!(0))), None);
        assert_eq!(parse_expect_count(Some(&json!(""))), None);
        assert_eq!(parse_expect_count(Some(&json!("abc"))), None);
        // 上限防止脏数据
        assert_eq!(parse_expect_count(Some(&json!(100000))), Some(999));

        let points = parse_ai_points(&json!({
            "points": [{"name": "P1", "actions": [{"type": "AI 识别", "expectCount": "2"}]}]
        }));
        assert_eq!(points[0].expect_count, Some(2));
    }

    #[test]
    fn check_type_is_explicit_or_inferred_from_expect_count() {
        // 显式字段优先
        assert_eq!(normalize_check_type(Some("count"), None), "count");
        assert_eq!(normalize_check_type(Some("exist"), Some(5)), "exist");
        // 中文 / 别名
        assert_eq!(normalize_check_type(Some("数量"), None), "count");
        assert_eq!(normalize_check_type(Some("检测数量是否正确"), None), "count");
        assert_eq!(normalize_check_type(Some("presence"), None), "exist");
        // 老数据没 checkType：给了 expectCount 就是数量任务，否则存在性任务
        assert_eq!(normalize_check_type(None, Some(3)), "count");
        assert_eq!(normalize_check_type(None, None), "exist");
    }

    #[test]
    fn check_type_reaches_agent_and_route_parse() {
        let points = parse_ai_points(&json!({
            "points": [
                {"name": "存在性", "actions": [{"type": "AI 识别", "checkType": "exist"}]},
                {"name": "数量", "actions": [{"type": "AI 识别", "expectCount": 2}]},
                {"name": "老数据", "actions": [{"type": "AI 识别"}]}
            ]
        }));
        assert_eq!(points[0].check_type, "exist");
        assert_eq!(points[1].check_type, "count", "老数据靠 expectCount 推断");
        assert_eq!(points[2].check_type, "exist");
    }

    #[test]
    fn count_alert_only_for_flagged_plan_hits() {
        let start = AiStart::from_value(&json!({
            "windowId": "AIW-1-0-1",
            "taskCode": "TK-1",
            "planId": "TK-1",
            "pointIndex": 0,
            "expectCount": 3,
        }))
        .expect("构造开窗指令失败");
        // 命中但数量一致：不告警
        assert!(
            count_alert(
                &json!({"type": "plan_hit", "count_ok": true, "count_alert": false,
                        "expect_count": 3, "found_count": 3}),
                &start,
            )
            .is_none()
        );
        // 普通 detect 事件不告警
        assert!(count_alert(&json!({"type": "detect", "boxes": []}), &start).is_none());
        let alert = count_alert(
            &json!({"type": "plan_hit", "count_alert": true, "expect_count": 3,
                    "found_count": 1, "evidence": "/x/y.jpg"}),
            &start,
        )
        .expect("数量不一致要告警");
        assert_eq!(alert["windowId"], "AIW-1-0-1");
        assert_eq!(alert["planId"], "TK-1");
        assert_eq!(alert["expectCount"], 3);
        assert_eq!(alert["foundCount"], 1);
        assert_eq!(alert["evidence"], "/x/y.jpg");
    }

    /// 证据图随告警上报：客户端与板端 Agent 同机，**只有它**能读到板端磁盘上的图，
    /// 企业微信图片消息要的是字节不是路径。读不到 / 不是 JPG-PNG / 超 2MB 一律不传，
    /// 告警本身不受影响（卡片里仍有路径）。
    #[tokio::test]
    async fn evidence_image_reads_board_file_and_skips_unusable_ones() {
        let dir = std::env::temp_dir().join(format!("robodog-evidence-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // ① 正常 JPEG（只看文件头）
        let jpeg = dir.join("dogcam_1.jpg");
        std::fs::write(&jpeg, [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).unwrap();
        let alert = json!({ "evidence": jpeg.to_string_lossy() });
        let encoded = evidence_image_base64(&alert).await.expect("JPEG 应能读出来");
        assert_eq!(encoded, "/9j/4AAQ", "base64 不能带换行");

        // ② 文件不在（客户端不在板端 / 路径不在本机）→ 不传图
        let missing = json!({ "evidence": dir.join("nope.jpg").to_string_lossy() });
        assert!(evidence_image_base64(&missing).await.is_none());

        // ③ 不是图片（只看文件头，不信扩展名）
        let fake = dir.join("fake.jpg");
        std::fs::write(&fake, b"hello world").unwrap();
        let fake_alert = json!({ "evidence": fake.to_string_lossy() });
        assert!(evidence_image_base64(&fake_alert).await.is_none());

        // ④ 超过企业微信 2MB 上限
        let huge = dir.join("huge.jpg");
        let mut bytes = vec![0xFF_u8, 0xD8, 0xFF];
        bytes.resize(EVIDENCE_IMAGE_MAX_BYTES as usize + 1, 0);
        std::fs::write(&huge, &bytes).unwrap();
        let huge_alert = json!({ "evidence": huge.to_string_lossy() });
        assert!(evidence_image_base64(&huge_alert).await.is_none());

        // ⑤ 没给路径（老客户端 / 事件没带 evidence）
        assert!(evidence_image_base64(&json!({})).await.is_none());
        assert!(evidence_image_base64(&json!({ "evidence": "  " })).await.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 板端 `plan_hit.evidence` 能反推出存证根目录（板子路径前缀不统一，不硬编码）
    #[test]
    fn jobs_root_is_learned_from_plan_hit_evidence() {
        assert_eq!(
            jobs_root_from_evidence(&json!({
                "type": "plan_hit",
                "evidence": "/root/261113/backpack_dashboard/backend/detect_jobs/TK-1/pt0/dogcam_1.jpg"
            })),
            Some("/root/261113/backpack_dashboard/backend/detect_jobs".to_string())
        );
        // 不是 plan_hit / 没给路径 / 布局不是 `<根>/<计划>/pt<n>/<图>`：一律不猜
        assert!(
            jobs_root_from_evidence(&json!({"type": "detect", "evidence": "/a/b/pt0/x.jpg"}))
                .is_none()
        );
        assert!(jobs_root_from_evidence(&json!({"type": "plan_hit"})).is_none());
        assert!(
            jobs_root_from_evidence(&json!({"type": "plan_hit", "evidence": "  "})).is_none()
        );
        assert!(
            jobs_root_from_evidence(&json!({"type": "plan_hit", "evidence": "/x/y.jpg"}))
                .is_none(),
            "目录不叫 pt<n> 就说明板端换了存证布局，反推出来的根不可信"
        );
    }

    /// 显式配的 `jobs_dir` 不能被板端事件覆盖（现场可能 bind-mount 到别处）
    #[tokio::test]
    async fn pinned_jobs_dir_wins_over_what_the_board_reports() {
        let ai = AiPatrol::with_jobs_dir("http://127.0.0.1:9", true, Some("/pinned/".to_string()));
        ai.learn_jobs_root("/from/board".to_string()).await;
        assert_eq!(ai.jobs_root().await.as_deref(), Some("/pinned"));

        // 没配时才学板端给的路径
        let ai = AiPatrol::with_jobs_dir("http://127.0.0.1:9", true, None);
        ai.learn_jobs_root("/from/board/detect_jobs".to_string()).await;
        assert_eq!(
            ai.jobs_root().await.as_deref(),
            Some("/from/board/detect_jobs")
        );
    }

    /// 存证目录里只认「本轮写下的」最新一张：上一轮巡检的图不能当这一帧的证据
    #[tokio::test]
    async fn newest_evidence_file_skips_stale_and_picks_freshest() {
        let dir = std::env::temp_dir().join(format!(
            "robodog-jobs-scan-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF, 0x00]).unwrap();
        std::fs::write(dir.join("notes.txt"), b"not an image").unwrap();

        // 窗口开始时间在未来（= 这些文件都是「上一轮的」）→ 不带图
        let future = SystemTime::now() + Duration::from_secs(60);
        assert!(newest_evidence_file(&dir, future).await.is_none());

        // 正常窗口：取到那张 jpg（非图片后缀直接忽略）
        let past = SystemTime::now() - Duration::from_secs(60);
        let picked = newest_evidence_file(&dir, past)
            .await
            .expect("应扫到命中帧");
        assert!(picked.ends_with("dogcam_1.jpg"), "{picked}");

        // 目录不存在（客户端不在板端 / 板端换了目录）：安静返回 None
        assert!(
            newest_evidence_file(&dir.join("nope"), past)
                .await
                .is_none()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ai_action_matches_ai_and_chinese_word_only() {
        assert!(is_ai_action(&json!({"type": "AI 识别"})));
        assert!(is_ai_action(&json!({"type": "ai detect"})));
        assert!(is_ai_action(&json!({"type": "智能识别"})));
        assert!(!is_ai_action(&json!({"type": "可见光拍照"})));
        assert!(!is_ai_action(&json!({"type": "红外测温"})));
    }

    #[test]
    fn empty_route_has_no_ai_points() {
        assert!(parse_ai_points(&json!({})).is_empty());
        assert!(parse_ai_points(&json!({"points": []})).is_empty());
    }

    #[test]
    fn start_parses_platform_payload() {
        let start = AiStart::from_value(&json!({
            "windowId": "w-1",
            "action": "config",
            "task": "firextinguisher",
            "taskCode": "TK-1112-11",
            "pointIndex": 0,
            "labels": ["extinguisher"],
            "conf": 0.35,
            "cam": "dogcam",
            "fps": 3,
            "secs": 30,
            "wasRunning": true
        }))
        .expect("payload parses");
        assert_eq!(start.window_id, "w-1");
        assert_eq!(start.action, "config");
        assert_eq!(start.task, "firextinguisher");
        assert_eq!(start.task_code, "TK-1112-11");
        assert_eq!(start.point_index, 0);
        assert_eq!(start.plan_id, "TK-1112-11", "没带 planId 时退回 taskCode");
        assert_eq!(start.expect_count, None);
        assert_eq!(start.labels, vec!["extinguisher".to_string()]);
        assert!((start.conf - 0.35).abs() < 1e-9);
        assert_eq!(start.secs, 30);
        assert!(start.was_running);
        assert!(AiStart::from_value(&json!({"action": "start"})).is_none());
    }

    #[test]
    fn hits_filter_by_conf_and_dedupe() {
        let mut seen = HashSet::new();
        let frame = json!({
            "type": "detect",
            "boxes": [
                {"label": "extinguisher", "conf": 0.92},
                {"label": "extinguisher", "conf": 0.91},
                {"label": "smoke", "conf": 0.20}
            ]
        });
        let first = take_hits(&frame, 0.35, &mut seen);
        assert_eq!(first, vec![("extinguisher".to_string(), 0.92)]);
        // 同一类别只报一次（窗口内 3~6fps 会重复出现）
        assert!(take_hits(&frame, 0.35, &mut seen).is_empty());
        // 低于阈值的不算
        assert!(take_hits(&json!({"type": "detect", "boxes": [{"label": "smoke", "conf": 0.30}]}),
            0.35, &mut seen).is_empty());
        // 非 detect 事件忽略
        assert!(take_hits(&json!({"type": "telemetry"}), 0.0, &mut seen).is_empty());
    }

    #[test]
    fn point_stages_keep_window_others_do_not() {
        // 到点阶段（触发开窗）必须包含在「还在航点上」（保持窗口）里
        for stage in ["patrol.point_arrived", "patrol.waiting"] {
            assert!(is_arrival_stage(stage));
            assert!(is_point_stage(stage), "{stage} 是在航点上");
        }
        // 点上的作业阶段：窗口继续开着（狗还站在那儿）
        for stage in ["patrol.photographing", "patrol.point_completed"] {
            assert!(!is_arrival_stage(stage));
            assert!(is_point_stage(stage), "{stage} 也是在本航点上");
        }
        for stage in [
            "patrol.navigating",
            "patrol.returning",
            "navigation.navigating",
            "",
        ] {
            assert!(!is_point_stage(stage), "{stage} 已经离开航点");
        }
    }

    #[test]
    fn sse_blocks_are_split_and_partial_stays() {
        let mut buffer = String::from("data: {\"a\":1}\n\ndata: {\"b\":2}\n\ndata: {\"part");
        assert_eq!(take_sse_events(&mut buffer), vec!["{\"a\":1}", "{\"b\":2}"]);
        assert_eq!(buffer, "data: {\"part");
        buffer.push_str("ial\":3}\n\n");
        assert_eq!(take_sse_events(&mut buffer), vec!["{\"partial\":3}"]);
    }
}

#[cfg(test)]
mod agent_flow {
    //! 起一个假板端 Agent（HTTP + SSE），把「客户端代调」整条链路跑一遍：
    //! `ai_point` 上报 → `ai_start` → 启停 Agent → 收 SSE → 按阈值去重回传 → 收尾。

    use super::*;
    use axum::Router;
    use axum::routing::{get, post};
    use futures_util::StreamExt as _;
    use tokio::net::TcpListener;

    const TASKS: &str = r#"{"count":1,"ready":1,"tasks":[{"task":"firextinguisher","name":"灭火器检测",
        "labels":[{"id":0,"name":"extinguisher","zh":"灭火器"}],"conf":0.25,"ok":true}]}"#;

    const STATUS: &str = r#"{"running":false,"state":"stopped","default_cam":"dogcam","cams":["dogcam"]}"#;

    /// 两帧同一类别（窗口内只报一次）、一帧低于阈值、一帧遥测（忽略），
    /// 最后一帧是板端「数量核对不一致」结论（应转成 ai_alert）
    const EVENTS: &str = concat!(
        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
        "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.92}]}\n\n",
        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
        "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.95}]}\n\n",
        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
        "\"boxes\":[{\"label\":\"smoke\",\"zh\":\"烟雾\",\"conf\":0.10}]}\n\n",
        "data: {\"type\":\"telemetry\",\"battery\":80}\n\n",
        "data: {\"type\":\"plan_hit\",\"plan_id\":\"TK-1\",\"point_index\":0,\"found\":true,",
        "\"found_count\":2,\"expect_count\":3,\"count_ok\":false,\"count_alert\":true,",
        "\"evidence\":\"/root/backpack_dashboard/backend/detect_jobs/TK-1/pt0/dogcam_1.jpg\"}\n\n",
    );

    /// 只有检出、没有 `plan_hit` 的事件流：模拟「命中帧还在采样、还没落盘」的那几秒
    const DETECT_ONLY: &str = concat!(
        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
        "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.92}]}\n\n",
    );

    /// 假 Agent 收到的调用：`(路径, body)`，body 用来验证计划参数确实透传了
    type Calls = Arc<Mutex<Vec<(String, Value)>>>;

    async fn spawn_agent(calls: Calls) -> String {
        spawn_agent_with(calls, EVENTS.to_string(), None, None).await
    }

    /// 可定制的假 Agent：
    ///
    /// - `events`：SSE 正文；
    /// - `delayed`：（多久之后, 又一段 SSE 正文）——用来模拟板端「命中帧落盘后才发
    ///   `plan_hit`」：现场实测比第一帧检出晚 4~5s；
    /// - `on_stop`：`/api/detect/stop` 时要写一个 JPEG 到哪里——模拟板端
    ///   `_plan_flush` 在停识别时把还在采样中的命中收尾存证。
    async fn spawn_agent_with(
        calls: Calls,
        events: String,
        delayed: Option<(Duration, String)>,
        on_stop: Option<std::path::PathBuf>,
    ) -> String {
        let app = Router::new()
            .route("/api/detect/tasks", get(|| async { TASKS }))
            .route("/api/detect/status", get(|| async { STATUS }))
            .route(
                "/api/detect/start",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>,
                     axum::Json(body): axum::Json<Value>| async move {
                        calls.lock().await.push(("start".to_string(), body));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/detect/config",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>,
                     axum::Json(body): axum::Json<Value>| async move {
                        calls.lock().await.push(("config".to_string(), body));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/detect/plan/clear",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>,
                     axum::Json(body): axum::Json<Value>| async move {
                        calls.lock().await.push(("plan/clear".to_string(), body));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/detect/stop",
                post(
                    move |axum::extract::State(calls): axum::extract::State<Calls>,
                          axum::Json(body): axum::Json<Value>| {
                        let on_stop = on_stop.clone();
                        async move {
                            calls.lock().await.push(("stop".to_string(), body));
                            // 板端停识别时会 `_plan_flush`：命中帧在这一刻才落盘
                            if let Some(path) = on_stop {
                                if let Some(parent) = path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let _ = std::fs::write(path, [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10]);
                            }
                            axum::Json(json!({"ok": true}))
                        }
                    },
                ),
            )
            .route(
                "/api/events",
                get(move || {
                    let events = events.clone();
                    let delayed = delayed.clone();
                    async move {
                        // 真机 SSE 是长连接：推完事件挂着不断开，收尾只能靠窗口到期/打断
                        let head = futures_util::stream::once(async move {
                            Ok::<_, std::io::Error>(events.into_bytes())
                        });
                        let tail = match delayed {
                            Some((after, body)) => futures_util::stream::once(async move {
                                tokio::time::sleep(after).await;
                                Ok::<_, std::io::Error>(body.into_bytes())
                            })
                            .boxed(),
                            None => futures_util::stream::empty().boxed(),
                        };
                        let stream = head.chain(tail).chain(futures_util::stream::pending());
                        (
                            [("content-type", "text/event-stream")],
                            axum::body::Body::from_stream(stream),
                        )
                    }
                }),
            )
            .with_state(calls);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定假 Agent 失败");
        let port = listener.local_addr().expect("取端口失败").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://127.0.0.1:{port}")
    }

    fn point() -> AiPoint {
        AiPoint {
            index: 0,
            name: "新航点 1".to_string(),
            dwell: 100,
            hint: "灭火器 · 灭火器".to_string(),
            scene: Some("firextinguisher".to_string()),
            labels: vec!["extinguisher".to_string()],
            threshold: "置信度 ≥ 0.35".to_string(),
            expect_count: Some(3),
            check_type: "count".to_string(),
            alert_when_missing: false,
        }
    }

    fn start_payload(secs: u64, was_running: bool) -> AiStart {
        AiStart::from_value(&json!({
            "windowId": "AIW-1-0-1",
            "action": if was_running { "config" } else { "start" },
            "task": "firextinguisher",
            "taskCode": "TK-1",
            "pointIndex": 0,
            "planId": "TK-1",
            "checkType": "count",
            "expectCount": 3,
            "labels": ["extinguisher"],
            "conf": 0.35,
            "cam": "dogcam",
            "fps": 3,
            "secs": secs,
            "wasRunning": was_running,
        }))
        .expect("构造开窗指令失败")
    }

    /// 存在性任务（`exist`，没有 `expectCount`）的开窗指令：
    /// 「存在就告警」这类航点同样要带证据图，与数量任务走同一条 `ai_detect`。
    fn exist_start_payload() -> AiStart {
        exist_start_payload_secs(1)
    }

    fn exist_start_payload_secs(secs: u64) -> AiStart {
        AiStart::from_value(&json!({
            "windowId": "AIW-1-0-2",
            "action": "start",
            "task": "firextinguisher",
            "taskCode": "TK-1",
            "pointIndex": 0,
            "planId": "TK-1",
            "checkType": "exist",
            "labels": ["extinguisher"],
            "conf": 0.35,
            "cam": "dogcam",
            "fps": 3,
            "secs": secs,
            "wasRunning": false,
        }))
        .expect("构造开窗指令失败")
    }

    /// 收消息直到收到 `ai_end`（或超时），返回期间收到的 (msgType, payload)
    async fn drain_until_end(
        rx: &mut mpsc::Receiver<WsMessage>,
        timeout: Duration,
    ) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                panic!("等 ai_end 超时，已收到: {out:?}");
            }
            match tokio::time::timeout(left, rx.recv()).await {
                Ok(Some(msg)) => {
                    let end = msg.msg_type == "ai_end";
                    out.push((msg.msg_type, msg.payload));
                    if end {
                        return out;
                    }
                }
                Ok(None) => return out,
                Err(_) => panic!("等 ai_end 超时，已收到: {out:?}"),
            }
        }
    }

    #[tokio::test]
    async fn point_arrived_reports_agent_catalog_to_platform() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(calls).await;
        let ai = Arc::new(AiPatrol::new(agent, true));
        let (tx, mut rx) = mpsc::channel(8);

        ai.point_arrived(&tx, "TK-1", &point()).await;
        let msg = rx.recv().await.expect("应上报 ai_point");
        assert_eq!(msg.msg_type, "ai_point");
        assert_eq!(msg.payload["taskCode"], "TK-1");
        assert_eq!(msg.payload["pointIndex"], 0);
        assert_eq!(msg.payload["scene"], "firextinguisher");
        assert_eq!(msg.payload["labels"], json!(["extinguisher"]));
        assert_eq!(msg.payload["threshold"], "置信度 ≥ 0.35");
        assert_eq!(msg.payload["expectCount"], 3, "检查目标数量要一起报给平台");
        assert_eq!(msg.payload["checkType"], "count", "任务类型要一起报给平台");
        assert_eq!(msg.payload["agentRunning"], false);
        assert_eq!(msg.payload["agentCam"], "dogcam");
        assert_eq!(
            msg.payload["tasks"]["tasks"][0]["task"], "firextinguisher",
            "必须把板端模型目录原样带上去，否则服务端算不出场景"
        );

        // 同一个点的后续帧（5009 每 500ms 一帧）不能反复上报
        ai.point_arrived(&tx, "TK-1", &point()).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(200), rx.recv())
                .await
                .is_err(),
            "同一个点只能请求一次"
        );
    }

    #[tokio::test]
    async fn window_starts_agent_streams_hits_and_stops_it() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(calls.clone()).await;
        let ai = Arc::new(AiPatrol::new(agent, true));
        let (tx, mut rx) = mpsc::channel(16);

        ai.start(&tx, start_payload(1, false)).await;
        let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;

        let detects: Vec<&Value> = messages
            .iter()
            .filter(|(kind, _)| kind == "ai_detect")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(detects.len(), 1, "同一类别只回传一次: {messages:?}");
        assert_eq!(detects[0]["label"], "extinguisher");
        assert_eq!(detects[0]["windowId"], "AIW-1-0-1");
        assert_eq!(detects[0]["planId"], "TK-1", "检出也要带计划 id");
        assert!((detects[0]["conf"].as_f64().unwrap() - 0.92).abs() < 1e-6);

        // 数量核对不一致：板端 plan_hit → 客户端转发 ai_alert（含计划/数量/证据）
        let alerts: Vec<&Value> = messages
            .iter()
            .filter(|(kind, _)| kind == "ai_alert")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(alerts.len(), 1, "数量不一致必须上报一条告警: {messages:?}");
        assert_eq!(alerts[0]["windowId"], "AIW-1-0-1");
        assert_eq!(alerts[0]["planId"], "TK-1");
        assert_eq!(alerts[0]["expectCount"], 3);
        assert_eq!(alerts[0]["foundCount"], 2);
        assert!(
            alerts[0]["evidence"].as_str().unwrap_or_default().ends_with(".jpg"),
            "{}:{}",
            alerts[0],
            "告警要带上板端证据图路径"
        );

        let end = messages
            .iter()
            .find(|(kind, _)| kind == "ai_end")
            .map(|(_, payload)| payload)
            .expect("窗口结束必须回报 ai_end");
        assert_eq!(end["hits"], 1);
        assert!(
            end["reason"].as_str().unwrap_or_default().contains("到期"),
            "{end}"
        );
        assert_eq!(end["expired"], true, "正常到期要标 expired=true: {end}");

        // 窗口结束后要把 Agent 停掉（否则 NPU 一直占着）
        let kinds: Vec<String> = calls
            .lock()
            .await
            .iter()
            .map(|(kind, _)| kind.clone())
            .collect();
        assert_eq!(kinds, ["start", "stop"]);

        // 计划参数（计划 id / 航点下标 / 检查目标数量）必须透传给板端
        let started = calls
            .lock()
            .await
            .iter()
            .find(|(kind, _)| kind == "start")
            .map(|(_, body)| body.clone())
            .expect("应调用 /api/detect/start");
        assert_eq!(started["plan_id"], "TK-1");
        assert_eq!(started["point_index"], 0);
        assert_eq!(started["expect_count"], 3);
        assert!(!ai.status_active().await, "窗口结束后不该还挂着窗口");
    }

    /// 检出告警要证据图：板端命中帧落在 `detect_jobs/<计划>/pt<航点>/`，
    /// 客户端自己扫目录取出最新一张，随 `ai_detect` 上报路径 + base64。
    ///
    /// 用**存在性任务**（`checkType=exist`）跑：现场「灭火器在不在」这类航点就是它，
    /// 与数量任务共用同一条 `ai_detect`，图不能只挂在数量告警上。
    #[tokio::test]
    async fn hit_alert_carries_the_board_evidence_frame() {
        let root = std::env::temp_dir().join(format!(
            "robodog-jobs-hit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let point_dir = root.join("TK-1").join("pt0");
        std::fs::create_dir_all(&point_dir).expect("建存证目录失败");
        let frame = point_dir.join("dogcam_1.jpg");
        let bytes = [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        std::fs::write(&frame, bytes).expect("写命中帧失败");

        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(calls).await;
        let ai = Arc::new(AiPatrol::with_jobs_dir(
            agent,
            true,
            Some(root.to_string_lossy().to_string()),
        ));
        let (tx, mut rx) = mpsc::channel(16);
        // 存在性任务（checkType=exist）也要图：现场配的灭火器巡检就是这一类
        ai.start(&tx, exist_start_payload()).await;
        let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;
        let _ = std::fs::remove_dir_all(&root);

        let detects: Vec<&Value> = messages
            .iter()
            .filter(|(kind, _)| kind == "ai_detect")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(detects.len(), 1, "{messages:?}");
        let evidence = detects[0]["evidence"].as_str().unwrap_or_default();
        assert!(
            evidence.ends_with("TK-1/pt0/dogcam_1.jpg"),
            "检出要带命中帧路径: {}",
            detects[0]
        );
        let encoded = detects[0]["evidenceImage"]
            .as_str()
            .expect("检出要带证据图字节");
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("证据图应是合法 base64");
        assert_eq!(decoded, bytes, "发出去的必须是命中帧原图字节");
    }

    /// 板端命中帧比 SSE `detect` 晚落盘（现场实测作业收尾时才写）：客户端要等它一会儿，
    /// 而不是「那一刻没文件就不带图」。
    #[tokio::test]
    async fn evidence_frame_that_lands_late_is_still_picked_up() {
        let root = std::env::temp_dir().join(format!(
            "robodog-jobs-late-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let point_dir = root.join("TK-1").join("pt0");
        std::fs::create_dir_all(&point_dir).expect("建存证目录失败");
        // 后台 600ms 后才写命中帧：早于/晚于第一个重试都要能等到
        let late = point_dir.join("dogcam_20261118_174010.jpg");
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            std::fs::write(&late, [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).expect("写命中帧失败");
        });

        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(calls).await;
        let ai = Arc::new(AiPatrol::with_jobs_dir(
            agent,
            true,
            Some(root.to_string_lossy().to_string()),
        ));
        let (tx, mut rx) = mpsc::channel(16);
        ai.start(&tx, exist_start_payload()).await;
        let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;
        let _ = std::fs::remove_dir_all(&root);

        let detect = messages
            .iter()
            .find(|(kind, _)| kind == "ai_detect")
            .map(|(_, payload)| payload)
            .expect("应上报检出");
        assert!(
            detect["evidenceImage"].as_str().is_some(),
            "晚落盘的命中帧也要带上: {detect}"
        );
    }

    /// 现场事故回归（2026-11-18 18:54，任务 TK-1112-11 航点 1「新航点 2」）：
    /// 板端 `_plan_frame` 要连采 3 帧命中才 `_plan_write` 落盘，现场那次命中帧比第一帧
    /// 检出晚 **4.6s**；客户端以前命中后只等 2.4s 就上报，群里那条「检出 Broken trash can」
    /// 只有卡片、没有图。现在命中先挂起、等 `plan_hit` 带 `evidence` 来（图在它之前已写好）
    /// 再上报，晚几秒也能带上图。
    #[tokio::test]
    async fn evidence_landing_seconds_late_still_rides_with_the_alert() {
        let root = std::env::temp_dir().join(format!(
            "robodog-jobs-plan-hit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let point_dir = root.join("TK-1").join("pt0");
        std::fs::create_dir_all(&point_dir).expect("建存证目录失败");
        let frame = point_dir.join("dogcam_20261118_185453.jpg");
        // 板端先写图（18:54:53.7）再发 `plan_hit`（带 evidence）；这里 4s 落盘、4.2s 才来事件
        let writer = frame.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(4)).await;
            std::fs::write(&writer, [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10]).expect("写命中帧失败");
        });
        let delayed = format!(
            concat!(
                "data: {{\"type\":\"plan_hit\",\"plan_id\":\"TK-1\",\"point_index\":0,",
                "\"found\":true,\"found_count\":1,\"count_alert\":false,",
                "\"evidence\":\"{}\"}}\n\n"
            ),
            frame.display()
        );

        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent_with(
            calls,
            DETECT_ONLY.to_string(),
            Some((Duration::from_secs(4), delayed)),
            None,
        )
        .await;
        let ai = Arc::new(AiPatrol::with_jobs_dir(
            agent,
            true,
            Some(root.to_string_lossy().to_string()),
        ));
        let (tx, mut rx) = mpsc::channel(16);
        let started = tokio::time::Instant::now();
        ai.start(&tx, exist_start_payload_secs(30)).await;

        let mut detects = Vec::new();
        while detects.is_empty() {
            let msg = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("等检出超时")
                .expect("通道关闭");
            if msg.msg_type == "ai_detect" {
                detects.push(msg.payload);
            }
        }
        let waited = started.elapsed();
        ai.stop("测试结束").await;
        let _ = std::fs::remove_dir_all(&root);

        assert!(
            waited >= Duration::from_secs(3),
            "要等到板端落盘才上报（旧实现 2.4s 就放弃带图了）: {waited:?}"
        );
        assert!(
            detects[0]["evidenceImage"].as_str().is_some(),
            "晚几秒落盘的命中帧也要跟着告警发出去: {}",
            detects[0]
        );
        assert!(
            detects[0]["evidence"]
                .as_str()
                .unwrap_or_default()
                .ends_with("pt0/dogcam_20261118_185453.jpg"),
            "证据路径要用板端 `plan_hit.evidence`: {}",
            detects[0]
        );
    }

    /// 窗口结束前板端还在采样（没等到 `plan_hit`）：停识别会让板端 `_plan_flush`
    /// 把采样中的命中画框存证，客户端要**先停再扫**，把图带上再发告警。
    #[tokio::test]
    async fn evidence_written_on_plan_flush_is_picked_up_at_window_end() {
        let root = std::env::temp_dir().join(format!(
            "robodog-jobs-flush-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let frame = root
            .join("TK-1")
            .join("pt0")
            .join("dogcam_20261118_185500.jpg");

        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        // 停识别时才写图（=板端 `_plan_flush`），此前目录里什么都没有
        let agent = spawn_agent_with(
            calls,
            DETECT_ONLY.to_string(),
            None,
            Some(frame.clone()),
        )
        .await;
        let ai = Arc::new(AiPatrol::with_jobs_dir(
            agent,
            true,
            Some(root.to_string_lossy().to_string()),
        ));
        let (tx, mut rx) = mpsc::channel(16);
        ai.start(&tx, exist_start_payload()).await;
        let messages = drain_until_end(&mut rx, Duration::from_secs(8)).await;
        let _ = std::fs::remove_dir_all(&root);

        let detects: Vec<&Value> = messages
            .iter()
            .filter(|(kind, _)| kind == "ai_detect")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(
            detects.len(),
            1,
            "挂起的检出不能丢（也不能重复发）: {messages:?}"
        );
        assert!(
            detects[0]["evidenceImage"].as_str().is_some(),
            "停识别后补扫到的命中帧要跟着告警发: {}",
            detects[0]
        );
        // 顺序：先停 Agent（板端才会 flush），再发检出，最后 ai_end
        let kinds: Vec<&str> = messages.iter().map(|(kind, _)| kind.as_str()).collect();
        let detect_at = kinds.iter().position(|kind| *kind == "ai_detect").unwrap();
        let end_at = kinds.iter().position(|kind| *kind == "ai_end").unwrap();
        assert!(detect_at < end_at, "检出要赶在 ai_end 之前上报: {kinds:?}");
    }

    /// 现场事故回归（2026-11-18 19:35，任务 TK-1112-11 航点 1「新航点 2」检出
    /// `Trash over flow` 0.29）：板端整个窗口只命中 **1 帧**，采样（连采 3 帧命中）
    /// 一直没跑完，`plan_hit` 要到窗口收尾 `_plan_flush` 才发。客户端 8s 兜底先把
    /// 卡片发了（没图），停识别扫到图后**单独补一条 `ai_evidence`**——平台只补一张
    /// 图，不重发卡片（群里不会出现两条告警）。
    #[tokio::test]
    async fn card_sent_without_image_is_followed_by_ai_evidence() {
        let root = std::env::temp_dir().join(format!(
            "robodog-jobs-follow-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        // 目录先建好，图要等停识别（板端 `_plan_flush`）时才写
        let point_dir = root.join("TK-1").join("pt0");
        std::fs::create_dir_all(&point_dir).expect("建存证目录失败");
        let frame = point_dir.join("dogcam_20261118_193557.jpg");

        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent_with(
            calls,
            DETECT_ONLY.to_string(),
            None,
            Some(frame.clone()),
        )
        .await;
        let ai = Arc::new(AiPatrol::with_jobs_dir(
            agent,
            true,
            Some(root.to_string_lossy().to_string()),
        ));
        let (tx, mut rx) = mpsc::channel(16);
        ai.start(&tx, exist_start_payload_secs(60)).await;

        // ① 等 8s 兜底：卡片先发出去，这时还没有图
        let detect = tokio::time::timeout(Duration::from_secs(12), async {
            loop {
                match rx.recv().await {
                    Some(msg) if msg.msg_type == "ai_detect" => return msg.payload,
                    Some(_) => continue,
                    None => panic!("通道关闭"),
                }
            }
        })
        .await
        .expect("等兜底上报告警超时");
        assert!(
            detect["evidenceImage"].as_str().is_none(),
            "兜底那条本来就没有图: {detect}"
        );

        // ② 窗口收尾：停识别 → 板端 flush 写出图 → 补 ai_evidence
        ai.stop("测试结束").await;
        let rest = drain_until_end(&mut rx, Duration::from_secs(8)).await;
        let _ = std::fs::remove_dir_all(&root);

        let evidence: Vec<&Value> = rest
            .iter()
            .filter(|(kind, _)| kind == "ai_evidence")
            .map(|(_, payload)| payload)
            .collect();
        assert_eq!(evidence.len(), 1, "补发一条图就够: {rest:?}");
        assert!(
            evidence[0]["evidenceImage"].as_str().is_some(),
            "补发必须带图字节: {}",
            evidence[0]
        );
        assert_eq!(evidence[0]["windowId"], "AIW-1-0-2");
        assert_eq!(evidence[0]["planId"], "TK-1");
        assert!(
            !rest.iter().any(|(kind, _)| kind == "ai_detect"),
            "补图不能重复发卡片: {rest:?}"
        );
    }

    #[tokio::test]
    async fn running_agent_is_hot_updated_and_left_alone() {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        let agent = spawn_agent(calls.clone()).await;
        let ai = Arc::new(AiPatrol::new(agent, true));
        let (tx, mut rx) = mpsc::channel(16);

        ai.start(&tx, start_payload(1, true)).await;
        let messages = drain_until_end(&mut rx, Duration::from_secs(5)).await;
        assert!(messages.iter().any(|(kind, _)| kind == "ai_detect"));

        // 操作员手动开着识别：只热更新，窗口结束不停人家的会话
        let kinds: Vec<String> = calls
            .lock()
            .await
            .iter()
            .map(|(kind, _)| kind.clone())
            .collect();
        assert_eq!(
            kinds,
            ["config", "plan/clear"],
            "识别留着时必须把计划摘掉，否则后续命中会继续写这个航点的存证"
        );
        let configured = calls
            .lock()
            .await
            .iter()
            .find(|(kind, _)| kind == "config")
            .map(|(_, body)| body.clone())
            .expect("应调用 /api/detect/config");
        assert_eq!(configured["plan_id"], "TK-1");
        assert_eq!(configured["expect_count"], 3);
    }

    #[tokio::test]
    async fn unreachable_agent_is_skipped_without_noise() {
        let ai = Arc::new(AiPatrol::new("http://127.0.0.1:9", true));
        let (tx, mut rx) = mpsc::channel(4);
        ai.point_arrived(&tx, "TK-1", &point()).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(300), rx.recv())
                .await
                .is_err(),
            "连不上 Agent 时不该往平台发消息（只在本地打 WARN）"
        );
    }

    /// 平台把证据图推进企业微信群 → 删掉板端这一轮的存证（画框 JPG + `result.json`）。
    ///
    /// 只删「这个计划的这个航点」：同一计划别的航点还在跑；整个计划目录空了才顺手删掉。
    #[tokio::test]
    async fn ai_cleanup_removes_the_point_dir_after_the_image_is_pushed() {
        let root = temp_jobs_dir("cleanup");
        let pt0 = root.join("TK-1").join("pt0");
        let pt1 = root.join("TK-1").join("pt1");
        for dir in [&pt0, &pt1] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF]).unwrap();
            std::fs::write(dir.join("result.json"), b"{}").unwrap();
        }
        let ai = AiPatrol::with_jobs_dir(
            "http://127.0.0.1:9",
            true,
            Some(root.to_string_lossy().to_string()),
        );

        ai.cleanup_evidence("TK-1", 0, "单测").await;
        assert!(!pt0.exists(), "命中的那个航点存证要删干净");
        assert!(pt1.exists(), "同一计划的其它航点不能动");
        assert!(root.join("TK-1").exists(), "计划目录还有 pt1，不能删");

        ai.cleanup_evidence("TK-1", 1, "单测").await;
        assert!(!pt1.exists());
        assert!(
            !root.join("TK-1").exists(),
            "计划目录空了就一起删，别在 detect_jobs 下留空壳"
        );
        assert!(root.exists(), "存证根本身不能动");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 窗口还开着时不能当场删：板端可能还在写图，客户端收尾还要扫目录补图（`ai_evidence`）。
    /// 先记账，收尾（[`AiPatrol::finish`]）后再删。
    #[tokio::test]
    async fn ai_cleanup_while_the_window_is_open_is_deferred() {
        let root = temp_jobs_dir("cleanup-defer");
        let pt0 = root.join("TK-1").join("pt0");
        std::fs::create_dir_all(&pt0).unwrap();
        std::fs::write(pt0.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF]).unwrap();
        let ai = AiPatrol::with_jobs_dir(
            "http://127.0.0.1:9",
            true,
            Some(root.to_string_lossy().to_string()),
        );
        *ai.window.lock().await = Some(Window {
            id: "AIW-1".to_string(),
            task_code: "TK-1".to_string(),
            plan_id: "TK-1".to_string(),
            point_index: 0,
            cancel: Arc::new(Notify::new()),
        });

        ai.cleanup_evidence("TK-1", 0, "单测").await;
        assert!(pt0.exists(), "窗口还开着就不能删（板端可能还在写图）");
        assert_eq!(
            ai.pending_cleanup.lock().await.clone(),
            vec![("TK-1".to_string(), 0)],
            "要记账，等收尾再删"
        );

        // 不是当前窗口的航点就可以当场删
        let pt2 = root.join("TK-1").join("pt2");
        std::fs::create_dir_all(&pt2).unwrap();
        ai.cleanup_evidence("TK-1", 2, "单测").await;
        assert!(!pt2.exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 删目录不可逆：平台下发的 `planId` 只认安全字符，路径穿越一律拒绝
    #[test]
    fn evidence_paths_refuse_unsafe_plan_ids() {
        let root = "/root/detect_jobs";
        assert_eq!(
            point_dir(root, "TK-1112-11", 3),
            Some(std::path::PathBuf::from("/root/detect_jobs/TK-1112-11/pt3"))
        );
        assert!(
            point_dir(root, "巡检计划-1", 0).is_some(),
            "汉字任务号要能用"
        );
        for bad in ["", "  ", "../etc", "a/b", "a\\b", "TK-1/../..", "TK:1"] {
            assert!(point_dir(root, bad, 0).is_none(), "{bad:?} 不能被拼进路径");
        }
        assert!(point_dir(root, "TK-1", -1).is_none(), "航点下标不能是负数");
    }

    /// 任务跑完：整个计划目录都删，包括**从没推过图**的航点（数量任务的检出不建单、
    /// 一个都没命中的航点只留 `result.json`）—— 它们永远等不到 `ai_cleanup`。
    #[tokio::test]
    async fn finish_plan_removes_the_whole_plan_dir() {
        let root = temp_jobs_dir("plan-end");
        let pt0 = root.join("TK-1").join("pt0");
        let pt1 = root.join("TK-1").join("pt1");
        let other = root.join("TK-2").join("pt0");
        for dir in [&pt0, &pt1, &other] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("result.json"), b"{}").unwrap();
        }
        std::fs::write(pt0.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF]).unwrap();
        let ai = Arc::new(AiPatrol::with_jobs_dir(
            "http://127.0.0.1:9",
            true,
            Some(root.to_string_lossy().to_string()),
        ));

        ai.finish_plan("TK-1", "单测").await;
        assert!(!root.join("TK-1").exists(), "整个计划目录要删干净");
        assert!(other.exists(), "别的计划不能动");
        assert!(root.exists(), "存证根本身不能动");
        assert!(ai.ending_plans.lock().await.is_empty(), "删完要把待清理标记摘掉");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 窗口还在收尾（停识别 → 板端 flush 落盘 → 扫盘补图）时不能当场删：
    /// 那几张图还要读出来推群。先记账，等窗口跑完再删。
    #[tokio::test]
    async fn finish_plan_while_the_window_is_open_defers_to_the_reaper() {
        let root = temp_jobs_dir("plan-defer");
        let pt0 = root.join("TK-1").join("pt0");
        std::fs::create_dir_all(&pt0).unwrap();
        std::fs::write(pt0.join("dogcam_1.jpg"), [0xFF_u8, 0xD8, 0xFF]).unwrap();
        let ai = Arc::new(AiPatrol::with_jobs_dir(
            "http://127.0.0.1:9",
            true,
            Some(root.to_string_lossy().to_string()),
        ));
        *ai.window.lock().await = Some(Window {
            id: "AIW-1".to_string(),
            task_code: "TK-1".to_string(),
            plan_id: "TK-1".to_string(),
            point_index: 0,
            cancel: Arc::new(Notify::new()),
        });

        ai.finish_plan("TK-1", "单测").await;
        assert!(pt0.exists(), "窗口还开着就不能删（图还没推出去）");
        assert_eq!(
            ai.ending_plans.lock().await.clone(),
            vec!["TK-1".to_string()],
            "要记账，等收尾再删"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 启动兜底：只清「早就该删」的残留，正在跑的 / 刚建的不能动
    #[tokio::test]
    async fn sweep_removes_only_stale_plan_dirs() {
        let root = temp_jobs_dir("sweep");
        let stale = root.join("TK-OLD").join("pt0");
        let fresh = root.join("TK-NEW").join("pt0");
        for dir in [&stale, &fresh] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("result.json"), b"{}").unwrap();
        }
        let ai = AiPatrol::with_jobs_dir(
            "http://127.0.0.1:9",
            true,
            Some(root.to_string_lossy().to_string()),
        );

        assert_eq!(ai.sweep_plans_older_than(Duration::from_secs(3600)).await, 0);
        assert!(stale.exists() && fresh.exists(), "刚写下的存证不能被当残留");

        assert_eq!(ai.sweep_plans_older_than(Duration::ZERO).await, 2);
        assert!(!stale.exists() && !fresh.exists());
        assert!(root.exists(), "存证根本身不能动");

        let _ = std::fs::remove_dir_all(&root);
    }

    fn temp_jobs_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "robodog-jobs-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
