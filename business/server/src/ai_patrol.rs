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
//! - Agent 的 AI 是**单路单任务**（`single_cam`），所以窗口是进程级唯一的；
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

/// 到点后识别窗口的基础时长（秒）：`dwell.max(4) + 8`，再夹到 `[10, 30]`。
const WINDOW_MIN_SECS: u64 = 10;
/// 窗口时长默认上限（秒）：与路线「停留时长」的单点上限一致（300 s = 150 个 2 s 等待）。
/// 要收紧用 `[backpack] ai_window_max_secs` / `ROBODOG_AI_WINDOW_MAX_SECS`。
pub const DEFAULT_WINDOW_MAX_SECS: u64 = 300;
/// 推理帧率：巡检到点停留才几秒，3 fps 足够抓目标，又不会占满板端算力。
const DETECT_FPS: u32 = 3;
/// 单条 SSE 缓冲上限：Agent 事件每条几十字节，超过说明流里没有 `\n\n`，直接丢弃重来。
const SSE_BUF_LIMIT: usize = 64 * 1024;
/// 客户端代调模式的窗口序号：同一个航点可能被开多次（长停留 / 重试），
/// 必须让每次的 `windowId` 唯一，客户端才能靠它判断「我是不是已经被顶替」。
static WINDOW_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// 已关窗口的留痕保留时长 / 条数：给迟到的 `ai_evidence` 找「该归到哪条告警」用。
const RECENT_WINDOW_TTL: Duration = Duration::from_secs(30 * 60);
const RECENT_WINDOW_KEEP: usize = 32;
/// 客户端模式关窗后的宽限期（秒）：见 [`AiPatrol::close_client`]。
///
/// 平台收到「已离开航点」时**不能立刻**判「一个都没找到」：客户端的结论天然比它晚
/// ——停识别 → 板端 `_plan_flush` 把采样中的命中落盘 → 扫盘/读图 → `ai_detect` /
/// `ai_alert` → `ai_end`。2026-11-20 13:38:03 现场（TK-1112-11 PT2）就是在这条缝里丢的：
/// 平台 03.908 写下「检查 2 个，实际检出 0 个」，板端 `result.json` 其实是
/// `found_count: 1`，客户端 04.506 才把 `ai_alert` + 证据图送上来，被当成失效窗口丢掉。
/// 所以先下发 `ai_stop`、把窗口留在 `active` 里等客户端说完（最多这么久），到点还没
/// 回话才按已上报的检出结算。
const CLOSE_GRACE: Duration = Duration::from_secs(12);

/// 到点 AI 识别的进程级状态（Agent 单路推理 → 窗口也只能有一个）。
static GLOBAL: LazyLock<AiPatrol> = LazyLock::new(AiPatrol::new);

pub fn global() -> &'static AiPatrol {
    &GLOBAL
}

/// 已经在跑的一个识别窗口
struct Window {
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
struct ClosedWindow {
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
struct Hit {
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
struct WindowSink {
    hits: Arc<Mutex<Vec<Hit>>>,
    alerted: Arc<AtomicBool>,
}

pub struct AiPatrol {
    active: Mutex<Option<Window>>,
    /// 刚关掉的窗口（最近的几条）：迟到的 `ai_evidence` 靠它找告警
    recent: Mutex<Vec<ClosedWindow>>,
}

impl AiPatrol {
    fn new() -> Self {
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

    /// 客户端到点后的开窗请求（`ai_point`）。
    ///
    /// 场景解析、类别过滤、阈值、窗口时长全部复用直连模式的函数；客户端只提供
    /// 「板端有什么模型、当前在不在跑」这两件服务端看不到的事实。
    pub async fn on_client_point(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiPointPayload,
    ) {
        if payload.task_code.is_empty() {
            return;
        }
        if source_for(&state.config) != PointAiSource::Client {
            debug!(
                "航点 AI 识别：point_ai={} 由服务端直连，忽略设备 {device_code} 的开窗请求",
                state.config.point_ai
            );
            return;
        }
        let task = match load_task(&state.db, &payload.task_code).await {
            Ok(Some(task)) => task,
            Ok(None) => {
                warn!(
                    "航点 AI 识别：任务 {} 不存在，忽略设备 {device_code} 的开窗请求",
                    payload.task_code
                );
                return;
            }
            Err(error) => {
                warn!("航点 AI 识别：查任务 {} 失败: {error}", payload.task_code);
                return;
            }
        };

        let tasks = payload.tasks.as_ref();
        let scene = resolve_scene(tasks, &payload.hint, payload.scene.as_deref());
        let threshold = parse_conf(&payload.threshold)
            .or_else(|| scene.as_ref().map(|item| item.conf))
            .unwrap_or(0.5)
            .clamp(0.05, 0.95);
        let labels = scene
            .as_ref()
            .map(|item| scene_labels(tasks, &item.task, &payload.labels))
            .unwrap_or_default();
        let task_name = scene
            .as_ref()
            .map(|item| item.task.clone())
            .unwrap_or_else(|| payload.hint.clone());
        let cam = payload
            .agent_cam
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("dogcam")
            .to_string();
        let secs = window_secs(payload.dwell.unwrap_or(0), state.config.ai_window_max_secs);
        let was_running = payload.agent_running.unwrap_or(false);
        let point_name = payload
            .point_name
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "航点".to_string());
        let seq = WINDOW_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let window_id = format!(
            "AIW-{device_code}-{}-{}-{seq}",
            payload.task_code, payload.point_index
        );
        // 「检查目标数量」以路线数据为准（旧版客户端不会带 `expectCount`）；
        // 客户端带了而路线里没配时用客户端的值兜底。
        let mut expect_count = payload.expect_count.filter(|value| *value > 0);
        // 任务类型同样以路线数据为准；旧版客户端不带 checkType 时按 expectCount 推断
        let mut check_type = normalize_check_type(payload.check_type.as_deref(), expect_count);
        // 「未检出也告警」同样以路线数据为准（客户端只做转发）
        let mut alert_when_missing = payload.alert_when_missing.unwrap_or(false);
        // 「数量一致也通知」同理：新平台随 `ai_point` 带上，老平台没有就用路线数据
        let mut notify_when_count_ok = payload.notify_when_count_ok.unwrap_or(false);
        if let Some(route_id) = task.route_id
            && let Ok(Some(point)) = load_point(&state.db, route_id, payload.point_index).await
            && let Some(action) = point.find_ai_action()
        {
            expect_count = action.expect_count.or(expect_count);
            check_type = action.check_type.clone();
            alert_when_missing = action.alert_when_missing;
            notify_when_count_ok = action.notify_when_count_ok;
        }

        let cfg = WindowConfig {
            device_code: device_code.to_string(),
            device_id: task.device_id,
            park_id: task.park_id,
            task_code: payload.task_code.clone(),
            task_name: task.name.clone(),
            point_index: payload.point_index,
            point_name: point_name.clone(),
            hint: payload.hint.clone(),
            scene: payload.scene.clone(),
            labels: payload.labels.clone(),
            threshold_text: payload.threshold.clone(),
            expect_count,
            check_type,
            alert_when_missing,
            notify_when_count_ok,
            secs,
            dwell: payload.dwell.unwrap_or(0).max(0) as u64,
        };
        let stop = Arc::new(Notify::new());
        let hits = Arc::new(Mutex::new(Vec::new()));
        let alerted = Arc::new(AtomicBool::new(false));
        let previous = {
            let mut guard = self.active.lock().await;
            let previous = guard.take();
            *guard = Some(Window {
                window_id: window_id.clone(),
                device_code: device_code.to_string(),
                task_code: payload.task_code.clone(),
                point_index: payload.point_index,
                point_name: point_name.clone(),
                scene: task_name.clone(),
                labels: labels.clone(),
                threshold,
                cam: cam.clone(),
                started_at: now(),
                started: Instant::now(),
                stop: stop.clone(),
                hits,
                alerted,
                cfg: cfg.clone(),
                closing: None,
            });
            previous
        };
        if let Some(previous) = previous {
            previous.stop.notify_one();
            if previous.closing.is_some() {
                // 上一个点还在收尾宽限里（新航点已经到点，客户端来不及回结论）：
                // 先把它结算掉，不能因为被顶替就把「有/没找到」吞掉。
                self.settle_and_remember(state, previous).await;
            }
        }

        let scene_name = task_name.clone();
        // 板端媒体目录 / 固定帧数：中台参数配置是唯一真源（带 60s 缓存），
        // 客户端原样转给板端 Agent 决定「存哪、存几张」，「同步媒体」按同一目录回传。
        let media = crate::zentai::media_capture_config(&state.config).await;
        let body = json!({
            "windowId": window_id,
            "action": if was_running { "config" } else { "start" },
            "task": task_name,
            "taskCode": payload.task_code,
            "pointIndex": payload.point_index,
            "pointName": point_name,
            "planId": payload.task_code,
            "checkType": cfg.check_type,
            "expectCount": expect_count,
            // 数量一致也通知：客户端据此把板端 `plan_hit.count_ok=true` 转成 `ai_count`
            // （平台只推企业微信回执，不落告警、不建工单）
            "notifyWhenCountOk": cfg.notify_when_count_ok,
            "labels": labels,
            "conf": threshold,
            "cam": cam,
            "fps": DETECT_FPS,
            "secs": secs,
            "wasRunning": was_running,
            "mediaDir": media.picture_dir,
            "mediaFrames": media.frames,
            // 航点自动采集总开关（中台 `media_capture_enabled`，默认关）：
            // 关时板端只写告警证据图，不再往媒体归档目录攒训练图。
            "captureEnabled": media.enabled,
        });
        if !crate::gateway::send_message_to_device(
            &state.device_connections,
            device_code,
            "ai_start",
            body,
        )
        .await
        {
            warn!(
                "航点 AI 识别：设备 {device_code} 不在线，航点 {}「{point_name}」跳过",
                payload.point_index
            );
            stop_if_current(&stop).await;
            return;
        }
        info!(
            "航点 AI 识别（客户端代调）开始：任务 {} 航点 {}「{point_name}」场景={} 类别={} 阈值={threshold:.2} 相机={cam} 窗口={secs}s 模式={}",
            payload.task_code,
            payload.point_index,
            scene_name,
            if labels.is_empty() {
                "全部".to_string()
            } else {
                labels.join("/")
            },
            if was_running {
                "config（复用已开的识别）"
            } else {
                "start"
            }
        );
        spawn_client_window_timeout(state.clone(), window_id, stop, secs);
    }

    /// 客户端回传的检出（`ai_detect`）：写告警 + 任务事件 + 企业微信推送。
    ///
    /// 客户端已按阈值过滤、并按类别去重过一次；这里再做一次双保险（窗口可能已被
    /// 新航点顶替，迟到的事件必须丢掉）。
    pub async fn on_client_detect(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiDetectPayload,
    ) {
        let (cfg, threshold, hits) = {
            let guard = self.active.lock().await;
            let Some(window) = guard.as_ref() else {
                debug!("航点 AI 识别：没有活动窗口，丢弃检出 {}", payload.label);
                return;
            };
            if window.device_code != device_code
                || (!window.window_id.is_empty() && window.window_id != payload.window_id)
            {
                debug!(
                    "航点 AI 识别：窗口 {} 已失效，丢弃检出 {}",
                    payload.window_id, payload.label
                );
                return;
            }
            (window.cfg.clone(), window.threshold, window.hits.clone())
        };
        if payload.conf + 1e-9 < threshold {
            debug!(
                "航点 AI 识别：类别 {} 置信度 {:.2} 低于阈值 {threshold:.2}，忽略",
                payload.label, payload.conf
            );
            return;
        }
        if hits
            .lock()
            .await
            .iter()
            .any(|hit| hit.label == payload.label)
        {
            return;
        }
        // 客户端随检出带上来的命中帧（路径 + base64）：读不到图就只发卡片
        let image = payload.evidence_image.as_deref().and_then(|raw| {
            match crate::notify::evidence_image(raw) {
                Ok(image) => Some(image),
                Err(error) => {
                    warn!("AI 识别：客户端上报的检出证据图不可用，只发卡片: {error}");
                    None
                }
            }
        });
        record_hit(
            state,
            &cfg,
            &payload.label,
            payload.conf,
            &hits,
            &payload.evidence,
            image,
        )
        .await;
    }

    /// 客户端回报窗口结束（`ai_end`）：收尾 + 记一条任务事件。
    pub async fn on_client_end(&self, state: &AppState, device_code: &str, payload: &AiEndPayload) {
        let taken = {
            let mut guard = self.active.lock().await;
            let matches = guard
                .as_ref()
                .map(|window| {
                    window.device_code == device_code
                        && (window.window_id.is_empty() || window.window_id == payload.window_id)
                })
                .unwrap_or(false);
            if !matches {
                // 服务端已关窗（任务终态 / 离开航点 / 被新航点顶替）后才到的 ai_end：
                // 不重复记事件，但留一行 debug——排查「窗口为什么没结束记录」时能对上
                debug!(
                    "航点 AI 识别：窗口 {} 已失效或已关闭，忽略 ai_end（{}）",
                    payload.window_id, payload.reason
                );
                return;
            }
            guard.take()
        };
        let Some(window) = taken else {
            return;
        };
        let hits = window.hits.lock().await.len();
        let at = now();
        // 「看满了一个都没找到」：数量任务算数量不一致，exist + alertWhenMissing 算未检出。
        // 看满 = 客户端回报窗口正常到期，或窗口实际已开够 `dwell` 秒（见 [`settle_no_hit`]）。
        let enough = payload.expired || watched_enough(&window.cfg, window.watched());
        settle_no_hit(
            state,
            &window.cfg,
            payload.hits > 0 || hits > 0,
            &window.alerted,
            enough,
        )
        .await;
        info!(
            "航点 AI 识别结束（客户端回报）：任务 {} 航点 {}「{}」{}，上报 {hits} 类目标",
            window.task_code, window.point_index, window.point_name, payload.reason
        );
        let content = format!(
            "AI 识别窗口结束：{}（上报 {hits} 类目标，客户端回报 {}）",
            if payload.reason.trim().is_empty() {
                "无原因"
            } else {
                payload.reason.as_str()
            },
            payload.hits
        );
        if let Err(error) = insert_task_event(&state.db, &window.cfg, &at, &content).await {
            warn!("AI 识别：写任务事件失败: {error}");
        }
        // 客户端已经收尾了，但图可能还在后面（`ai_evidence` 跟着 `ai_end` 到）：留下窗口痕迹
        self.remember(window).await;
    }

    /// 客户端转发的「数量不一致」（`ai_alert`）：告警中心 + 任务时间线 + 企业微信。
    ///
    /// 判定在板端做（Agent 知道本帧到底画出了几个框，也有证据图和 `result.json`）；
    /// 这里只做归属校验与去重：同一个窗口只落一条告警，迟到（窗口已被新航点顶替）
    /// 的直接丢弃，避免旧航点的结论污染新航点。
    pub async fn on_client_alert(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiAlertPayload,
    ) {
        let (cfg, alerted, expect, check_type) = {
            let guard = self.active.lock().await;
            let Some(window) = guard.as_ref() else {
                debug!("航点 AI 识别：没有活动窗口，丢弃数量告警");
                return;
            };
            if window.device_code != device_code
                || (!window.window_id.is_empty() && window.window_id != payload.window_id)
            {
                debug!(
                    "航点 AI 识别：窗口 {} 已失效，丢弃数量告警",
                    payload.window_id
                );
                return;
            }
            (
                window.cfg.clone(),
                window.alerted.clone(),
                window.cfg.expect_count,
                window.cfg.check_type.clone(),
            )
        };
        if check_type != "count" {
            // 存在性任务不判数量：脏数据/旧客户端误报一律丢弃
            debug!("航点 AI 识别：本窗口是存在性任务，忽略数量告警");
            return;
        }
        let Some(expect) = payload.expect_count.filter(|value| *value > 0).or(expect) else {
            // 没配「检查目标数量」却收到告警：按脏数据丢弃（不猜数量）
            warn!("航点 AI 识别：收到数量告警但本窗口没配检查数量，忽略: {payload:?}");
            return;
        };
        if alerted.swap(true, Ordering::Relaxed) {
            debug!("航点 AI 识别：本窗口已报过数量告警，忽略重复上报");
            return;
        }
        let found = payload.found_count.unwrap_or(0).max(0);
        // 客户端随告警带上来的证据图（base64）；校验不过就只发卡片
        let image = payload.evidence_image.as_deref().and_then(|raw| {
            match crate::notify::evidence_image(raw) {
                Ok(image) => Some(image),
                Err(error) => {
                    warn!("AI 识别：客户端上报的证据图不可用，只发卡片: {error}");
                    None
                }
            }
        });
        record_count_alert(state, &cfg, found, expect, &payload.evidence, image).await;
    }

    /// 客户端转发的「数量核对一致」（`ai_count`）：**只推企业微信回执**。
    ///
    /// 数量任务默认只有「不一致」才出声（[`record_count_alert`]）；有的现场想把
    /// 「这轮核对上了」也留在群里，动作里勾了 `notifyWhenCountOk` 才走到这里。
    /// 这类消息不是告警：不落 `alerts`、不派单、不建工单（见 [`crate::notify::count_matched`]）。
    pub async fn on_client_count(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiCountPayload,
    ) {
        let (cfg, alerted, expect) = {
            let guard = self.active.lock().await;
            let Some(window) = guard.as_ref() else {
                // 板端 `plan_hit` 可能在窗口收尾（`ai_stop`）之后才落盘：那时没有活动窗口，
                // 没有告警要补图，也没有任务上下文，只能丢（下面留一行 debug 可查）
                debug!("航点 AI 识别：没有活动窗口，丢弃数量一致通知");
                return;
            };
            if window.device_code != device_code
                || (!window.window_id.is_empty() && window.window_id != payload.window_id)
            {
                debug!(
                    "航点 AI 识别：窗口 {} 已失效，丢弃数量一致通知",
                    payload.window_id
                );
                return;
            }
            (
                window.cfg.clone(),
                window.alerted.clone(),
                window.cfg.expect_count,
            )
        };
        if cfg.check_type != "count" {
            // 存在性任务没有「数量一致」这种结论：脏数据 / 旧客户端误报一律丢弃
            debug!("航点 AI 识别：本窗口是存在性任务，忽略数量一致通知");
            return;
        }
        if !cfg.notify_when_count_ok {
            // 动作里没勾「数量一致也通知」：客户端本不该发（新客户端会按 `ai_start` 自己门控）
            debug!("航点 AI 识别：本窗口未开启「数量一致也通知」，忽略 ai_count");
            return;
        }
        let Some(expect) = payload.expect_count.filter(|value| *value > 0).or(expect) else {
            warn!("航点 AI 识别：收到数量一致通知但本窗口没配检查数量，忽略: {payload:?}");
            return;
        };
        // 只认「确实对上了」的结论：不一致（或没带检出数）走 `ai_alert` 那条路，
        // 这里**不能**拿 expect 当成 found 凑一个一致出来
        let Some(found) = payload.found_count.filter(|value| *value >= 0) else {
            warn!("航点 AI 识别：数量一致通知没带 foundCount，忽略: {payload:?}");
            return;
        };
        if found != expect {
            debug!(
                "航点 AI 识别：数量一致通知的检出色数 {} ≠ 检查数量 {expect}，忽略（不一致走 ai_alert）",
                found
            );
            return;
        }
        // 与数量告警共用「本窗口结论已上报」标志：`plan_hit` 只发一次，抖动重发也合并
        if alerted.swap(true, Ordering::Relaxed) {
            debug!("航点 AI 识别：本窗口结论已上报过，忽略重复的数量一致通知");
            return;
        }
        let image = payload.evidence_image.as_deref().and_then(|raw| {
            match crate::notify::evidence_image(raw) {
                Ok(image) => Some(image),
                Err(error) => {
                    warn!("AI 识别：数量一致通知的证据图不可用，只发卡片: {error}");
                    None
                }
            }
        });
        record_count_ok(state, &cfg, found, expect, &payload.evidence, image).await;
    }

    /// 客户端补发的命中帧（`ai_evidence`）：告警卡片早就发过，这里**只补一张图**。
    ///
    /// 什么时候会走这条路：板端 `_plan_frame` 要连采 3 帧命中才画框存证，命中帧稀疏时
    /// 采样一直到窗口收尾（客户端 POST `/api/detect/stop` → `_plan_flush`）才结束，
    /// 那时客户端已经用 8s 兜底把卡片发出去了。停识别后图才落盘，所以单独补报。
    ///
    /// 归到哪条告警：先找还开着的窗口，再找刚关掉的（`recent`）——平台「已离开航点」
    /// 关窗在前、客户端扫盘上报在后，所以**后者才是常态**；两边都按「本窗口里还没
    /// 带过图的那条检出」定位，已带过就不重复发。
    pub async fn on_client_evidence(
        &self,
        state: &AppState,
        device_code: &str,
        payload: &AiEvidencePayload,
    ) {
        let Some(image) = payload.evidence_image.as_deref().and_then(|raw| {
            match crate::notify::evidence_image(raw) {
                Ok(image) => Some(image),
                Err(error) => {
                    warn!("AI 识别：补发的证据图不可用，跳过: {error}");
                    None
                }
            }
        }) else {
            return;
        };
        let Some(alert_code) = self
            .claim_alert_for_evidence(device_code, &payload.window_id)
            .await
        else {
            // 窗口既不在 `active` 也不在 `recent`，或者本窗口的检出都带过图：
            // 前者是真丢图（现场只看得到「没图」），用 info 留一条线索
            info!(
                "航点 AI 识别：窗口 {} 的检出都带过图（或窗口已过期），忽略补发的证据图",
                payload.window_id
            );
            return;
        };
        info!(
            "航点 AI 识别：补发命中帧证据图（{} 字节，告警 {alert_code}，任务 {} 航点 {:?}，证据：{}）",
            image.bytes,
            payload.task_code,
            payload.point_index,
            payload.evidence.trim()
        );
        crate::notify::alert_evidence(state, &alert_code, image).await;
    }
}

/// 客户端窗口的超时看门狗。
///
/// 客户端进程崩了 / 业务 WS 断了，「窗口结束」就永远不来；不清窗口态的话
/// `GET /api/backpack/ai-window` 会一直显示「在识别」，下次到点还会被判成
/// 「这个点已经在识别」而跳过。窗口秒数 + 10s 兜底。
fn spawn_client_window_timeout(state: AppState, window_id: String, stop: Arc<Notify>, secs: u64) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(secs.saturating_add(10))).await;
        let ai = global();
        let taken = {
            let mut guard = ai.active.lock().await;
            match guard.as_ref() {
                Some(window) if Arc::ptr_eq(&window.stop, &stop) => guard.take(),
                _ => None,
            }
        };
        let Some(window) = taken else {
            return;
        };
        let hits = window.hits.lock().await.len();
        warn!(
            "航点 AI 识别：客户端窗口 {window_id} 超过 {}s 未回报结束，服务端清掉窗口态",
            secs.saturating_add(10)
        );
        if window.closing.is_some() {
            // 已经在收尾宽限里（平台先下了 `ai_stop`，看门狗却先到点）：
            // 按已上报的检出结算，「没找到」不能因为客户端没回话就永远丢。
            ai.settle_and_remember(&state, window).await;
            return;
        }
        // 这里**不**记「没找到」：客户端的结论（有没有检出 / 停够了没有）全在它手里，
        // 它一句话都没回来说明链路己经断了，推“未检出”只会是误报。
        let content = format!("AI 识别窗口超时未收到客户端结束回报（已上报 {hits} 类目标）");
        let cfg = window.cfg.clone();
        ai.remember(window).await;
        if let Err(error) = insert_task_event(&state.db, &cfg, &now(), &content).await {
            warn!("AI 识别：写任务事件失败: {error}");
        }
    });
}

/// 关窗后的收尾看门狗：等客户端把本窗口的结论说完，最多 [`CLOSE_GRACE`] 秒。
///
/// 正常路径是客户端 `ai_end` 先到（[`AiPatrol::on_client_end`] 摘窗口并结算），这里
/// 什么也不做。只有客户端进程崩了 / 业务 WS 断了才由这里按已上报的检出结算 ——
/// 「看满了却没找到」不能因为一条永远不来的 `ai_end` 就不告警。
fn spawn_close_grace(state: AppState, stop: Arc<Notify>) {
    tokio::spawn(async move {
        tokio::time::sleep(CLOSE_GRACE).await;
        let ai = global();
        let taken = {
            let mut guard = ai.active.lock().await;
            match guard.as_ref() {
                Some(window) if Arc::ptr_eq(&window.stop, &stop) && window.closing.is_some() => {
                    guard.take()
                }
                _ => None,
            }
        };
        let Some(window) = taken else {
            return;
        };
        let hits = window.hits.lock().await.len();
        warn!(
            "航点 AI 识别：窗口 {} 收尾 {}s 内没等到客户端回报结束，按已上报的 {hits} 类目标结算",
            window.window_id,
            CLOSE_GRACE.as_secs()
        );
        ai.settle_and_remember(&state, window).await;
    });
}

/// 一个窗口的启动参数（算好后窗口任务只负责执行）
#[derive(Clone)]
struct WindowConfig {
    /// 设备编号（`0202222672400230`）：告警去重键与文案都用它
    device_code: String,
    device_id: Option<i64>,
    park_id: Option<i64>,
    task_code: String,
    task_name: String,
    point_index: i64,
    point_name: String,
    /// 动作 `param` 里的场景提示，如「烟火 / 人员 / 仪表读数」
    hint: String,
    /// 动作 `scene`：编辑器从实时视频模型目录里选的模型包（优先于 `hint` 猜词）
    scene: Option<String>,
    /// 动作 `labels`：勾选的识别类别（英文 `name`，空 = 该场景全部类别）
    labels: Vec<String>,
    /// 动作 `threshold` 原文，如「置信度 > 0.85」
    threshold_text: String,
    /// 动作 `expectCount`：检查目标数量（数量任务必填）
    expect_count: Option<i64>,
    /// 任务类型：`exist`（是否存在）/ `count`（数量是否正确）
    check_type: String,
    /// 动作 `alertWhenMissing`：`exist` 任务看满停留时长、**一个都没检出**时也告警
    ///
    /// `exist` 默认只在「检出」时告警；「应该有却没有」这种语义靠这个开关（否则只能用
    /// `count` + `expectCount`）。`count` 任务自带「0 ≠ N」告警，这个字段对它无效。
    alert_when_missing: bool,
    /// 动作 `notifyWhenCountOk`：`count` 任务数量**一致**时也推一条企业微信回执
    ///
    /// 只发群消息：不进告警中心、不派单、不建工单（见 `notify::count_matched`）。
    /// `exist` 任务没有「数量一致」这种结论，这个字段对它无效。
    notify_when_count_ok: bool,
    secs: u64,
    /// 航点停留时长（秒）：窗口开了这么久还没检出，才把「一个都没找到」算成结论
    dwell: u64,
}

/// 打开识别 → 收 SSE → 检出即告警 → 到期/抢占收尾
async fn run_window(
    state: AppState,
    config: WindowConfig,
    stop: Arc<Notify>,
    hits: Arc<Mutex<Vec<Hit>>>,
    alerted: Arc<AtomicBool>,
) {
    let started = Instant::now();
    let Ok(base) = backpack::agent_base(&state.config).map(str::to_string) else {
        // 用 WARN 而不是 DEBUG：配了「只给浏览器直连地址」（`ROBODOG_BACKPACK_BROWSER_URL`）
        // 的部署里，服务端根本连不到 Agent，航点 AI 永远起不来；现场只看得到「没告警」，
        // 必须让这条原因出现在日志里。
        warn!(
            "AI 识别：本部署的服务端访问不到背包 Agent，跳过航点「{}」。\
             需要服务端与板端同网段并配置 ROBODOG_BACKPACK_URL（浏览器直连模式不够）",
            config.point_name
        );
        // 没接背包时不要把窗口留在状态里（否则 `/backpack/ai-window` 会一直说「在识别」）
        stop_if_current(&stop).await;
        return;
    };
    let Ok(client) = backpack::api_client() else {
        return;
    };

    // 1) 现状：是否已经在跑（操作员手动开的识别），以及可用的模型场景
    let tasks_json = backpack::fetch_json(&client, &format!("{base}/api/detect/tasks"))
        .await
        .ok();
    let status = backpack::fetch_json(&client, &format!("{base}/api/detect/status"))
        .await
        .ok();
    let was_running = status
        .as_ref()
        .and_then(|value| value.get("running"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let scene = resolve_scene(tasks_json.as_ref(), &config.hint, config.scene.as_deref());
    let threshold = parse_conf(&config.threshold_text)
        .or_else(|| scene.as_ref().map(|item| item.conf))
        .unwrap_or(0.5)
        .clamp(0.05, 0.95);
    // 勾选的类别只保留该场景真实存在的，全部失效时退回「全部类别」（`labels: null`）
    let labels = scene
        .as_ref()
        .map(|item| scene_labels(tasks_json.as_ref(), &item.task, &config.labels))
        .unwrap_or_default();
    let cam = status
        .as_ref()
        .and_then(|value| value.get("default_cam"))
        .and_then(Value::as_str)
        .unwrap_or("dogcam")
        .to_string();
    let task_name = scene
        .as_ref()
        .map(|item| item.task.clone())
        .unwrap_or_else(|| config.hint.clone());

    update_resolved(&stop, &task_name, &labels, threshold, &cam).await;

    // 2) 启动（已在跑就热更新，避免「识别已在运行」被拒）
    //    计划 id / 航点下标 / 检查目标数量一并下发：板端命中即按计划目录存证，
    //    数量与 expect_count 不符就在 SSE 的 plan_hit 里带 count_alert。
    //    `capture_enabled` 是航点自动采集总开关（中台 `media_capture_enabled`，默认关）：
    //    直连模式以前不带媒体参数，板端按自己的 `MEDIA_DATA_DIR` 默默攒图，这条一并收口。
    let capture_enabled = crate::zentai::media_capture_config(&state.config)
        .await
        .enabled;
    let body = json!({
        "task": task_name,
        "labels": if labels.is_empty() {
            Value::Null
        } else {
            json!(labels)
        },
        "conf": threshold,
        "cams": [cam],
        "fps": DETECT_FPS,
        "plan_id": config.task_code,
        "point_index": config.point_index,
        "check_type": config.check_type,
        "expect_count": config.expect_count,
        "capture_enabled": capture_enabled,
    });
    if let Err(error) = start_detect(&client, &base, &body, was_running).await {
        warn!(
            "AI 识别：启动失败（航点「{}」场景 {task_name}）: {error}",
            config.point_name
        );
        stop_if_current(&stop).await;
        return;
    }
    info!(
        "航点 AI 识别开始：任务 {} 航点 {}「{}」场景={task_name} 类别={} 阈值={threshold:.2} 相机={cam} 窗口={}s",
        config.task_code,
        config.point_index,
        config.point_name,
        if labels.is_empty() {
            "全部".to_string()
        } else {
            labels.join("/")
        },
        config.secs
    );

    // 3) 收事件（到期或被打断就收尾）
    let deadline = tokio::time::Instant::now() + Duration::from_secs(config.secs);
    let sink = WindowSink {
        hits: hits.clone(),
        alerted: alerted.clone(),
    };
    let expired = consume_events(&state, &base, &config, threshold, &stop, &sink, deadline).await;
    // 看满了一个都没找到：数量任务算「数量不一致」，exist + alertWhenMissing 算「未检出」。
    // 到期算看满；被「已离开航点」提前摘掉时，只要窗口已开够 dwell 也算看满
    // （否则这类告警永远报不出来，见 [`watched_enough`]）。
    let hit_seen = !hits.lock().await.is_empty();
    let enough = expired || watched_enough(&config, started.elapsed());
    settle_no_hit(&state, &config, hit_seen, &alerted, enough).await;

    // 只在「本窗口还是当前窗口」时才去停 Agent：
    // - 被新窗口顶替时，新窗口已经接管（且可能有操作员的识别在跑），不能停；
    // - 操作员本来开着识别（was_running）时，窗口结束也不该把人家的停掉。
    if was_running {
        info!("航点 AI 识别窗口结束（保留操作员已开启的识别）");
        // 识别留着，但计划必须摘掉：否则之后画面再命中会继续写这个航点的存证/告警
        let _ = client
            .post(format!("{base}/api/detect/plan/clear"))
            .json(&json!({}))
            .send()
            .await;
        return;
    }
    if !global().replaced_by_other(&stop).await {
        let _ = client
            .post(format!("{base}/api/detect/stop"))
            .json(&json!({}))
            .send()
            .await;
        info!(
            "航点 AI 识别结束：任务 {} 航点 {}「{}」，上报 {} 类目标",
            config.task_code,
            config.point_index,
            config.point_name,
            hits.lock().await.len()
        );
    }
}

/// 读 SSE（`/api/events`），把窗口内的 `detect` 事件里的目标写进告警中心。
async fn consume_events(
    state: &AppState,
    base: &str,
    config: &WindowConfig,
    threshold: f64,
    stop: &Arc<Notify>,
    sink: &WindowSink,
    deadline: tokio::time::Instant,
) -> bool {
    // 返回值：窗口是否「正常到期」（false = 被 stop 打断 / 事件流断了）
    let hits = &sink.hits;
    let alerted = &sink.alerted;
    let Ok(client) = backpack::stream_client() else {
        return false;
    };
    let mut response = match client
        .get(format!("{base}/api/events"))
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            warn!("AI 识别：Agent 拒绝事件流 HTTP {}", response.status());
            return false;
        }
        Err(error) => {
            warn!("AI 识别：连不上 Agent 事件流: {error}");
            return false;
        }
    };

    let notified = stop.notified();
    tokio::pin!(notified);
    let mut buffer = String::new();
    let mut seen: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            _ = &mut notified => return false,
            _ = tokio::time::sleep_until(deadline) => return true,
            chunk = response.chunk() => {
                match chunk {
                    Ok(Some(bytes)) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        if buffer.len() > SSE_BUF_LIMIT {
                            warn!("AI 识别：SSE 缓冲超过上限，丢弃重来");
                            buffer.clear();
                            continue;
                        }
                        for payload in take_sse_events(&mut buffer) {
                            let Ok(event) = serde_json::from_str::<Value>(&payload) else {
                                continue;
                            };
                            // 板端已判定的「数量不一致」：每个窗口只落一条告警
                            if let Some((expect, found, evidence)) = plan_hit_alert(&event) {
                                if !alerted.swap(true, Ordering::Relaxed) {
                                    record_count_alert(state, config, found, expect, &evidence, None)
                                        .await;
                                }
                                continue;
                            }
                            // 板端已判定的「数量一致」：勾了「一致也通知」才推一条企业微信回执
                            // （不落告警、不建工单，见 [`record_count_ok`]）
                            if let Some((expect, found, evidence)) = plan_hit_count_ok(&event, config)
                            {
                                if !alerted.swap(true, Ordering::Relaxed) {
                                    record_count_ok(state, config, found, expect, &evidence, None)
                                        .await;
                                }
                                continue;
                            }
                            let Some((label, conf)) = detect_hit(&event, threshold) else {
                                continue;
                            };
                            if !seen.insert(label.clone()) {
                                continue;
                            }
                            record_hit(state, config, &label, conf, hits, "", None).await;
                        }
                    }
                    Ok(None) => return false,
                    Err(error) => {
                        warn!("AI 识别：事件流中断: {error}");
                        return false;
                    }
                }
            }
        }
    }
}

/// 检出 → 告警 + 任务事件
///
/// **数量核对任务（`checkType=count`）例外**：检出只写任务事件，不落告警、不建工单、
/// 不推企业微信——数量任务只有「检查 N 个、实际检出 M 个」这一个结论要处置，
/// 把每个检出类别都报一遍会把那条结论淹掉（见 [`record_count_alert`]）。
///
/// `evidence` / `image` 是这一帧命中图的「板端路径」和「字节」：企业微信的图片消息
/// 要字节不要路径（见 [`crate::notify::evidence_image`]），路径只留在告警正文
/// （告警中心 / 工单详情能回看）与任务时间线里，**群卡片正文不带**
/// （`notify::without_evidence_path`：图就紧跟在卡片后面，路径是噪点）。
/// 客户端代调模式下由客户端扫板端存证目录给出；服务端直连模式
/// （`ROBODOG_BACKPACK_URL`）读不到板端磁盘，两个都是空 → 只发卡片。
async fn record_hit(
    state: &AppState,
    config: &WindowConfig,
    label: &str,
    conf: f64,
    hits: &Arc<Mutex<Vec<Hit>>>,
    evidence: &str,
    image: Option<NoticeImage>,
) {
    let at = now();
    let key = ai_dedup_key(config, label);
    let mut content = format!(
        "AI 识别：任务「{}」航点「{}」检出 {}（置信度 {:.2}）",
        config.task_name, config.point_name, label, conf
    );
    if !evidence.trim().is_empty() {
        content.push_str(&format!("（证据：{}）", evidence.trim()));
    }
    info!("{content}");

    // 数量核对任务（`checkType=count`）**只认数量结论**：检出本身不是结论，
    // 「检查 N 个，实际检出 M 个」才是。每检出一种目标就落一条告警，会把真正要处置的
    // 那条数量告警淹掉（一个航点检出 3 类目标就是 4 条告警 / 4 张工单 / 4 条群消息），
    // 现场要的是「只推数量不一致」。所以这里只留任务时间线：不落告警、不建工单、
    // 不推企业微信，数量结论统一由 [`record_count_alert`] 出。
    let count_task = config.check_type == "count";
    let has_image = image.is_some();
    let alert_code = if count_task {
        None
    } else {
        match insert_alert(&state.db, config, &content, &key).await {
            Ok(alert_code) => {
                crate::notify::alert_created_with_image(state, &alert_code, image).await;
                Some(alert_code)
            }
            Err(error) => {
                warn!("AI 识别：写告警失败: {error}");
                None
            }
        }
    };
    hits.lock().await.push(Hit {
        at: at.clone(),
        label: label.to_string(),
        conf,
        alert_code,
        // 数量任务的检出没有告警可补图（`ai_evidence` 认领的是「还没带过图的检出」），
        // 标记成已带图，免得晚到的图白认领一次、还打一条「都已带过图」的日志
        image: has_image || count_task,
    });
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
}

/// 数量核对不一致 → 告警中心 + 任务时间线 + 企业微信（带证据图）
///
/// 与 [`record_hit`] 平级：检出是「找到了」，这里是「找到了但数量对不上」，
/// 两者都要能推企业微信，所以告警入库后同样走 `notify::alert_created*`。
///
/// `image` 是板端证据图（客户端从磁盘读出后随 `ai_alert` 带上）：企业微信的图片消息
/// 要字节不要路径，见 [`crate::notify::evidence_image`]。服务端直连板端 Agent 的模式下
/// 平台读不到板端磁盘，这时只有路径、没有图，卡片照发。
async fn record_count_alert(
    state: &AppState,
    config: &WindowConfig,
    found: i64,
    expect: i64,
    evidence: &str,
    image: Option<NoticeImage>,
) {
    let at = now();
    let mut content = format!(
        "AI 识别数量核对不一致：任务「{}」航点「{}」检查 {} 个，实际检出 {} 个",
        config.task_name, config.point_name, expect, found
    );
    if !evidence.trim().is_empty() {
        content.push_str(&format!("（证据：{}）", evidence.trim()));
    }
    info!("{content}");

    let key = ai_dedup_key(config, "count");
    match insert_alert(&state.db, config, &content, &key).await {
        Ok(alert_code) => crate::notify::alert_created_with_image(state, &alert_code, image).await,
        Err(error) => warn!("AI 识别：写数量告警失败: {error}"),
    }
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
}

/// 数量核对一致 → 企业微信回执 + 任务时间线（**不落告警、不派单、不建工单**）
///
/// 与 [`record_count_alert`] 是两条路：那条是「对不上，要处置」，这条是「对上了，
/// 留个回执」。只在动作勾了 `notifyWhenCountOk` 时才会被调用（见
/// [`AiPatrol::on_client_count`] 与 `plan_hit_count_ok`）。
///
/// 为什么不能复用 `insert_alert` + `alert_created`：那条链路的第一步就是自动派单
/// （`alert_flow::on_alert`），工单列表里多一条「检查 3 个、检出 3 个」只会让真正
/// 要处置的单淹掉。
async fn record_count_ok(
    state: &AppState,
    config: &WindowConfig,
    found: i64,
    expect: i64,
    evidence: &str,
    image: Option<NoticeImage>,
) {
    let at = now();
    let mut content = format!(
        "AI 识别数量核对一致：任务「{}」航点「{}」检查 {} 个，实际检出 {} 个",
        config.task_name, config.point_name, expect, found
    );
    if !evidence.trim().is_empty() {
        content.push_str(&format!("（证据：{}）", evidence.trim()));
    }
    info!("{content}");

    crate::notify::count_matched(
        state,
        crate::notify::CountMatched {
            device_code: &config.device_code,
            task_code: &config.task_code,
            task_name: &config.task_name,
            point_index: config.point_index,
            point_name: &config.point_name,
            expect,
            found,
            content: &content,
            image,
        },
    )
    .await;
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
}

/// 「应该有、但一个都没找到」→ 告警中心 + 任务时间线 + 企业微信。
///
/// 只给 `exist` + `alertWhenMissing` 的航点用：`exist` 本来只会在**检出**时告警，
/// 「灭火器不见了」这类「缺失」语义以前根本报不出来（只能用 `count` + `expectCount`）。
/// 与数量告警同理，只在「狗看够了」时才算（见 [`watched_enough`]）
/// ——被任务结束/新航点打断且停留时间没走完时，狗可能还没看够。
async fn record_missing_alert(state: &AppState, config: &WindowConfig) {
    let at = now();
    let targets = if config.labels.is_empty() {
        config.scene.clone().unwrap_or_else(|| config.hint.clone())
    } else {
        config.labels.join("/")
    };
    let content = format!(
        "AI 识别未检出：任务「{}」航点「{}」窗口 {}s 内没找到 {}（场景 {}）",
        config.task_name,
        config.point_name,
        config.secs,
        if targets.trim().is_empty() {
            "目标"
        } else {
            targets.as_str()
        },
        if config
            .scene
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
        {
            "-"
        } else {
            config.scene.as_deref().unwrap_or_default()
        }
    );
    info!("{content}");

    let key = ai_dedup_key(config, "missing");
    match insert_alert(&state.db, config, &content, &key).await {
        Ok(alert_code) => crate::notify::alert_created(state, &alert_code).await,
        Err(error) => warn!("AI 识别：写未检出告警失败: {error}"),
    }
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
}

/// 「一个都没找到」的记账入口：数量任务落「数量不一致（0 ≠ N）」，
/// `exist` + `alertWhenMissing` 落「AI 识别未检出」。
///
/// 两个调用前提都由调用方给：
/// - `hit_seen`：本窗口有没有报过检出（报过就不再判「没找到」）；
/// - `enough`：狗在这个点上看够了没有（见 [`watched_enough`]）。
///
/// 三条路（直连到期 / 客户端 `ai_end` / 平台「已离开航点」关窗）都会走到这里，
/// 用共享的 `alerted` 保证一个窗口只落一条告警。
async fn settle_no_hit(
    state: &AppState,
    config: &WindowConfig,
    hit_seen: bool,
    alerted: &Arc<AtomicBool>,
    enough: bool,
) {
    if hit_seen || !enough {
        return;
    }
    if config.check_type == "count" {
        if let Some(expect) = config.expect_count
            && !alerted.swap(true, Ordering::Relaxed)
        {
            record_count_alert(state, config, 0, expect, "", None).await;
        }
    } else if config.check_type == "exist"
        && config.alert_when_missing
        && !alerted.swap(true, Ordering::Relaxed)
    {
        record_missing_alert(state, config).await;
    }
}

/// 狗在这个点上看够了没有：窗口开了至少 `dwell.max(4)` 秒。
///
/// 为什么不用「窗口到期」当唯一标准：窗口时长是 `dwell + 8`（给 Agent 加载模型留的余量），
/// 而狗停到 `dwell` 就往下一点走了——平台收到「已离开航点」时窗口往往还差几秒没到点，
/// 于是 `expired` 恒为 false、窗口也早被摘掉，「没找到也告警」永远报不出来
/// （2026-11-18 办公室巡检「垃圾桶溢出」漏报：pt1 停 65 s、窗口 68 s）。
/// 反过来，停留 0 秒的「路过点」`dwell.max(4)` 要求至少看 4 秒，也不至于误报。
fn watched_enough(config: &WindowConfig, watched: Duration) -> bool {
    watched >= Duration::from_secs(config.dwell.max(4))
}

/// 把解析出来的场景/阈值/相机回填到窗口记录（`GET /api/backpack/ai-window` 看得到）
async fn update_resolved(
    stop: &Arc<Notify>,
    scene: &str,
    labels: &[String],
    threshold: f64,
    cam: &str,
) {
    let ai = global();
    let mut guard = ai.active.lock().await;
    if let Some(window) = guard.as_mut()
        && Arc::ptr_eq(&window.stop, stop)
    {
        window.scene = scene.to_string();
        window.labels = labels.to_vec();
        window.threshold = threshold;
        window.cam = cam.to_string();
    }
}

/// 启动失败时把自己从状态里摘掉（只摘自己，不误伤已经接管的新窗口）
async fn stop_if_current(stop: &Arc<Notify>) {
    let ai = global();
    let mut guard = ai.active.lock().await;
    if guard
        .as_ref()
        .map(|window| Arc::ptr_eq(&window.stop, stop))
        .unwrap_or(false)
    {
        guard.take();
    }
}

/* ------------------------------- 数据读取 ------------------------------- */

struct TaskRow {
    park_id: Option<i64>,
    device_id: Option<i64>,
    route_id: Option<i64>,
    name: String,
}

struct PointRow {
    name: String,
    dwell: i64,
    actions: Vec<Value>,
}

struct AiAction {
    /// 动作 `param`：场景提示（老数据 / 手填关键词）
    hint: String,
    /// 动作 `threshold`：置信度原文
    threshold: String,
    /// 动作 `scene`：路线编辑器从实时视频模型目录里选的模型包（精确命中，不猜词）
    scene: Option<String>,
    /// 动作 `labels`：勾选的识别类别（Agent 的 label `name`，空 = 该场景全部类别）
    labels: Vec<String>,
    /// 动作 `expectCount`：检查目标数量（数量任务必填）
    expect_count: Option<i64>,
    /// 动作 `checkType`：`exist`（是否存在）/ `count`（数量是否正确）
    check_type: String,
    /// 动作 `alertWhenMissing`：`exist` 任务未检出时也告警（「应该有却没有」）
    alert_when_missing: bool,
    /// 动作 `notifyWhenCountOk`：`count` 任务数量一致时也推企业微信（不建单）
    notify_when_count_ok: bool,
}

impl PointRow {
    fn find_ai_action(&self) -> Option<AiAction> {
        self.actions.iter().find_map(|action| {
            let kind = action.get("type").and_then(Value::as_str).unwrap_or("");
            if !is_ai_action(kind) {
                return None;
            }
            Some(AiAction {
                hint: action
                    .get("param")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
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
            })
        })
    }
}

/// `tasks` 里 AI 窗口需要的那几列（避免 clippy 的 `type_complexity`）
type TaskColumns = (i64, Option<i64>, Option<i64>, Option<i64>, String);

async fn load_task(db: &sqlx::SqlitePool, task_code: &str) -> Result<Option<TaskRow>, sqlx::Error> {
    let row: Option<TaskColumns> = sqlx::query_as(
        "SELECT id, park_id, device_id, route_id, task_name FROM tasks WHERE task_code = ?1",
    )
    .bind(task_code)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(_, park_id, device_id, route_id, name)| TaskRow {
        park_id,
        device_id,
        route_id,
        name,
    }))
}

async fn load_point(
    db: &sqlx::SqlitePool,
    route_id: i64,
    index: i64,
) -> Result<Option<PointRow>, sqlx::Error> {
    let raw: Option<(Option<String>,)> =
        sqlx::query_as("SELECT route_data FROM routes WHERE id = ?1")
            .bind(route_id)
            .fetch_optional(db)
            .await?;
    let Some((Some(text),)) = raw else {
        return Ok(None);
    };
    let Ok(data) = serde_json::from_str::<Value>(&text) else {
        return Ok(None);
    };
    let Some(point) = data
        .get("points")
        .and_then(Value::as_array)
        .and_then(|points| points.get(index as usize))
    else {
        return Ok(None);
    };
    Ok(Some(PointRow {
        name: point
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("航点")
            .to_string(),
        dwell: point.get("dwell").and_then(Value::as_i64).unwrap_or(0),
        actions: point
            .get("actions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    }))
}

/// 返回写入的 `alert_code`，供企业微信推送按码回查（见 `notify::alert_created`）。
///
/// `dedup_key` 是这个航点上的「同一个问题」的结构化标识（见
/// `migrations/024_alert_dedup_key.sql`）：自动工单按它去重，不再拿带置信度/证据路径的
/// 标题比相等，否则每轮巡检都会建一张新单。
async fn insert_alert(
    db: &sqlx::SqlitePool,
    config: &WindowConfig,
    content: &str,
    dedup_key: &str,
) -> Result<String, sqlx::Error> {
    let next_id = crate::ids::next_id(db, "alerts", "alerts").await?;
    let timestamp = now();
    let alert_code = format!("AL-{next_id:03}");
    sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, device_id, park_id, status,
            occurred_at, handled_at, created_at, dedup_key)
         VALUES (?1, '警告', ?2, ?3, ?4, '未处理', ?5, NULL, ?5, ?6)",
    )
    .bind(&alert_code)
    .bind(content)
    .bind(config.device_id)
    .bind(config.park_id)
    .bind(&timestamp)
    .bind(dedup_key)
    .execute(db)
    .await?;
    Ok(alert_code)
}

/// 航点 AI 告警的结构化去重键：`ai:<device>:<taskCode>:<point>:<what>`
///
/// `what` 区分同一航点上的三件事：`<label>` 检出 / `count` 数量不一致 /
/// `missing` 未检出。同一轮与跨轮的同一件事都能靠它命中同一张在办工单。
fn ai_dedup_key(config: &WindowConfig, what: &str) -> String {
    format!(
        "ai:{}:{}:{}:{}",
        config.device_code, config.task_code, config.point_index, what
    )
}

async fn insert_task_event(
    db: &sqlx::SqlitePool,
    config: &WindowConfig,
    at: &str,
    content: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('ai', ?1, ?2, ?3,
                 (SELECT id FROM tasks WHERE task_code = ?4), ?5)",
    )
    .bind(format!("AI 识别 · 航点「{}」", config.point_name))
    .bind(content)
    .bind(config.device_id)
    .bind(&config.task_code)
    .bind(at)
    .execute(db)
    .await?;
    Ok(())
}

/* ------------------------------- 纯函数 ------------------------------- */

fn is_terminal_status(status: &str) -> bool {
    crate::status::is_terminal(status)
}

fn is_arrival_stage(stage: &str) -> bool {
    matches!(stage, "patrol.point_arrived" | "patrol.waiting")
}

/// 动作是否属于「AI 识别」：动作名含 `AI` 或 `识别`。
///
/// 注意排除「红外测温」这类同样带中文的动作——它不含「识别」二字，不受影响。
fn is_ai_action(kind: &str) -> bool {
    kind.to_ascii_uppercase().contains("AI") || kind.contains("识别")
}

/// 窗口秒数：跟着停留时长走（`dwell + 8`），下限 10 s，上限由配置给（默认 300 s）。
///
/// 这里**不再**把长停留截断成 30 s：截断会让 `dwell=100` 的点只覆盖前 30 s，后面 70 s
/// 狗还站在那儿却不识别。离开航点由阶段收口（见 `is_point_stage`），不靠时长兜。
fn window_secs(dwell: i64, max_secs: u64) -> u64 {
    let base = dwell.max(4) + 8;
    let max = max_secs.max(WINDOW_MIN_SECS);
    (base as u64).clamp(WINDOW_MIN_SECS, max)
}

/// 「还在航点上」的阶段：这些阶段窗口可以继续识别。
///
/// 其余阶段（`patrol.navigating` 前往下一点、`patrol.returning*` 返航、建图/导航任务的
/// 阶段）说明狗已经离开，窗口必须立刻关掉——否则相机会把路过的东西当成这个点的目标。
fn is_point_stage(stage: &str) -> bool {
    matches!(
        stage,
        "patrol.point_arrived"
            | "patrol.waiting"
            | "patrol.photographing"
            | "patrol.point_completed"
    )
}

/// 置信度：`0.85` / `> 0.85` / `85%` 都认；`65℃` 这种温度阈值不算（>1 又会误判成 65 → 0.65）。
///
/// 规则：取第一个数字；`0 < x <= 1` 直接用，`1 < x <= 100` 且原文带 `%` 或整数时才 /100；
/// 其它情况返回 `None`，由场景默认值兜底。
fn parse_conf(text: &str) -> Option<f64> {
    let number = first_number(text)?;
    if (0.0..=1.0).contains(&number) && number > 0.0 {
        return Some(number);
    }
    if text.contains('%') && (1.0..=100.0).contains(&number) {
        return Some(number / 100.0);
    }
    None
}

fn first_number(text: &str) -> Option<f64> {
    let mut buf = String::new();
    let mut started = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() || (ch == '.' && started) {
            buf.push(ch);
            started = true;
        } else if started {
            break;
        }
    }
    if buf.is_empty() {
        return None;
    }
    buf.trim_end_matches('.').parse::<f64>().ok()
}

/// 动作里的「检查目标数量」：接受数字或数字字符串（编辑器可能是输入框），
/// 空/0/非法一律当「不核对数量」（`None`），上限 999 防脏数据。
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
    if number < 1 {
        return None;
    }
    Some(number.min(999))
}

/// 任务类型归一化：`exist`（检测是否存在）/ `count`（检测数量是否正确）。
///
/// 兼容三种来源：路线编辑器写 `checkType`；老数据只有 `expectCount`（给了数量就按
/// 数量任务算）；两者都没有就是存在性任务。中文别名也认（手改 route_data 时不踩坑）。
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

/// 板端 `plan_hit` 事件里的数量核对结论：只在 `countAlert` 为真时返回
/// （期望数量, 实际检出, 证据图路径）。
fn plan_hit_alert(event: &Value) -> Option<(i64, i64, String)> {
    if event.get("type").and_then(Value::as_str) != Some("plan_hit") {
        return None;
    }
    if event.get("count_alert").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let expect = event.get("expect_count").and_then(Value::as_i64)?;
    let found = event
        .get("found_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let evidence = event
        .get("evidence")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some((expect, found, evidence))
}

/// 板端 `plan_hit` 事件里的「数量核对一致」：只有**数量任务 + 开了「一致也通知」+
/// `count_ok=true`** 才返回（期望数量, 实际检出, 证据图路径）。
///
/// 服务端直连板端时走这条路；客户端代调模式下由客户端把同一事件转成 `ai_count`
/// （见 [`AiPatrol::on_client_count`]），两边最后都到 [`record_count_ok`]。
fn plan_hit_count_ok(event: &Value, config: &WindowConfig) -> Option<(i64, i64, String)> {
    if config.check_type != "count" || !config.notify_when_count_ok {
        return None;
    }
    if event.get("type").and_then(Value::as_str) != Some("plan_hit") {
        return None;
    }
    if event.get("count_ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let expect = event.get("expect_count").and_then(Value::as_i64)?;
    let found = event
        .get("found_count")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if found != expect {
        return None;
    }
    let evidence = event
        .get("evidence")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some((expect, found, evidence))
}

/// `[name, {properties:{...}}]` 之类都无所谓的宽松查找：把 SSE 里的一帧 `detect` 事件
/// 翻成「第一个超过阈值的类别」。
fn detect_hit(event: &Value, threshold: f64) -> Option<(String, f64)> {
    if event.get("type").and_then(Value::as_str) != Some("detect") {
        return None;
    }
    let boxes = event.get("boxes").and_then(Value::as_array)?;
    boxes.iter().find_map(|item| {
        let conf = item.get("conf").and_then(Value::as_f64)?;
        if conf < threshold {
            return None;
        }
        let label = item
            .get("zh")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .or_else(|| item.get("label").and_then(Value::as_str))
            .unwrap_or("未知目标");
        Some((label.to_string(), conf))
    })
}

/// 从 SSE 文本缓冲里切出完整事件（按空行分隔，取 `data:` 行）。
fn take_sse_events(buffer: &mut String) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(position) = buffer.find("\n\n") {
        let block: String = buffer.drain(..position + 2).collect();
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

struct Scene {
    task: String,
    conf: f64,
}

/// 把动作参数对到 Agent 的模型场景上。
///
/// 两个来源，优先级从高到低：
///
/// 1. `scene`：路线编辑器从「实时视频 → AI 识别」模型目录里选的模型包 id（如 `fire`），
///    精确匹配 `task`，不做猜词——现场选什么就跑什么；
/// 2. `hint`（动作 `param`）：历史数据 / 手填的中文提示（「烟火 / 人员 / 仪表读数」）。
///    按 `/ 、 , 空格` 切词，匹配 `task` / `name` / `desc` 与每个类别的 `name`、`zh`，
///    命中最多者胜；一个都不命中时退回第一个可用场景（并打 WARN），总比「什么都不跑」好排查。
fn resolve_scene(tasks: Option<&Value>, hint: &str, scene: Option<&str>) -> Option<Scene> {
    let list = tasks?.get("tasks")?.as_array()?;
    let wanted = scene.map(str::trim).filter(|value| !value.is_empty());

    if let Some(wanted) = wanted
        && let Some(found) = list.iter().find(|item| {
            item.get("task")
                .and_then(Value::as_str)
                .is_some_and(|task| task.eq_ignore_ascii_case(wanted))
        })
    {
        let scene = Scene {
            task: found
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            conf: found.get("conf").and_then(Value::as_f64).unwrap_or(0.5),
        };
        debug!(
            "AI 识别场景：按路线编辑器选的模型包直接命中 → {}",
            scene.task
        );
        return Some(scene);
    }
    if let Some(wanted) = wanted {
        warn!(
            "AI 识别：路线里指定的场景「{wanted}」不在板端模型列表里（模型包可能已更新），\
             退回关键词匹配，建议到路线编辑里重选场景"
        );
    }

    let tokens: Vec<String> = hint
        .split(['/', '、', ',', '，', ' '])
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(|token| token.to_lowercase())
        .collect();

    let mut best: Option<(usize, Scene)> = None;
    let mut fallback: Option<Scene> = None;
    for item in list {
        let ok = item.get("ok").and_then(Value::as_bool).unwrap_or(true);
        let task = item
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if task.is_empty() {
            continue;
        }
        let scene = Scene {
            task: task.clone(),
            conf: item.get("conf").and_then(Value::as_f64).unwrap_or(0.5),
        };
        if ok && fallback.is_none() {
            fallback = Some(Scene {
                task: scene.task.clone(),
                conf: scene.conf,
            });
        }
        let mut haystack = String::new();
        for key in ["task", "name", "desc"] {
            if let Some(text) = item.get(key).and_then(Value::as_str) {
                haystack.push_str(&text.to_lowercase());
                haystack.push(' ');
            }
        }
        if let Some(labels) = item.get("labels").and_then(Value::as_array) {
            for label in labels {
                for key in ["name", "zh"] {
                    if let Some(text) = label.get(key).and_then(Value::as_str) {
                        haystack.push_str(&text.to_lowercase());
                        haystack.push(' ');
                    }
                }
            }
        }
        let score = tokens
            .iter()
            .filter(|token| haystack.contains(token.as_str()))
            .count();
        if score > 0 && best.as_ref().map(|(top, _)| score > *top).unwrap_or(true) {
            best = Some((score, scene));
        }
    }

    if let Some((score, scene)) = best {
        debug!(
            "AI 识别场景匹配：提示「{hint}」命中 {} 个关键词 → {}",
            score, scene.task
        );
        return Some(scene);
    }
    if let Some(scene) = fallback {
        warn!(
            "AI 识别：动作参数「{hint}」没有匹配到任何模型场景，退回 {}（可在动作参数里填模型中文名）",
            scene.task
        );
        return Some(scene);
    }
    None
}

/// 把动作里勾选的识别类别对到解析出的场景上。
///
/// 只保留该场景真实存在（且未被板端标记 `ignored`）的类别名——英文 `name`，
/// 与「实时视频」下发的 `labels` 是同一套。全部失效时返回空，调用方传
/// `labels: null` 让 Agent 用该场景的全部类别（比「一个都不识别」安全）。
fn scene_labels(tasks: Option<&Value>, scene: &str, wanted: &[String]) -> Vec<String> {
    if wanted.is_empty() {
        return Vec::new();
    }
    let Some(item) = tasks
        .and_then(|value| value.get("tasks"))
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .find(|item| item.get("task").and_then(Value::as_str) == Some(scene))
        })
    else {
        return Vec::new();
    };
    let known: Vec<&str> = item
        .get("labels")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter(|label| {
                    !label
                        .get("ignored")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .filter_map(|label| label.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let keep: Vec<String> = wanted
        .iter()
        .filter(|name| known.contains(&name.as_str()))
        .cloned()
        .collect();
    if keep.len() != wanted.len() {
        warn!(
            "AI 识别：场景 {scene} 上不存在的类别已忽略（勾了 {} 个，保留 {} 个）：{wanted:?}",
            wanted.len(),
            keep.len()
        );
    }
    keep
}

/* ------------------------------- 启停 ------------------------------- */

async fn start_detect(
    client: &reqwest::Client,
    base: &str,
    body: &Value,
    was_running: bool,
) -> Result<(), String> {
    // 已在跑（操作员开的识别 / 上一个窗口没收干净）→ 热更新，别硬停人家的会话
    let action = if was_running { "config" } else { "start" };
    let response = client
        .post(format!("{base}/api/detect/{action}"))
        .json(body)
        .send()
        .await
        .map_err(|error| format!("请求失败: {error}"))?;
    let text = response.text().await.unwrap_or_default();
    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let ok = payload.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        return Ok(());
    }
    // `config` 被拒（例如识别没在跑）时补一次 `start`
    if was_running {
        let retry = client
            .post(format!("{base}/api/detect/start"))
            .json(body)
            .send()
            .await
            .map_err(|error| format!("请求失败: {error}"))?;
        let text = retry.text().await.unwrap_or_default();
        let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if payload.get("ok").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(());
        }
        return Err(payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(&text)
            .to_string());
    }
    Err(payload
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or(&text)
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expect_count_parsing_is_lenient_but_never_zero() {
        // 编辑器写数字；历史数据 / 手改可能是字符串
        assert_eq!(parse_expect_count(Some(&json!(3))), Some(3));
        assert_eq!(parse_expect_count(Some(&json!("4"))), Some(4));
        assert_eq!(parse_expect_count(Some(&json!(" 5 "))), Some(5));
        // 留空 / 0 / 负数 / 非法 = 不核对数量
        assert_eq!(parse_expect_count(None), None);
        assert_eq!(parse_expect_count(Some(&Value::Null)), None);
        assert_eq!(parse_expect_count(Some(&json!(0))), None);
        assert_eq!(parse_expect_count(Some(&json!(-2))), None);
        assert_eq!(parse_expect_count(Some(&json!(""))), None);
        assert_eq!(parse_expect_count(Some(&json!("abc"))), None);
        // 上限防脏数据
        assert_eq!(parse_expect_count(Some(&json!(100000))), Some(999));

        // 动作里的字段名是 `expectCount`（与路线编辑器一致）
        let point = PointRow {
            name: "P1".to_string(),
            dwell: 5,
            actions: vec![json!({"type": "AI 识别", "expectCount": "2", "scene": "fire"})],
        };
        let action = point.find_ai_action().expect("应识别出 AI 动作");
        assert_eq!(action.expect_count, Some(2));
    }

    #[test]
    fn check_type_is_explicit_or_inferred() {
        // 显式字段优先：存在性任务即使写了数量也不判数量
        assert_eq!(normalize_check_type(Some("exist"), Some(5)), "exist");
        assert_eq!(normalize_check_type(Some("count"), None), "count");
        // 中文 / 别名（手改 route_data 时常见）
        assert_eq!(normalize_check_type(Some("数量"), None), "count");
        assert_eq!(
            normalize_check_type(Some("检测数量是否正确"), None),
            "count"
        );
        assert_eq!(normalize_check_type(Some("检测是否存在"), None), "exist");
        // 老数据只有 expectCount：给了数量就是数量任务
        assert_eq!(normalize_check_type(None, Some(3)), "count");
        assert_eq!(normalize_check_type(None, None), "exist");

        // 路线里的动作字段名是 `checkType`，与编辑器一致
        let point = PointRow {
            name: "P1".to_string(),
            dwell: 5,
            actions: vec![json!({"type": "AI 识别", "checkType": "exist", "expectCount": 9})],
        };
        let action = point.find_ai_action().expect("应识别出 AI 动作");
        assert_eq!(action.check_type, "exist");
    }

    #[test]
    fn plan_hit_alerts_only_when_count_flagged() {
        // 命中且数量一致：不告警
        assert!(
            plan_hit_alert(&json!({
                "type": "plan_hit", "count_ok": true, "count_alert": false,
                "expect_count": 3, "found_count": 3
            }))
            .is_none()
        );
        // 普通检出帧不告警
        assert!(plan_hit_alert(&json!({"type": "detect", "boxes": []})).is_none());
        // 数量不一致：拿得到期望/实际/证据
        let (expect, found, evidence) = plan_hit_alert(&json!({
            "type": "plan_hit", "count_alert": true, "expect_count": 3,
            "found_count": 1, "evidence": "/x/y.jpg"
        }))
        .expect("数量不一致要告警");
        assert_eq!((expect, found), (3, 1));
        assert_eq!(evidence, "/x/y.jpg");
    }

    /// 数量**一致**的回执：只有「数量任务 + 动作里勾了 notifyWhenCountOk + 真的对上了」
    /// 才返回；其它一律 None（一致且没勾就该安静，不一致走告警那条路）。
    #[test]
    fn count_ok_notice_needs_the_waypoint_switch_and_a_real_match() {
        let cfg = |check_type: &str, notify: bool| WindowConfig {
            device_code: "D1".to_string(),
            device_id: None,
            park_id: None,
            task_code: "TK-1".to_string(),
            task_name: "巡检".to_string(),
            point_index: 0,
            point_name: "P1".to_string(),
            hint: String::new(),
            scene: None,
            labels: vec![],
            threshold_text: String::new(),
            expect_count: Some(3),
            check_type: check_type.to_string(),
            alert_when_missing: false,
            notify_when_count_ok: notify,
            secs: 20,
            dwell: 10,
        };
        let hit = json!({
            "type": "plan_hit", "count_ok": true, "count_alert": false,
            "expect_count": 3, "found_count": 3, "evidence": "/x/y.jpg"
        });
        let (expect, found, evidence) =
            plan_hit_count_ok(&hit, &cfg("count", true)).expect("勾了开关且一致时应该发回执");
        assert_eq!((expect, found), (3, 3));
        assert_eq!(evidence, "/x/y.jpg");
        // 没勾：默认安静（数量任务只报不一致）
        assert!(plan_hit_count_ok(&hit, &cfg("count", false)).is_none());
        // 存在性任务没有「数量一致」这种结论
        assert!(plan_hit_count_ok(&hit, &cfg("exist", true)).is_none());
        // 不一致：那是 `plan_hit_alert` 的事
        assert!(
            plan_hit_count_ok(
                &json!({"type": "plan_hit", "count_ok": false, "count_alert": true,
                         "expect_count": 3, "found_count": 2}),
                &cfg("count", true),
            )
            .is_none()
        );
    }

    #[test]
    fn notify_when_count_ok_is_parsed_from_the_action() {
        let point = PointRow {
            name: "P1".to_string(),
            dwell: 5,
            actions: vec![json!({
                "type": "AI 识别", "checkType": "count", "expectCount": 3,
                "notifyWhenCountOk": true
            })],
        };
        let action = point.find_ai_action().expect("应识别出 AI 动作");
        assert!(
            action.notify_when_count_ok,
            "编辑器写的 notifyWhenCountOk 要生效"
        );
        // 老数据没这个字段：默认关
        let legacy = PointRow {
            name: "P1".to_string(),
            dwell: 5,
            actions: vec![json!({"type": "AI 识别", "checkType": "count", "expectCount": 3})],
        };
        assert!(!legacy.find_ai_action().unwrap().notify_when_count_ok);
    }

    #[test]
    fn point_ai_source_follows_config_and_deployment() {
        // 显式配置优先于部署形态
        assert_eq!(resolve_source("client", true), PointAiSource::Client);
        assert_eq!(resolve_source("server", false), PointAiSource::Server);
        // auto：服务端能直连板端（配了 ROBODOG_BACKPACK_URL）就自己调
        assert_eq!(resolve_source("auto", true), PointAiSource::Server);
        // auto + 正式环境（云上，连不到板端）→ 客户端代调
        assert_eq!(resolve_source("auto", false), PointAiSource::Client);
        // 未知值走 auto，不能悄悄变成 server（否则航点 AI 又会「配了却没告警」）
        assert_eq!(resolve_source("", false), PointAiSource::Client);
        assert_eq!(resolve_source("garbage", true), PointAiSource::Server);
    }

    #[test]
    fn ai_action_is_recognized_and_others_are_not() {
        assert!(is_ai_action("AI 识别"));
        assert!(is_ai_action("ai detect"));
        assert!(is_ai_action("目标识别"));
        assert!(!is_ai_action("可见光拍照"));
        assert!(!is_ai_action("红外测温"));
        assert!(!is_ai_action("静默通行"));
    }

    #[test]
    fn arrival_stages_are_the_patrol_point_events() {
        assert!(is_arrival_stage("patrol.point_arrived"));
        assert!(is_arrival_stage("patrol.waiting"));
        assert!(!is_arrival_stage("patrol.navigating"));
        assert!(!is_arrival_stage("patrol.completed"));
    }

    #[test]
    fn terminal_status_closes_window() {
        assert!(is_terminal_status("已完成"));
        assert!(is_terminal_status("异常"));
        assert!(is_terminal_status("已暂停"));
        assert!(!is_terminal_status("执行中"));
    }

    #[test]
    fn window_follows_dwell_up_to_configured_max() {
        let max = DEFAULT_WINDOW_MAX_SECS;
        // dwell 0 → max(4)+8 = 12
        assert_eq!(window_secs(0, max), 12);
        assert_eq!(window_secs(6, max), 14);
        // 下限 4 s 的 dwell 已经给出 12 s，所以 2 s 的点也是 12 s（模型加载要 1~3 s）
        assert_eq!(window_secs(2, max), 12);
        // 长停留不再被截断：停 100 s 就识别 108 s（以前是 30 s）
        assert_eq!(window_secs(100, max), 108);
        // 停留顶格 300 s（路线单点上限）时，+8 s 的余量被配置上限吃掉，正好 300 s
        assert_eq!(window_secs(300, max), 300);
        // 上限可收紧（运维怕误报就压小），且不会低于下限
        assert_eq!(window_secs(100, 30), 30);
        assert_eq!(window_secs(100, 1), WINDOW_MIN_SECS);
    }

    #[test]
    fn point_stages_keep_window_others_do_not() {
        for stage in [
            "patrol.point_arrived",
            "patrol.waiting",
            "patrol.photographing",
            "patrol.point_completed",
        ] {
            assert!(is_point_stage(stage), "{stage} 是在航点上");
        }
        for stage in [
            "patrol.navigating",
            "patrol.returning",
            "patrol.returning_to_charge",
            "navigation.navigating",
            "mapping.running",
            "",
        ] {
            assert!(!is_point_stage(stage), "{stage} 已经离开航点");
        }
    }

    #[test]
    fn conf_accepts_decimal_and_percent_only() {
        assert_eq!(parse_conf("置信度 > 0.85"), Some(0.85));
        assert_eq!(parse_conf("0.6"), Some(0.6));
        assert_eq!(parse_conf("> 85%"), Some(0.85));
        // 温度阈值不是置信度（没有 % 且 >1）
        assert_eq!(parse_conf("> 65℃ 生成告警"), None);
        assert_eq!(parse_conf("—"), None);
    }

    #[test]
    fn sse_blocks_are_split_and_partial_stays() {
        let mut buffer = String::from(
            "data: {\"type\":\"detect\",\"boxes\":[{},{}]}\n\ndata: {\"type\":\"telemetry\"}\n\ndata: {\"partial\":",
        );
        let events = take_sse_events(&mut buffer);
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("detect"));
        // 没收到空行的那半截留在缓冲里等下一块
        assert!(buffer.contains("partial"));
    }

    #[test]
    fn detect_hit_needs_type_and_threshold() {
        let event = json!({
            "type": "detect",
            "boxes": [
                {"zh": "烟", "conf": 0.4},
                {"zh": "火焰", "conf": 0.91}
            ]
        });
        assert_eq!(detect_hit(&event, 0.8), Some(("火焰".to_string(), 0.91)));
        assert_eq!(detect_hit(&event, 0.95), None);
        assert_eq!(detect_hit(&json!({"type": "telemetry"}), 0.1), None);
    }

    #[test]
    fn scene_falls_back_to_first_ready_task() {
        let tasks = json!({
            "tasks": [
                {"task": "fire", "name": "烟火检测", "conf": 0.4, "ok": true,
                 "labels": [{"name": "smoke", "zh": "烟雾"}]},
                {"task": "meter", "name": "仪表读数", "conf": 0.6, "ok": true,
                 "labels": [{"name": "gauge", "zh": "仪表"}]}
            ]
        });
        let scene = resolve_scene(Some(&tasks), "仪表读数", None).expect("scene");
        assert_eq!(scene.task, "meter");
        assert_eq!(scene.conf, 0.6);
        // 完全不匹配 → 退回第一个可用场景，而不是什么都不跑
        let scene = resolve_scene(Some(&tasks), "不存在的目标", None).expect("fallback");
        assert_eq!(scene.task, "fire");
        assert!(resolve_scene(None, "烟火", None).is_none());
    }

    #[test]
    fn editor_scene_wins_over_keyword_matching() {
        let tasks = json!({
            "tasks": [
                {"task": "fire", "name": "烟火检测", "conf": 0.4, "ok": true,
                 "labels": [{"name": "smoke", "zh": "烟雾"}, {"name": "flame", "zh": "火焰"}]},
                {"task": "meter", "name": "仪表读数", "conf": 0.6, "ok": true,
                 "labels": [{"name": "gauge", "zh": "仪表"}]}
            ]
        });
        // 编辑器选的模型包优先，即使关键词能命中别的场景
        let scene = resolve_scene(Some(&tasks), "仪表", Some("fire")).expect("scene");
        assert_eq!(scene.task, "fire");
        // 选错了（模型包已下线）→ 退回关键词匹配，不让窗口直接不跑
        let scene = resolve_scene(Some(&tasks), "仪表", Some("gone")).expect("fallback");
        assert_eq!(scene.task, "meter");
    }

    #[test]
    fn labels_are_filtered_to_the_resolved_scene() {
        let tasks = json!({
            "tasks": [{
                "task": "fire", "name": "烟火检测", "ok": true,
                "labels": [
                    {"name": "smoke", "zh": "烟雾"},
                    {"name": "flame", "zh": "火焰"},
                    {"name": "person", "zh": "人员", "ignored": true}
                ]
            }]
        });
        // 不存在的 / 被板端标记 ignored 的类别都丢掉
        let labels = scene_labels(
            Some(&tasks),
            "fire",
            &[
                "flame".to_string(),
                "person".to_string(),
                "ghost".to_string(),
            ],
        );
        assert_eq!(labels, vec!["flame".to_string()]);
        // 一个都没勾 / 全部无效 → 空，调用方传 `labels: null`（该场景全部类别）
        assert!(scene_labels(Some(&tasks), "fire", &[]).is_empty());
        assert!(scene_labels(Some(&tasks), "fire", &["ghost".to_string()]).is_empty());
        assert!(scene_labels(Some(&tasks), "meter", &["flame".to_string()]).is_empty());
    }

    #[test]
    fn point_reads_ai_action_from_route_data() {
        let point = PointRow {
            name: "配电柜".to_string(),
            dwell: 6,
            actions: vec![
                json!({"type": "可见光拍照", "param": "1080P"}),
                json!({"type": "AI 识别", "param": "烟火 / 人员", "threshold": "> 0.8"}),
            ],
        };
        let action = point.find_ai_action().expect("ai action");
        assert_eq!(action.hint, "烟火 / 人员");
        assert_eq!(parse_conf(&action.threshold), Some(0.8));
        assert!(action.scene.is_none());
        assert!(action.labels.is_empty());

        // 路线编辑器新写法：模型目录里选的场景 + 类别
        let structured = PointRow {
            name: "配电柜".to_string(),
            dwell: 6,
            actions: vec![json!({
                "type": "AI 识别",
                "param": "烟火检测 · 火焰 / 烟雾",
                "threshold": "置信度 ≥ 0.40",
                "scene": " fire ",
                "labels": ["flame", "smoke"]
            })],
        };
        let action = structured.find_ai_action().expect("ai action");
        assert_eq!(action.scene.as_deref(), Some("fire"));
        assert_eq!(
            action.labels,
            vec!["flame".to_string(), "smoke".to_string()]
        );
        assert_eq!(parse_conf(&action.threshold), Some(0.4));

        let plain = PointRow {
            name: "过道".to_string(),
            dwell: 0,
            actions: vec![json!({"type": "静默通行"})],
        };
        assert!(plain.find_ai_action().is_none());
    }
}
