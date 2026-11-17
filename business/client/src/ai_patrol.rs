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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::Instant;
use tracing::{debug, info, warn};

/// 探测用的短超时：Agent 与客户端同机，3s 足够；超时就按「没接背包」处理
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
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
            secs: value.get("secs").and_then(Value::as_u64).unwrap_or(15),
            was_running: value
                .get("wasRunning")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// 当前窗口（进程内唯一：Agent 单路推理）
struct Window {
    id: String,
    task_code: String,
    point_index: i64,
    /// 停止信号：`notify_one` 留 permit，窗口任务即使在 `chunk().await` 里也能立刻醒
    cancel: Arc<Notify>,
}

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
}

impl AiPatrol {
    pub fn new(agent: impl Into<String>, enabled: bool) -> Self {
        let agent = agent.into().trim().trim_end_matches('/').to_string();
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
        {
            let mut guard = self.window.lock().await;
            // 抢占：Agent 单路，先让旧窗口收尾（它自己判断要不要停 Agent）
            if let Some(previous) = guard.take() {
                previous.cancel.notify_one();
            }
            *guard = Some(Window {
                id: start.window_id.clone(),
                task_code: start.task_code.clone(),
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
        });
        let path = if start.action == "config" {
            "/api/detect/config"
        } else {
            "/api/detect/start"
        };
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
                self.finish(&out_tx, &start, &format!("启动失败: {error}"), 0, false)
                    .await;
                return;
            }
        }

        let (hits, stopped_early) = self.consume(&out_tx, &start, &cancel).await;
        let reason = if stopped_early {
            "任务结束 / 被新航点顶替".to_string()
        } else {
            format!("窗口到期（{}s）", start.secs)
        };
        // `expired = true` 只代表「窗口正常到期」：数量任务这时一个都没找到，
        // 服务端才把它算成数量不一致（0 ≠ N）；被任务结束/新航点打断不算账。
        self.finish(&out_tx, &start, &reason, hits, !stopped_early)
            .await;
    }

    /// 读 SSE，把超过阈值的类别按「本窗口首次出现」上报；返回（上报类别数, 是否被提前打断）
    async fn consume(
        &self,
        out_tx: &mpsc::Sender<WsMessage>,
        start: &AiStart,
        cancel: &Arc<Notify>,
    ) -> (usize, bool) {
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
                return (0, false);
            }
            Err(error) => {
                warn!("航点 AI 识别：连不上 Agent 事件流: {error}");
                return (0, false);
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

        loop {
            tokio::select! {
                _ = &mut notified => return (hits, true),
                _ = tokio::time::sleep_until(deadline) => return (hits, false),
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
                                if let Some(alert) = count_alert(&event, start)
                                    && !alerted.swap(true, Ordering::Relaxed)
                                {
                                    let expect =
                                        alert.get("expectCount").and_then(|v| v.as_i64()).unwrap_or(0);
                                    let found =
                                        alert.get("foundCount").and_then(|v| v.as_i64()).unwrap_or(0);
                                    warn!(
                                        "航点 AI 识别：数量核对不一致（检查 {expect} / 检出 {found}），上报告警"
                                    );
                                    send(out_tx, "ai_alert", alert).await;
                                }
                                for (label, conf) in take_hits(&event, start.conf, &mut seen) {
                                    hits += 1;
                                    debug!("航点 AI 识别：检出 {label}（{conf:.2}）");
                                    send(
                                        out_tx,
                                        "ai_detect",
                                        json!({
                                            "windowId": start.window_id,
                                            "task": start.task,
                                            "planId": start.plan_id,
                                            "pointIndex": start.point_index,
                                            "label": label,
                                            "conf": conf,
                                        }),
                                    )
                                    .await;
                                }
                            }
                        }
                        Ok(None) => return (hits, false),
                        Err(error) => {
                            warn!("航点 AI 识别：事件流中断: {error}");
                            return (hits, false);
                        }
                    }
                }
            }
        }
    }

    /// 收尾：按需停 Agent → 上报 `ai_end` → 释放窗口槽位
    async fn finish(
        &self,
        out_tx: &mpsc::Sender<WsMessage>,
        start: &AiStart,
        reason: &str,
        hits: usize,
        expired: bool,
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

/// 从 SSE 缓冲里取出完整事件（以空行分隔），残留部分留在缓冲里等下一块
fn take_sse_events(buffer: &mut String) -> Vec<String> {
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

    /// 假 Agent 收到的调用：`(路径, body)`，body 用来验证计划参数确实透传了
    type Calls = Arc<Mutex<Vec<(String, Value)>>>;

    async fn spawn_agent(calls: Calls) -> String {
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
                    |axum::extract::State(calls): axum::extract::State<Calls>,
                     axum::Json(body): axum::Json<Value>| async move {
                        calls.lock().await.push(("stop".to_string(), body));
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/events",
                get(|| async {
                    // 真机 SSE 是长连接：推完事件挂着不断开，收尾只能靠窗口到期/打断
                    let stream = futures_util::stream::once(async {
                        Ok::<_, std::io::Error>(EVENTS.as_bytes().to_vec())
                    })
                    .chain(futures_util::stream::pending());
                    (
                        [("content-type", "text/event-stream")],
                        axum::body::Body::from_stream(stream),
                    )
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
}
