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

use crate::wire::{WsMessage, send};
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
/// 到点后最多等这么久「真正停稳」（5009 `patrol.waiting`）。
///
/// 5009 的 `patrol.point_arrived` 只是「位置到了」：本体随后还要原地转到路线里配的
/// `heading` 才进 `patrol.waiting`（= 真正开始停留）。现场实测这段恒定 **2~3 s**
/// （2026-11-25 15:31 广汇二期 `gh-test2` / `TK-1125-19`：15:31:40.39 到点 →
/// 15:31:43.36 才 `waiting`，当天每个航点都一样），那几秒画面在转：推理出来基本是
/// 无效帧，还会误检（数量任务更麻烦：转动扫过的瞬时命中会被算进结论）。
///
/// 所以窗口分两步（见 `run_window`）：
///
/// 1. 到点就来——板端先**预热**（RKNN 模型 / 抓帧 / 画框 OSD 都在这 3s 里起来，
///    不占额外时间），但不登记航点计划；
/// 2. **停稳后才开始算**：此时才把计划（`plan_id` / 数量核对 / 存证）热更新给板端、
///    才开始读 SSE —— 转身那几秒的帧不进结论。
///
/// 老本体 / 某些场景不报 `waiting` 时由这个 grace 兜底（退回“到点就开始算”），
/// 另有一条更早的放行：本体阶段越过到点（`photographing` / `point_completed`）。
const WAITING_GRACE: Duration = Duration::from_secs(5);

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
    /// 动作 `notifyWhenCountOk`：`count` 任务数量**一致**时也通知企业微信
    ///
    /// 只影响「要不要把板端的 `plan_hit.count_ok=true` 转成 `ai_count`」；
    /// 平台收到后**只推群消息，不落告警、不建工单**（见 `server/src/notify.rs`）。
    pub notify_when_count_ok: bool,
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
            notify_when_count_ok: action
                .get("notifyWhenCountOk")
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
    /// 数量**一致**时是否也通知企业微信（动作 `notifyWhenCountOk`，平台据此判）
    pub notify_when_count_ok: bool,
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
    /// 航点自动采集总开关（中台 `media_capture_enabled`，默认关）：
    /// `Some(false)` 时板端只写告警证据图，不往媒体归档目录攒训练图；
    /// 老平台没这个字段（`None`）时按板端 / 老行为走。
    pub media_capture: Option<bool>,
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
            notify_when_count_ok: value
                .get("notifyWhenCountOk")
                .and_then(Value::as_bool)
                .unwrap_or(false),
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
            media_capture: value.get("captureEnabled").and_then(Value::as_bool),
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
pub(crate) struct Leftover {
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

/// 「本体在这个点上已经停稳了」（5009 `patrol.waiting` / 过了到点阶段）。
///
/// 窗口在**到点**就开了（板端提前预热，见 [`WAITING_GRACE`]），但**计数/上报要等停稳**：
/// 存成（任务, 航点）是为了只认当前这个点（上一个点的标记不能用）。
type SettledPoint = (String, i64);

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
    /// 本体已经在这个点停稳过（`patrol.waiting` / 阶段越过到点）：
    ///
    /// 窗口在到点就开了（板端提前预热），但计数/上报要等停稳 —— 见
    /// [`WAITING_GRACE`] 与 [`AiPatrol::wait_settled`]。
    settled: Mutex<Option<SettledPoint>>,
    /// 等停稳的兜底时长（生产是 [`WAITING_GRACE`]，测试里调小）
    settle_grace: Duration,
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
            settled: Mutex::new(None),
            // 单测里「等停稳」不拖时间（每窗 5s 会把测试拖成分钟级）：要验这个行为的
            // 用例自己 `set_settle_grace`；生产走 `WAITING_GRACE`（实测转身 2~3s，留 5s 余量）。
            settle_grace: if cfg!(test) {
                Duration::ZERO
            } else {
                WAITING_GRACE
            },
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

    /// 调小「等停稳」的兜底时长（只给测试：真机上是 [`WAITING_GRACE`]）
    #[cfg(test)]
    pub(crate) fn set_settle_grace(&mut self, grace: Duration) {
        self.settle_grace = grace;
    }

    /// 5009 报「本体已经在这个点上停稳」（`patrol.waiting`，或阶段已越过到点）。
    ///
    /// 窗口在到点就开了（板端借此提前预热模型/抓帧/画框），但**计数与上报要等这里**：
    /// `patrol.point_arrived` 之后本体还要原地转到路线里配的 `heading`，实测 2~3s，
    /// 那几秒画面在转 —— 转身期的帧既没信息量又容易误检（见 [`WAITING_GRACE`]）。
    ///
    /// 只记「哪个点」：`run_window` 拿它对上自己这一窗才放行，上一个点的标记不会串。
    pub async fn mark_settled(&self, task_code: &str, point_index: i64) {
        if !self.enabled {
            return;
        }
        let mut guard = self.settled.lock().await;
        // `patrol.waiting` 在整个停留期间持续上报（500ms 一帧）：重复写同一个值没有意义
        if guard
            .as_ref()
            .is_some_and(|(code, index)| code == task_code && *index == point_index)
        {
            return;
        }
        debug!("航点 AI 识别：任务 {task_code} 航点 {point_index} 已停稳，开始计数/上报");
        *guard = Some((task_code.to_string(), point_index));
    }

    /// 等本体停稳（最多 `settle_grace`）。返回是否真的等到了。
    ///
    /// 100ms 一次查标记，不用 `Notify`：标记可能在窗口建起来**之前**就写下了
    /// （`waiting` 事件比平台往返快的时候），查标记天然没有丢唤醒的问题。
    async fn wait_settled(&self, task_code: &str, point_index: i64) -> bool {
        let deadline = Instant::now() + self.settle_grace;
        loop {
            if self
                .settled
                .lock()
                .await
                .as_ref()
                .is_some_and(|(code, index)| code == task_code && *index == point_index)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
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
                "notifyWhenCountOk": point.notify_when_count_ok,
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
        // 停稳标记只是「计数器放行」用的，窗口关了就没意义（下个点会重新写）
        self.settled.lock().await.take();
        let taken = self.window.lock().await.take();
        if let Some(window) = taken {
            info!("航点 AI 识别窗口关闭（{reason}）：窗口 {}", window.id);
            window.cancel.notify_one();
        }
        *self.requested.lock().await = None;
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

mod agent;
mod evidence;
mod window;

// 子模块里的实现与自由函数：窗口 / 存证 / Agent 调用各自成文件。
// 这些再导出只给 `tests` / `agent_flow` 用（`use super::*`）
#[allow(unused_imports)]
pub(crate) use agent::*;
#[allow(unused_imports)]
pub(crate) use evidence::*;
#[allow(unused_imports)]
pub(crate) use window::*;

#[cfg(test)]
mod tests;

/// 端到端：起一个假板端 Agent（HTTP + SSE），把「客户端代调」整条链路跑一遍。
#[cfg(test)]
mod agent_flow;
