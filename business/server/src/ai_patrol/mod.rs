//! 航点到点 AI 识别：巡检走到带「AI 识别」动作的航点时，自动打开背包 Agent 的
//! 实时识别，检出的目标写进平台告警中心。
//!
//! 为什么放在服务端而不是客户端：
//!
//! - 背包 Agent 的 AI 是**推流旁路抽帧**（RTSP → RKNN），事件走 SSE（`/api/events`），
//!   服务端已经在代理这条流（[`crate::backpack::events`]），复用现成通道最省事；
//! - 告警落库、去重、与任务/园区/设备的关联都在服务端有现成上下文；
//! - 客户端只报「到点了、第几个点」（`task_progress` 的 `stage` + `pointIndex`），
//!   不引入任何客户端侧状态机，任务执行与识别解耦：AI 挂了巡检照样跑完。
//!
//! 触发条件：`task_progress` 的 `stage` 命中 `patrol.point_arrived` / `patrol.waiting`，
//! 且该航点的 `actions[]` 里有类型含 `AI` / `识别` 的动作。
//!
//! 「哪个航点」只认 `waypointIndex`（0 基下标）：`pointIndex` 在协议里是**已完成航点数**
//! （1 基，5s 定时摘要就靠它显示「进度 n/N」），拿它查路线会查到下一个点，
//! 表现为「AI 窗口刚开 3~5s 就被『到达无 AI 动作的航点』关掉、每 5s 开关一次」。
//! 只有老客户端（不发 `waypointIndex`）才退回 `pointIndex`。
//!
//! ⚠️ 这个下标是**内部**用的：查路线 `points[]`、拼去重键 `ai_dedup_key`、打日志。
//! 告警正文与任务时间线里**不写它**，只写航点名（`航点「新航点 5」`）——页面上航点
//! 按 1 基编号（P1、P2…），写成 `航点 3` 会让人以为凭空多了一个点（2026-11-19
//! 「华郅9层巡检」那条 `航点 3「新航点 5」` 就是 P4，名字里的 5 是删点后没重编号的残留）。
//!
//! 动作怎么配：路线编辑器「执行参数」直接取**「实时视频 → AI 识别」的模型目录**，
//! 写成动作的 `scene`（模型包 id）+ `labels`（勾选的类别 `name`）+ `threshold`（置信度）；
//! 没有这两个字段的历史数据仍按 `param` 里的中文关键词猜场景（兼容老路线）。
//!
//! ### 计划 id 与「检查目标数量」
//!
//! 开窗时代码把三个跟航点绑定的参数一起传给板端：`planId`（= 业务任务号
//! `taskCode`，板端拿它建存证目录 `detect_jobs/<planId>/pt<航点下标>/`）、
//! `pointIndex`、`checkType` 和 `expectCount`。`checkType` 区分两类任务：
//!
//! - `exist`（检测是否存在）：找到目标就存证/上报，数量不参与判定；
//! - `count`（检测数量是否正确）：除了找到，还要把 AI 检出的目标数与
//!   `expectCount` 比对，不一致（含「一个都没找到」= 0 ≠ N）就上报数量告警。
//!   ⚠️ `count` 任务的**检出只进任务时间线**（不落告警、不建工单、不推企业微信）：
//!   数量任务只有一个结论要处置，检出是它的中间过程。见 [`record_hit`]。
//!   核对**一致**默认也不出声；动作勾了 `notifyWhenCountOk` 时只推一条企业微信群回执
//!   （`ai_count` → [`record_count_ok`]，**不落告警、不建工单**）。
//!
//! 不一致时板端上报 `ai_alert`（客户端代调）或在 SSE 里带 `count_alert`（服务端直连），
//! 两条路都落到同一个 `record_count_alert`：告警中心 + 任务时间线 + 企业微信。
//! 「未命中」的判据是**狗在这个点上看够了没有**，而不是「窗口定时器有没有走到头」：
//! 客户端 `ai_end` 的 `expired=true`，或者窗口从开出来到现在已经 ≥ `dwell` 秒（狗按计划
//! 停满整个停留时长后离开，平台随之「已离开航点」关窗），都算正常结束。
//! 只看 `expired` 是不行的：窗口时长是 `dwell + 8`，而狗停到 `dwell` 就走了，
//! 于是窗口总是先被「已离开航点」摘掉、`expired` 永远是 false——「没找到也告警」
//! 会**永远报不出来**（2026-11-18 办公室巡检「垃圾桶溢出」漏报，pt1 停 65 s、窗口 68 s）。
//! 被任务结束 / 新航点顶替且**没停满** `dwell` 时才不记账（狗还没看够）。
//!
//! 但「看够了」不等于「现在就能下结论」：客户端模式的结论比「已离开航点」晚一步
//! （`ai_stop` → 板端 `_plan_flush` 落盘 → 扫盘/读图 → `ai_detect` / `ai_alert` /
//! `ai_end`）。所以关窗不再当场结算，而是把窗口标成 `closing` 留在 `active` 里，
//! 等客户端 `ai_end` 或最多 [`CLOSE_GRACE`] 秒（见 [`AiPatrol::close_client`]）。
//! 2026-11-20 13:38:03（TK-1112-11 PT2）就是没等：平台写了「检查 2 个，实际检出 0 个」，
//! 板端 `result.json` 却是 `found_count: 1`，客户端 0.6 s 后到的 `ai_alert` + 证据图
//! 还被当成失效窗口丢掉。
//!
//! 窗口时长 = `dwell.max(4) + 8` 秒，下限 10 s（Agent 加载模型要 1~3 s，太短会
//! 「还没开始识别狗就走了」），上限 `[backpack] ai_window_max_secs`（默认 300 s，与
//! 路线停留上限一致）——**停多久就识别多久**，不再把长停留截断成 30 s。
//!
//! 防「离开航点后继续把别的画面当目标」不靠截断时长，而靠阶段收口：`stage` 一旦不再是
//! `patrol.point_*` / `patrol.waiting`（即狗开始走向下一个点），窗口立刻关闭。
//!
//! 两种调用方（`[backpack] point_ai` / `ROBODOG_POINT_AI`，默认 `auto`）：
//!
//! - `server`：本模块直连 Agent（要求服务端能 HTTP 到板端，即配了 `ROBODOG_BACKPACK_URL`）；
//! - `client`：**设备客户端代调**。正式环境的业务平台在公网、Agent 只在现场内网，
//!   服务端直连必然超时（现场表现就是「配了 AI 识别从来没告警」）。客户端与 Agent
//!   同机（`127.0.0.1:8000`），到点把「动作参数 + 板端模型目录」上报（`ai_point`），
//!   本模块用**同一套** `resolve_scene` / `scene_labels` / `parse_conf` / `window_secs`
//!   算好参数下发 `ai_start`，客户端启停 Agent 并把检出回传（`ai_detect` / `ai_end`）。
//!   决策逻辑只有这一份，两边不会各说一套。
//! - `auto`：配了 `backpack_base_url` 走 `server`，否则走 `client`。
//!
//! 已知边界（写在这里以免下次重新踩）：
//!
//! - Agent 的 AI 是**单路单任务**（`single_cam`），所以窗口只能有一份
//!   （状态挂在 [`crate::app::AppState`] 上，生产环境就是一个实例）；
//!   操作员在「实时视频」页手动开的识别会被巡检窗口视为「已在运行」，
//!   我们只做热更新（`/api/detect/config`），窗口结束后**不会**把操作员的识别停掉；
//! - Agent 目前没有「传一张图返回结果」的接口，所以只能这样开窗口，做不到「按张推理」；
//! - 这条链路是**服务端**调 Agent，因此要求服务端能 HTTP 到板端（配 `ROBODOG_BACKPACK_URL`）。
//!   只配 `ROBODOG_BACKPACK_BROWSER_URL`（浏览器直连）时，服务端连不到 Agent，
//!   窗口会立刻收掉并打 WARN——现场「配了 AI 识别却没告警」先查这一点。

use crate::app::AppState;
use crate::backpack;
use crate::db::now;
use crate::gateway_types::{
    AiAlertPayload, AiCountPayload, AiDetectPayload, AiEndPayload, AiEvidencePayload,
    AiPointPayload, TaskProgressPayload,
};
use crate::notify::NoticeImage;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

/// 到点后识别窗口的基础时长（秒）：`dwell.max(4) + 8`，再夹到 `[10, 30]`。
pub(crate) const WINDOW_MIN_SECS: u64 = 10;
/// 窗口时长默认上限（秒）：与路线「停留时长」的单点上限一致（300 s = 150 个 2 s 等待）。
/// 要收紧用 `[backpack] ai_window_max_secs` / `ROBODOG_AI_WINDOW_MAX_SECS`。
pub const DEFAULT_WINDOW_MAX_SECS: u64 = 300;
/// 推理帧率：巡检到点停留才几秒，3 fps 足够抓目标，又不会占满板端算力。
pub(crate) const DETECT_FPS: u32 = 3;
/// 单条 SSE 缓冲上限：Agent 事件每条几十字节，超过说明流里没有 `\n\n`，直接丢弃重来。
pub(crate) const SSE_BUF_LIMIT: usize = 64 * 1024;
/// 客户端代调模式的窗口序号：同一个航点可能被开多次（长停留 / 重试），
/// 必须让每次的 `windowId` 唯一，客户端才能靠它判断「我是不是已经被顶替」。
pub(crate) static WINDOW_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// 已关窗口的留痕保留时长 / 条数：给迟到的 `ai_evidence` 找「该归到哪条告警」用。
pub(crate) const RECENT_WINDOW_TTL: Duration = Duration::from_secs(30 * 60);
pub(crate) const RECENT_WINDOW_KEEP: usize = 32;
/// 客户端模式关窗后的宽限期（秒）：见 [`AiPatrol::close_client`]。
///
/// 平台收到「已离开航点」时**不能立刻**判「一个都没找到」：客户端的结论天然比它晚
/// ——停识别 → 板端 `_plan_flush` 把采样中的命中落盘 → 扫盘/读图 → `ai_detect` /
/// `ai_alert` → `ai_end`。2026-11-20 13:38:03 现场（TK-1112-11 PT2）就是在这条缝里丢的：
/// 平台 03.908 写下「检查 2 个，实际检出 0 个」，板端 `result.json` 其实是
/// `found_count: 1`，客户端 04.506 才把 `ai_alert` + 证据图送上来，被当成失效窗口丢掉。
/// 所以先下发 `ai_stop`、把窗口留在 `active` 里等客户端说完（最多这么久），到点还没
/// 回话才按已上报的检出结算。
pub(crate) const CLOSE_GRACE: Duration = Duration::from_secs(12);
mod agent;
mod client;
mod parse;
mod repo;
#[cfg(test)]
mod tests;
mod window;

// crate 内共享：窗口生命周期、纯函数与数据读取在几个文件之间互相调用。
#[allow(clippy::wildcard_imports)]
pub(crate) use agent::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use parse::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use repo::*;
#[allow(clippy::wildcard_imports)]
pub(crate) use window::*;

/// 已经在跑的一个识别窗口
pub(crate) struct Window {
    /// 客户端代调模式下的窗口 ID（服务端直连模式为空串）
    window_id: String,
    device_code: String,
    task_code: String,
    point_index: i64,
    point_name: String,
    scene: String,
    /// 实际下发的识别类别（空 = 该场景全部类别），排查「勾了类别还是报了别的目标」用
    labels: Vec<String>,
    threshold: f64,
    cam: String,
    started_at: String,
    /// 窗口开出来的时刻：判「狗在这个点上看够了没有」用（见 [`settle_no_hit`]）
    started: Instant,
    /// 停止信号：`notify_one` 会留 permit，窗口任务即使在 `chunk().await` 里也能立刻醒
    stop: Arc<Notify>,
    /// 本窗口已上报的类别（同一类别只报一次，避免 3 fps 刷屏）
    hits: Arc<Mutex<Vec<Hit>>>,
    /// 本窗口是否已经报过「数量不一致」：告警只落一条（同一个窗口可能收到多帧结论）
    alerted: Arc<AtomicBool>,
    /// 告警 / 任务事件需要的上下文（任务名、园区、航点名等）
    cfg: WindowConfig,
    /// 已经下发 `ai_stop`、正等客户端把本窗口的结论说完（见 [`CLOSE_GRACE`]）。
    ///
    /// `Some(原因)` = 窗口已经「下班」（不再对外开窗、也不会再开新的一轮），但还留在
    /// [`AiPatrol::active`] 里十几秒：客户端收尾时回来的 `ai_detect` / `ai_alert` /
    /// `ai_evidence` / `ai_end` 必须还能归到它身上，否则「实际检出 0 个」这种假结论
    /// 和证据图一起丢（2026-11-20 13:38 现场）。
    closing: Option<String>,
}

impl Window {
    /// 这个窗口从开出来到现在过了多久（判「看够了没有」用，见 [`watched_enough`]）
    fn watched(&self) -> Duration {
        self.started.elapsed()
    }
}

/// 已经关掉的窗口留下的薄薄一层痕迹：`windowId → 本窗口的检出/告警`。
///
/// 为什么需要它：平台收到「已离开航点」就摘窗口并下发 `ai_stop`，客户端**正是**
/// 收到这条 `ai_stop` 才开始收尾（停识别 → `_plan_flush` 落盘 → 扫盘 → `ai_evidence`）。
/// 所以证据图永远比窗口活得久，只认 `active` 窗口会把每张晚到的图都丢掉
/// （2026-11-18 21:34 pt1「Trash over flow」卡片发了、图没发）。
pub(crate) struct ClosedWindow {
    window_id: String,
    device_code: String,
    closed_at: Instant,
    hits: Vec<Hit>,
}

/// 航点 AI 到底谁去调 Agent。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointAiSource {
    Client,
    Server,
}

/// 按配置解析调用方：`auto` 看有没有配服务端可达的 Agent 地址。
pub fn source_for(config: &crate::config::Config) -> PointAiSource {
    resolve_source(&config.point_ai, config.backpack_base_url.is_some())
}

/// [`source_for`] 的纯函数版本（配置解析 + 单测都走这里，规则只写一份）
fn resolve_source(point_ai: &str, has_base_url: bool) -> PointAiSource {
    match point_ai {
        "client" => PointAiSource::Client,
        "server" => PointAiSource::Server,
        // `auto`（含非法值兜底）：服务端能直连板端就自己干，否则交给设备客户端
        _ => {
            if has_base_url {
                PointAiSource::Server
            } else {
                PointAiSource::Client
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Hit {
    at: String,
    label: String,
    conf: f64,
    /// 这条检出对应的告警码：卡片已推、图晚到时靠它补发（`ai_evidence`）
    alert_code: Option<String>,
    /// 告警推的时候是否已经带上了证据图：带过就不用再补发
    image: bool,
}

/// 窗口的两个上报累加器：类别去重表 + 「数量告警已报过」标志。
///
/// 打包成一个参数传，是因为 `consume_events` 的参数个数已到 clippy 的上限（8/7）。
pub(crate) struct WindowSink {
    hits: Arc<Mutex<Vec<Hit>>>,
    alerted: Arc<AtomicBool>,
}

pub struct AiPatrol {
    active: Mutex<Option<Window>>,
    /// 刚关掉的窗口（最近的几条）：迟到的 `ai_evidence` 靠它找告警
    recent: Mutex<Vec<ClosedWindow>>,
}

impl Default for AiPatrol {
    fn default() -> Self {
        Self::new()
    }
}
impl AiPatrol {
    /// 新建一份识别窗口状态。
    ///
    /// **一份实例 = 一路推理**（板端 Agent 只有一个推理进程，所以窗口也只能有一个）。
    /// 它挂在 [`crate::app::AppState`] 上，不再是进程级 `static`：
    /// 以前那份 `LazyLock<AiPatrol>` 让所有集成测试共用同一个窗口，
    /// 只能靠 `TEST_LOCK` 串行 + 「用独立任务号别串台」的注释兜着；
    /// 现在每个 `test_state()` 都是干净的实例，想跑多实例也不会打架。
    pub fn new() -> Self {
        Self {
            active: Mutex::new(None),
            recent: Mutex::new(Vec::new()),
        }
    }

    /// 停掉当前窗口（清状态、通知窗口任务退出），**不**做「没找到」的记账。
    ///
    /// 任务终态 / 已离开航点这类正常收口走 [`Self::close`]（它会在狗看够了时记账）；
    /// 这里是给测试复位、以及「结论不可信」的场合用的（设备断线时谁也不知道板端看到了什么）。
    pub async fn stop(&self, reason: &str) {
        let mut guard = self.active.lock().await;
        if let Some(window) = guard.take() {
            info!(
                "航点 AI 识别窗口关闭（{reason}）：任务 {} 航点 {}「{}」",
                window.task_code, window.point_index, window.point_name
            );
            window.stop.notify_one();
        }
    }

    /// 给 `GET /api/backpack/ai-window` 用的只读快照。
    pub async fn status(&self) -> Value {
        let guard = self.active.lock().await;
        match guard.as_ref() {
            None => json!({ "active": false }),
            Some(window) => {
                let hits = window.hits.lock().await.clone();
                json!({
                    "active": true,
                    "windowId": if window.window_id.is_empty() {
                        Value::Null
                    } else {
                        json!(window.window_id)
                    },
                    "source": if window.window_id.is_empty() { "server" } else { "client" },
                    "deviceCode": window.device_code,
                    "taskCode": window.task_code,
                    "pointIndex": window.point_index,
                    "pointName": window.point_name,
                    "scene": window.scene,
                    "labels": window.labels,
                    "threshold": window.threshold,
                    "cam": window.cam,
                    "startedAt": window.started_at,
                    "windowSecs": window.cfg.secs,
                    // 收尾宽限期里窗口还「在」，但已经不再接受新的一轮（见 `CLOSE_GRACE`）
                    "closing": window.closing,
                    "hits": hits.iter().map(|h| json!({
                        "at": h.at, "label": h.label, "conf": h.conf
                    })).collect::<Vec<_>>(),
                })
            }
        }
    }

    /// 自己是不是被**新窗口顶替**了。
    ///
    /// 收尾时用它决定要不要去停 Agent：
    /// - 被顶替（新窗口已接管，且可能继承着操作员手动开的识别）→ **不能停**；
    /// - 自己到期 / 被 `stop()` 摘掉（任务终态）→ 要停，这里的 `false` 包含这种情况，
    ///   所以不能用「还是当前窗口」作为判据（那时记录已经被摘掉了）。
    async fn replaced_by_other(&self, stop: &Arc<Notify>) -> bool {
        self.active
            .lock()
            .await
            .as_ref()
            .map(|window| !Arc::ptr_eq(&window.stop, stop))
            .unwrap_or(false)
    }

    /// 处理一条任务进度上报：到点且有 AI 动作 → 开窗口；任务终态 → 关窗口。
    pub async fn on_progress(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &TaskProgressPayload,
    ) {
        if payload.task_code.is_empty() {
            return;
        }
        if is_terminal_status(&payload.status) {
            self.close(state, device_code, "任务终态").await;
            return;
        }
        let stage = payload.stage.as_deref().unwrap_or_default();
        if !is_point_stage(stage) {
            // 已经离开航点（前往下一点 / 返航 / 别的任务阶段）：窗口没有继续的意义
            self.close(state, device_code, "已离开航点").await;
            return;
        }
        if !is_arrival_stage(stage) {
            return;
        }
        // 航点下标只认 `waypointIndex`（0 基，= 5009 的 `detail_index`）。
        //
        // 不能用 `point_index`：那是「已完成航点数」，客户端的 5s 定时摘要里它等于
        // 「当前点 + 1」，拿它查路线会查到**下一个**航点——2026-11-17 现场
        // TK-1112-11 就是这么每 5s 把刚开的窗口当成「到达无 AI 动作的航点」关掉的
        // （AI 航点后面跟一个拍照点时必现：窗口活不过 5s，Agent 反复起停、零检出）。
        // 老客户端不发 `waypointIndex`，只能退回 `point_index`（差一位的老行为）。
        let Some(point_index) = payload.waypoint_index.or(payload.point_index) else {
            return;
        };

        // 同一个点在多个阶段（point_arrived / waiting）都会上报，窗口已在这个点就跳过
        {
            let guard = self.active.lock().await;
            if let Some(window) = guard.as_ref()
                && window.task_code == payload.task_code
                && window.point_index == point_index
            {
                return;
            }
        }

        let task = match load_task(&state.db, &payload.task_code).await {
            Ok(Some(task)) => task,
            Ok(None) => return,
            Err(error) => {
                warn!("AI 识别：查任务 {} 失败: {error}", payload.task_code);
                return;
            }
        };
        let Some(route_id) = task.route_id else {
            return;
        };
        let point = match load_point(&state.db, route_id, point_index).await {
            Ok(Some(point)) => point,
            Ok(None) => return,
            Err(error) => {
                warn!("AI 识别：读路线 {route_id} 航点 {point_index} 失败: {error}");
                return;
            }
        };
        let Some(action) = point.find_ai_action() else {
            // 这个点没有 AI 动作：如果上一个点的窗口还开着（例如上一轮没有 AI 的点），关掉
            self.close(state, device_code, "到达无 AI 动作的航点").await;
            return;
        };
        if source_for(&state.config) == PointAiSource::Client {
            // 客户端代调：由它到点探测板端目录后上报 `ai_point`，服务端不直连 Agent
            return;
        }

        let threshold = parse_conf(&action.threshold).unwrap_or(0.0);
        let window_secs = window_secs(point.dwell, state.config.ai_window_max_secs);
        let stop = Arc::new(Notify::new());
        let hits = Arc::new(Mutex::new(Vec::new()));
        let alerted = Arc::new(AtomicBool::new(false));
        let config = WindowConfig {
            device_code: device_code.to_string(),
            device_id: task.device_id,
            park_id: task.park_id,
            task_code: payload.task_code.clone(),
            task_name: task.name.clone(),
            point_index,
            point_name: point.name.clone(),
            hint: action.hint.clone(),
            scene: action.scene.clone(),
            labels: action.labels.clone(),
            threshold_text: action.threshold.clone(),
            expect_count: action.expect_count,
            check_type: action.check_type.clone(),
            alert_when_missing: action.alert_when_missing,
            notify_when_count_ok: action.notify_when_count_ok,
            secs: window_secs,
            dwell: point.dwell.max(0) as u64,
        };

        // 抢占：Agent 单路，先关旧窗口再开新窗口
        {
            let mut guard = self.active.lock().await;
            if let Some(previous) = guard.take() {
                previous.stop.notify_one();
            }
            *guard = Some(Window {
                window_id: String::new(),
                device_code: device_code.to_string(),
                task_code: payload.task_code.clone(),
                point_index,
                point_name: point.name.clone(),
                scene: action.hint.clone(),
                labels: Vec::new(),
                threshold,
                cam: String::new(),
                started_at: now(),
                started: Instant::now(),
                stop: stop.clone(),
                hits: hits.clone(),
                alerted: alerted.clone(),
                cfg: config.clone(),
                closing: None,
            });
        }

        let state = state.clone();
        tokio::spawn(async move {
            run_window(state, config, stop, hits, alerted).await;
        });
    }

    /// 关窗并（客户端代调模式下）通知设备：任务终态 / 暂停 / 走到没有 AI 动作的航点。
    ///
    /// 服务端直连模式由窗口任务自己收尾（它知道我停的是不是别人的识别），这里补一次
    /// 「没找到」的记账：狗停满 `dwell` 之后离开时，平台会先收到「已离开航点」而把
    /// 窗口摘掉，客户端随后的 `ai_end` 已经找不到窗口，只认「到期 / `expired=true`」的
    /// 地方根本收不到结论（见 [`settle_no_hit`]）。
    ///
    /// 客户端模式**不**立刻摘窗口：见 [`Self::close_client`]。
    async fn close(&self, state: &AppState, device_code: &str, reason: &str) {
        if source_for(&state.config) == PointAiSource::Client {
            self.close_client(state, device_code, reason).await;
            return;
        }
        // 窗口是进程级单例：别的设备的终态/进度不应该把当前设备的窗口关掉
        let taken = {
            let mut guard = self.active.lock().await;
            match guard.as_ref() {
                Some(window) if window.device_code == device_code => guard.take(),
                _ => None,
            }
        };
        let Some(window) = taken else {
            return;
        };
        info!(
            "航点 AI 识别窗口关闭（{reason}）：任务 {} 航点 {}「{}」",
            window.task_code, window.point_index, window.point_name
        );
        window.stop.notify_one();
        self.settle_and_remember(state, window).await;
    }

    /// 客户端模式的关窗：**先让客户端把话说完**。
    ///
    /// 下发 `ai_stop` 之后，客户端的收尾（停识别 → 板端 `_plan_flush` 落盘 → 扫盘 →
    /// `ai_detect` / `ai_alert` / `ai_end`）才真正开始，本窗口的结论（含证据图）就在
    /// 这几百毫秒到几秒里回来。以前这里立刻 `settle_no_hit` 判「0 ≠ N」，那条真结论
    /// 回来时窗口已经没了，被当成失效上报直接丢弃：群里既写错数量、也丢了命中帧。
    ///
    /// 所以：窗口标成 `closing` 留在 `active` 里（迟到的上报还能归到它身上），超时
    /// `CLOSE_GRACE` 或客户端 `ai_end` 到了才结算（后者在 [`Self::on_client_end`]）。
    async fn close_client(&self, state: &AppState, device_code: &str, reason: &str) {
        let stop = {
            let mut guard = self.active.lock().await;
            let Some(window) = guard.as_mut() else {
                return;
            };
            // 窗口是进程级单例：别的设备的终态/进度不应该把当前设备的窗口关掉
            if window.device_code != device_code {
                return;
            }
            if window.closing.is_some() {
                // 已经收尾过（先「已离开航点」、后任务终态）：`ai_stop` 也发过了
                return;
            }
            window.closing = Some(reason.to_string());
            info!(
                "航点 AI 识别窗口关闭（{reason}）：任务 {} 航点 {}「{}」，等客户端回报结论（最多 {}s）",
                window.task_code,
                window.point_index,
                window.point_name,
                CLOSE_GRACE.as_secs()
            );
            window.stop.clone()
        };
        let _ = crate::gateway::send_message_to_device(
            &state.device_connections,
            device_code,
            "ai_stop",
            json!({ "reason": reason }),
        )
        .await;
        spawn_close_grace(state.clone(), stop);
    }

    /// 结算一个窗口（「一个都没找到」记账）+ 留痕。
    ///
    /// 关窗、宽限期到点、被新航点顶替都走这里：判定规则只有 [`settle_no_hit`] 一份。
    async fn settle_and_remember(&self, state: &AppState, window: Window) {
        let hit_seen = !window.hits.lock().await.is_empty();
        let enough = watched_enough(&window.cfg, window.watched());
        settle_no_hit(state, &window.cfg, hit_seen, &window.alerted, enough).await;
        self.remember(window).await;
    }

    /// 把一个刚关掉的窗口记到 `recent` 里（只留 30 分钟 / 最近 32 个）。
    ///
    /// 迟到的证据图（`ai_evidence`）要靠它找到「本窗口哪条告警还没图」。
    async fn remember(&self, window: Window) {
        let hits = window.hits.lock().await.clone();
        let mut recent = self.recent.lock().await;
        recent.retain(|item| item.closed_at.elapsed() < RECENT_WINDOW_TTL);
        recent.push(ClosedWindow {
            window_id: window.window_id,
            device_code: window.device_code,
            closed_at: Instant::now(),
            hits,
        });
        let overflow = recent.len().saturating_sub(RECENT_WINDOW_KEEP);
        if overflow > 0 {
            recent.drain(..overflow);
        }
    }

    /// 给一张迟到的证据图认领“该归到哪条告警”：返回告警码，并把这条检出标成「已带图」。
    ///
    /// 先看还开着的窗口，再看刚关掉的（`recent`）——后者才是常态，因为平台
    /// 「已离开航点」关窗在先，客户端收尾扫盘在后（见 [`ClosedWindow`]）。
    async fn claim_alert_for_evidence(&self, device_code: &str, window_id: &str) -> Option<String> {
        {
            let guard = self.active.lock().await;
            if let Some(window) = guard.as_ref()
                && window.device_code == device_code
                && (window.window_id.is_empty() || window.window_id == window_id)
            {
                let mut hits = window.hits.lock().await;
                let hit = hits.iter_mut().rev().find(|hit| !hit.image)?;
                hit.image = true;
                return hit.alert_code.clone();
            }
        }
        let mut recent = self.recent.lock().await;
        let window = recent.iter_mut().rev().find(|item| {
            item.device_code == device_code
                && (window_id.is_empty() || item.window_id == window_id)
                && item.closed_at.elapsed() < RECENT_WINDOW_TTL
        })?;
        let hit = window.hits.iter_mut().rev().find(|hit| !hit.image)?;
        hit.image = true;
        hit.alert_code.clone()
    }
}
