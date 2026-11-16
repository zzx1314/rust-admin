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
                self.finish(&out_tx, &start, &format!("启动失败: {error}"), 0)
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
        self.finish(&out_tx, &start, &reason, hits).await;
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
                                for (label, conf) in take_hits(&event, start.conf, &mut seen) {
                                    hits += 1;
                                    debug!("航点 AI 识别：检出 {label}（{conf:.2}）");
                                    send(
                                        out_tx,
                                        "ai_detect",
                                        json!({
                                            "windowId": start.window_id,
                                            "task": start.task,
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
        assert!(ai_point_at(&points, 1).is_none(), "拍照点不该触发识别");
        assert!(ai_point_at(&points, 0).is_some());
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

    /// 两帧同一类别（窗口内只报一次）、一帧低于阈值、一帧遥测（忽略）
    const EVENTS: &str = concat!(
        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
        "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.92}]}\n\n",
        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
        "\"boxes\":[{\"label\":\"extinguisher\",\"zh\":\"灭火器\",\"conf\":0.95}]}\n\n",
        "data: {\"type\":\"detect\",\"cam\":\"dogcam\",\"task\":\"firextinguisher\",",
        "\"boxes\":[{\"label\":\"smoke\",\"zh\":\"烟雾\",\"conf\":0.10}]}\n\n",
        "data: {\"type\":\"telemetry\",\"battery\":80}\n\n",
    );

    type Calls = Arc<Mutex<Vec<String>>>;

    async fn spawn_agent(calls: Calls) -> String {
        let app = Router::new()
            .route("/api/detect/tasks", get(|| async { TASKS }))
            .route("/api/detect/status", get(|| async { STATUS }))
            .route(
                "/api/detect/start",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>| async move {
                        calls.lock().await.push("start".to_string());
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/detect/config",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>| async move {
                        calls.lock().await.push("config".to_string());
                        axum::Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/detect/stop",
                post(
                    |axum::extract::State(calls): axum::extract::State<Calls>| async move {
                        calls.lock().await.push("stop".to_string());
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
        }
    }

    fn start_payload(secs: u64, was_running: bool) -> AiStart {
        AiStart::from_value(&json!({
            "windowId": "AIW-1-0-1",
            "action": if was_running { "config" } else { "start" },
            "task": "firextinguisher",
            "taskCode": "TK-1",
            "pointIndex": 0,
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
        assert!((detects[0]["conf"].as_f64().unwrap() - 0.92).abs() < 1e-6);

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

        // 窗口结束后要把 Agent 停掉（否则 NPU 一直占着）
        assert_eq!(calls.lock().await.as_slice(), ["start", "stop"]);
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
        assert_eq!(calls.lock().await.as_slice(), ["config"]);
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
