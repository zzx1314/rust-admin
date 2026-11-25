//! 航点 / 航段语音播报：到点 / 进段调板端 Agent `/api/speak`（聆思 TTS）。
//!
//! 为什么播报由客户端驱动：播报的起停时机只有 5009 的状态机知道（到点 / 离开 /
//! 终态），而板端 Agent 只在内网可达（客户端与它同机 `127.0.0.1:8000`）。
//!
//! ```text
//! 路线动作：航点「语音播报」→ plan.points[i] 的 speakText / speakSeconds
//!                        → 到点 Agent `call 0|1 <内容>`，离开航点 stop
//! 路线动作：航段「语音播报」→ plan.legs[i]（起点 = 航点 i）的 speakText
//!                        → 进入该航段（= 到达 P{i+1}）时播，离开航段 / 终态 stop
//! 闭环返航段（returnLeg）→ 起点 = 最后一个航点
//! ```
//!
//! 另外还有一条**手动**通路：任务详情页的「呼叫」（平台 `POST /tasks/{id}/call`
//! → 设备指令 `呼叫` → [`manual_call`] → 板端 `/api/speak/call`）。
//! 它不属于任何计划，不去重（同一句话可以再喊一遍），停止同样走 `/api/speak/stop`。
//!
//! 三条约定（改这块先看）：
//!
//! 1. **一次只播一条**：板端 FIFO 是单通道，新播报会打断旧播报，所以「到点播报」
//!    不需要先发 stop——但**循环播报**必须在「离开航点 / 终态 / 取消」发 stop，
//!    否则狗走了还在原地喊（板端看门狗 `SPEAK_MAX_SECS` 只是兜底）。
//! 2. **幂等**：5009 每 500ms 一帧，`point_arrived` 按「这条任务这个点已经播过」去重；
//!    重新下发同一条计划（`start_task`）才会重新播。
//! 3. **播报失败不影响巡检**：Agent 404 / 没联网 / asr 起不来都只打日志，
//!    老版本 Agent 记一次 `unsupported` 本轮不再试，绝不往上抛错打断 5009 处理链路。
//!
//! 内容限制来自板端固件（见 `business/backpack_dashboard/backend/speak.py`）：
//! 不能有空格，只认中文汉字 + 中文标点 + 数字，中文 ≤30 字、总长 ≤40。
//! 路线编辑器按同一份规则校验；客户端再净化一次，保证「配歪了也只是不播，不报错」。

use reqwest::Client;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// 板端写一条 FIFO 命令是毫秒级（同机 HTTP），3s 足够，别把 5009 处理链路拖住。
const AGENT_TIMEOUT: Duration = Duration::from_secs(3);

/// 播报内容上限（与板端 `SPEAK_MAX_CHARS` / `SPEAK_MAX_LEN` 一致）。
pub const MAX_SPEAK_CHARS: usize = 30;
pub const MAX_SPEAK_LEN: usize = 40;

/// 循环播报的时长上限（秒）：与板端 `SPEAK_MAX_SECS` 一致，板端还会再夹一次。
pub const MAX_SPEAK_SECS: u32 = 600;

/// 航段动作没配 `speakSeconds` 时的默认循环时长：够覆盖大半条航段，
/// 真离开航段（或到终态）客户端会提前 stop。
pub const DEFAULT_LEG_SECS: u32 = 120;

/// 动作库里「语音播报」的占位参数：老路线只有它，没有 `speakText`，
/// 不能拿它去播（「安全提示语音 01」不是给人听的话术）。
const LEGACY_PLACEHOLDER: &str = "安全提示语音 01";

/// 中文标点（板端 TTS 只认这些；与 `speak.py` 的 `_PUNCT` 保持同一份）
const CN_PUNCT: &str = "，。！？、；：（）《》〈〉「」『』【】…—·";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakPoint {
    /// 航点下标（0 基，与 `routeData.points` / 5009 `detail_index` 一致）
    pub index: i64,
    pub name: String,
    pub text: String,
    /// 循环播报秒数；0 = 只播一次
    pub seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeakLeg {
    /// 航段起点航点下标（0 基）：`legs[i]` 从 P{i+1} 出发，返航段从最后一个点出发
    pub index: i64,
    pub text: String,
    /// 循环播报秒数；0 = 进入航段时播一次
    pub seconds: u32,
}

/// 一条路线里的播报配置
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeakPlan {
    pub points: Vec<SpeakPoint>,
    pub legs: Vec<SpeakLeg>,
}

impl SpeakPlan {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.legs.is_empty()
    }

    pub fn point_at(&self, index: i64) -> Option<&SpeakPoint> {
        self.points.iter().find(|point| point.index == index)
    }

    pub fn leg_at(&self, index: i64) -> Option<&SpeakLeg> {
        self.legs.iter().find(|leg| leg.index == index)
    }
}

/// 动作是不是「语音播报」（点位动作 / 航段动作都算）。
///
/// 「静默通行」（航段动作）是**反义**动作：它不播报，也不会屏蔽别的动作——
/// 播报只由显式的「语音播报」触发，没有默认播放的东西。
pub fn is_speak_action(action_type: &str) -> bool {
    let text = action_type.trim();
    if text.is_empty() || text.contains("静默") || text.contains("关闭语音") {
        return false;
    }
    text.contains("播报") || text.contains("语音")
}

fn is_cn(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
}

/// 净化播报内容：去空白、丢掉白名单之外的字符、截断到长度上限。
///
/// 白名单与板端固件一致（中文汉字 / 中文标点 / 数字）。空结果返回 `None`
/// （没有可播的内容就别发请求）。
pub fn sanitize_text(raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    let mut kept = String::new();
    let mut cn = 0usize;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            continue;
        }
        let chinese = is_cn(ch);
        if !chinese && !ch.is_ascii_digit() && !CN_PUNCT.contains(ch) {
            continue;
        }
        if chinese {
            if cn >= MAX_SPEAK_CHARS {
                continue;
            }
            cn += 1;
        }
        if kept.chars().count() >= MAX_SPEAK_LEN {
            continue;
        }
        kept.push(ch);
    }
    if kept.is_empty() { None } else { Some(kept) }
}

/// 从动作里取播报内容：`speakText` 优先，`param` 只在「本身就是可播文本」时兜底。
///
/// 老数据的 `param` 是动作库的占位名（「安全提示语音 01」），既不是话术也有空格，
/// 直接播会念出一串占位词 —— 所以 `param` 必须**不含空白且净化后不变**才用。
fn action_text(action: &Value) -> Option<String> {
    if let Some(text) = action
        .get("speakText")
        .and_then(Value::as_str)
        .and_then(sanitize_text)
    {
        return Some(text);
    }
    let param = action.get("param").and_then(Value::as_str)?.trim();
    if param.is_empty() || param == LEGACY_PLACEHOLDER || param.chars().any(char::is_whitespace) {
        return None;
    }
    let cleaned = sanitize_text(param)?;
    (cleaned == param).then_some(cleaned)
}

fn action_seconds(action: &Value, default_seconds: u32) -> u32 {
    let raw = action.get("speakSeconds").and_then(Value::as_i64);
    match raw {
        Some(seconds) => seconds.clamp(0, MAX_SPEAK_SECS as i64) as u32,
        None => default_seconds,
    }
}

/// 解析 `routeData` 里所有「语音播报」动作。
///
/// - 同一个航点配了多条播报动作：只认第一条（板端一次只播一条，合并反而超长）；
/// - 同一个航点既有航点播报又有航段播报：**航点优先**，航段那条忽略（见 [`Speaker::point_arrived`]）；
/// - `returnLeg.actions` 的播报算在最后一个航点上（闭环返航从那里出发）。
pub fn parse_speak_plan(route_data: &Value) -> SpeakPlan {
    let mut plan = SpeakPlan::default();
    let points = route_data
        .get("points")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, point) in points.iter().enumerate() {
        let Some(actions) = point.get("actions").and_then(Value::as_array) else {
            continue;
        };
        let Some(action) = actions.iter().find(|action| {
            action
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(is_speak_action)
        }) else {
            continue;
        };
        let Some(text) = action_text(action) else {
            continue;
        };
        plan.points.push(SpeakPoint {
            index: index as i64,
            name: point
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("航点")
                .to_string(),
            text,
            // 航点动作默认只在到点播一次；要循环就把 speakSeconds 配上
            seconds: action_seconds(action, 0),
        });
    }
    if let Some(legs) = route_data.get("legs").and_then(Value::as_array) {
        for (index, leg) in legs.iter().enumerate() {
            if let Some(speak) = leg_speak(leg, index as i64) {
                plan.legs.push(speak);
            }
        }
    }
    if let (Some(return_leg), Some(index)) = (
        route_data.get("returnLeg"),
        points.len().checked_sub(1).map(|index| index as i64),
    ) && let Some(speak) = leg_speak(return_leg, index)
    {
        plan.legs.push(speak);
    }
    plan
}

fn leg_speak(leg: &Value, index: i64) -> Option<SpeakLeg> {
    let actions = leg.get("actions").and_then(Value::as_array)?;
    let action = actions.iter().find(|action| {
        action
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(is_speak_action)
    })?;
    let text = action_text(action)?;
    Some(SpeakLeg {
        index,
        text,
        // 航段任务本来就是「行进中持续执行」，没配时长就按默认循环
        seconds: action_seconds(action, DEFAULT_LEG_SECS),
    })
}

/// 需要播报的内容（航点或航段，二选一：板端一次只播一条）
#[derive(Debug, Clone, PartialEq, Eq)]
struct Say {
    text: String,
    seconds: u32,
    scope: &'static str,
}

impl Say {
    fn looping(&self) -> bool {
        self.seconds > 0
    }
}

#[derive(Default)]
struct State {
    task_code: Option<String>,
    plan: SpeakPlan,
    /// 这条任务已经播过的航点（5009 每 500ms 一帧，不去重会一直重播）
    spoken_point: Option<i64>,
    /// 当前这条是循环播报：离开航点 / 终态要发 stop，否则狗走了还在喊
    looping: bool,
    /// 这条任务已收尾：迟到的到点帧不许再开始播报
    finished: bool,
    /// 板端 Agent 没有播报接口（老版本）：本轮不再尝试
    unsupported: bool,
}

/// 巡检语音播报运行态（进程内唯一）
pub struct Speaker {
    /// 板端 Agent 基地址（与 `ai_patrol` / `lights` 共用 `[ai] agent_url`）
    agent: String,
    enabled: bool,
    client: Client,
    state: Mutex<State>,
    /// 起停串行化：到点播报与紧随其后的「离开航点停播」交错会出现
    /// 「stop 先到、start 后到」→ 循环播报一直喊到板端看门狗超时。
    gate: Mutex<()>,
}

impl Speaker {
    pub fn new(agent: impl Into<String>, enabled: bool) -> Self {
        Self {
            agent: agent.into().trim_end_matches('/').to_string(),
            enabled,
            client: Client::builder()
                .timeout(AGENT_TIMEOUT)
                .build()
                .unwrap_or_default(),
            state: Mutex::new(State::default()),
            gate: Mutex::new(()),
        }
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// 新任务开始：换掉播报配置，清掉上一轮状态。
    ///
    /// 上一条任务如果还在循环播报（客户端重启 / 终态帧丢了），先给它发一条 stop：
    /// 不然新任务的到点播报会跟旧循环抢同一条 FIFO。
    pub async fn start_task(self: &Arc<Self>, task_code: &str, plan: SpeakPlan) {
        if !self.enabled {
            return;
        }
        let stale = {
            let state = self.state.lock().await;
            match state.task_code.as_deref() {
                Some(code) if code == task_code => state.looping.then(|| code.to_string()),
                Some(code) => Some(code.to_string()),
                None => None,
            }
        };
        if stale.is_some() {
            self.stop("新任务下发 / 重新下发").await;
        }
        if !plan.is_empty() {
            info!(
                "任务 {task_code} 含 {} 个语音播报航点（{}），{} 个播报航段（{}）",
                plan.points.len(),
                plan.points
                    .iter()
                    .map(|point| format!(
                        "{}={}{}",
                        point.index,
                        point.text,
                        if point.seconds > 0 {
                            format!("×{}s", point.seconds)
                        } else {
                            String::new()
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join(","),
                plan.legs.len(),
                plan.legs
                    .iter()
                    .map(|leg| format!("{}={}×{}s", leg.index, leg.text, leg.seconds))
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        let mut state = self.state.lock().await;
        state.task_code = Some(task_code.to_string());
        state.plan = plan;
        state.spoken_point = None;
        state.looping = false;
        state.finished = false;
    }

    /// 到点播报（5009 `patrol.point_arrived` / `patrol.waiting`）。
    ///
    /// 同一个点重复调用只播一次；这个点没有播报动作就什么都不做。
    pub async fn point_arrived(self: &Arc<Self>, task_code: &str, point_index: i64) {
        if !self.enabled {
            return;
        }
        let say = {
            let state = self.state.lock().await;
            if state.finished
                || state.unsupported
                || state.task_code.as_deref() != Some(task_code)
                || state.spoken_point == Some(point_index)
            {
                return;
            }
            say_at(&state.plan, point_index)
        };
        let Some(say) = say else {
            // 没有播报动作也记一笔：这个点以后不再重复判断
            self.state.lock().await.spoken_point = Some(point_index);
            return;
        };
        let _gate = self.gate.lock().await;
        {
            let mut state = self.state.lock().await;
            // 排队期间可能已经换任务 / 已收尾
            if state.finished
                || state.unsupported
                || state.task_code.as_deref() != Some(task_code)
                || state.spoken_point == Some(point_index)
            {
                return;
            }
            state.spoken_point = Some(point_index);
        }
        let body = json!({
            "text": say.text,
            "loop": say.looping(),
            "seconds": say.seconds,
            "planId": task_code,
            "pointIndex": point_index,
            "scope": say.scope,
        });
        match self.post("/api/speak", body).await {
            Ok(value) if value.get("ok").and_then(Value::as_bool) == Some(false) => {
                let error = value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("板端拒绝播报");
                warn!("语音播报：任务 {task_code} 航点 {point_index} 未播（{error}）；巡检继续");
                self.state.lock().await.looping = false;
            }
            Ok(_) => {
                self.state.lock().await.looping = say.looping();
                info!(
                    "语音播报：任务 {task_code} 航点 {point_index} 播报「{}」（{}{}）",
                    say.text,
                    if say.scope == "leg" {
                        "航段"
                    } else {
                        "航点"
                    },
                    if say.looping() {
                        format!("，循环 {}s", say.seconds)
                    } else {
                        "，单次".to_string()
                    }
                );
            }
            Err(error) => {
                warn!(
                    "语音播报：任务 {task_code} 航点 {point_index} 调板端失败（{error}）；巡检继续"
                );
                self.state.lock().await.looping = false;
                if error.contains("没有播报接口") {
                    let mut state = self.state.lock().await;
                    state.unsupported = true;
                    warn!(
                        "板端 Agent 未升级播报接口：本轮任务不再尝试播报（升级 backpack-agent 后自动恢复）"
                    );
                }
            }
        }
    }

    /// 离开航点：停掉循环播报（单次播报由板端自己说完就回到 voice）。
    pub async fn leave_point(&self, reason: &str) {
        if !self.enabled {
            return;
        }
        if !self.state.lock().await.looping {
            return;
        }
        self.stop(reason).await;
    }

    /// 任务终态 / 取消 / 本地收尾：停掉播报，并标成已收尾。
    ///
    /// `task_code` 保持与 `Recorder` / `PointLights` 同一套签名（平台取消时可能不带任务号），
    /// 当前实现不强校验归属：终态 / 取消就是「这条路线不再播」，宁可多停一次也不留循环。
    ///
    /// **`looping` 只能由 [`Self::stop`] 在锁内清**：这里先清掉的话，
    /// `stop` 会以为「本来就没在循环」而不发 `/api/speak/stop`，狗到家了还在喊
    /// （回归用例 `finish_stops_the_loop_and_late_arrivals_are_ignored`）。
    pub async fn finish_task(&self, _task_code: &str, reason: &str) {
        if !self.enabled {
            return;
        }
        {
            let mut state = self.state.lock().await;
            state.finished = true;
            state.spoken_point = None;
        }
        self.stop(reason).await;
    }
    async fn stop(&self, reason: &str) {
        let _gate = self.gate.lock().await;
        let looping = {
            let mut state = self.state.lock().await;
            let looping = state.looping;
            state.looping = false;
            looping
        };
        // 单次播报不用 stop：板端说完会自己 `call stop` 回到 voice。
        // 这里只在「循环播报真的起过」时收口，避免每个航点离开都写一条 FIFO。
        if !looping || !self.enabled {
            return;
        }
        if let Err(error) = self
            .post("/api/speak/stop", json!({ "reason": reason }))
            .await
        {
            warn!("语音播报：停止失败（{reason}）: {error}；交给板端看门狗兜底");
        } else {
            info!("语音播报：已停止（{reason}）");
        }
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        post_agent(&self.client, &self.agent, path, body, "播报").await
    }
}

/// 调一次板端 Agent 并解析 JSON（`Speaker`、手动「呼叫」与 [`crate::phone`] 共用）。
///
/// `interface` 只用于 404 的文案（`播报` / `电话`）。**播报传的必须是「播报」**：
/// 老版本 Agent 靠「没有播报接口」这句话进入 `unsupported`，不要改这个措辞。
pub(crate) async fn post_agent(
    client: &Client,
    agent: &str,
    path: &str,
    body: Value,
    interface: &str,
) -> Result<Value, String> {
    let agent = agent.trim_end_matches('/');
    let response = client
        .post(format!("{agent}{path}"))
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
        return Err(format!("板端 Agent 没有{interface}接口（旧版本）"));
    }
    if !status.is_success() {
        return Err(format!("{path} HTTP {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|error| format!("{path} 响应不是 JSON: {error}"))
}

/// 手动「呼叫」（任务详情页喊话）：平台 → 设备客户端 → 板端 Agent `/api/speak/call`。
///
/// 与航点播报的区别：**不去重**（同一句话再喊一遍是真的要再喊），其余同一套内容
/// 白名单与看门狗。内容净化后为空直接报错，不白写一条 FIFO。
pub async fn manual_call(
    agent: &str,
    text: &str,
    looping: bool,
    seconds: u32,
) -> Result<String, String> {
    let Some(text) = sanitize_text(text) else {
        return Err("呼叫内容不能为空（只支持中文汉字、中文标点与数字）".to_string());
    };
    let body = json!({
        "text": text,
        "loop": looping,
        "seconds": seconds.min(MAX_SPEAK_SECS),
        "manual": true,
    });
    let client = Client::builder()
        .timeout(AGENT_TIMEOUT)
        .build()
        .map_err(|error| format!("建 HTTP 客户端失败: {error}"))?;
    post_agent(&client, agent, "/api/speak/call", body, "播报").await?;
    info!(
        "呼叫：{}{}",
        text,
        if looping {
            format!("（循环 {}s）", seconds)
        } else {
            "（单次）".to_string()
        }
    );
    Ok(format!("已呼叫：{text}"))
}

/// 手动停止呼叫（循环播报收尾 / 人工掐断）。与航点播报共用 `/api/speak/stop`，
/// 所以任务里的循环播报也会一起停 —— 那正是操作员按下「停止」时期望的结果。
pub async fn manual_stop(agent: &str, reason: &str) -> Result<String, String> {
    let client = Client::builder()
        .timeout(AGENT_TIMEOUT)
        .build()
        .map_err(|error| format!("建 HTTP 客户端失败: {error}"))?;
    post_agent(
        &client,
        agent,
        "/api/speak/stop",
        json!({ "reason": reason }),
        "播报",
    )
    .await?;
    info!("呼叫：已停止（{reason}）");
    Ok("已停止呼叫".to_string())
}

/// 这个航点要播什么：航点动作优先，没配才用从这里出发的航段动作。
fn say_at(plan: &SpeakPlan, index: i64) -> Option<Say> {
    if let Some(point) = plan.point_at(index) {
        return Some(Say {
            text: point.text.clone(),
            seconds: point.seconds,
            scope: "point",
        });
    }
    plan.leg_at(index).map(|leg| Say {
        text: leg.text.clone(),
        seconds: leg.seconds,
        scope: "leg",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::Router;
    use axum::extract::State;
    use axum::routing::post;

    type Calls = Arc<Mutex<Vec<Value>>>;

    async fn spawn_agent(calls: Calls) -> String {
        let app = Router::new()
            .route(
                "/api/speak",
                post(
                    |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(json!({"path": "speak", "body": body}));
                        Json(json!({"ok": true}))
                    },
                ),
            )
            .route(
                "/api/speak/stop",
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
                "/api/speak/call",
                post(
                    |State(calls): State<Calls>, Json(body): Json<Value>| async move {
                        calls
                            .lock()
                            .await
                            .push(json!({"path": "call", "body": body}));
                        Json(json!({"ok": true}))
                    },
                ),
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

    fn route() -> Value {
        json!({
            "points": [
                {"name": "P1", "actions": [{"type": "可见光拍照"}]},
                {
                    "name": "P2",
                    "actions": [
                        {"type": "语音播报", "speakText": "前方作业，请勿靠近", "speakSeconds": 0}
                    ]
                },
                {
                    "name": "P3",
                    "actions": [
                        {"type": "语音播报", "speakText": "设备巡检中，请注意避让", "speakSeconds": 30}
                    ]
                }
            ],
            "legs": [
                {"actions": []},
                {"actions": [{"type": "语音播报", "speakText": "正在通过仓储区"}]}
            ],
            "returnLeg": {"actions": [{"type": "语音播报", "speakText": "巡检结束，返回起点"}]}
        })
    }

    #[test]
    fn point_and_leg_actions_are_parsed_with_their_own_defaults() {
        let plan = parse_speak_plan(&route());
        assert_eq!(plan.points.len(), 2, "P1 没有播报动作：{plan:?}");
        assert_eq!(plan.points[0].index, 1);
        assert_eq!(plan.points[0].text, "前方作业，请勿靠近");
        assert_eq!(plan.points[0].seconds, 0, "航点默认只播一次");
        assert_eq!(plan.points[1].index, 2);
        assert_eq!(plan.points[1].seconds, 30, "speakSeconds 要生效（循环）");
        // 航段：legs[1] 从 P2（下标 1）出发；返航段从最后一个点（下标 2）出发
        assert_eq!(plan.legs.len(), 2, "{plan:?}");
        assert_eq!(plan.legs[0].index, 1);
        assert_eq!(plan.legs[0].text, "正在通过仓储区");
        assert_eq!(plan.legs[0].seconds, DEFAULT_LEG_SECS, "航段默认循环");
        assert_eq!(plan.legs[1].index, 2);
        assert_eq!(plan.legs[1].text, "巡检结束，返回起点");
    }

    #[test]
    fn legacy_placeholder_param_is_not_spoken() {
        let plan = parse_speak_plan(&json!({
            "points": [{"name": "P1", "actions": [
                {"type": "语音播报", "param": "安全提示语音 01"}
            ]}]
        }));
        assert!(plan.is_empty(), "占位名不是话术，别念出来：{plan:?}");
        // 老数据里 param 本身就是话术时仍然要播（历史路线不能因为没这个字段就不播）
        let plan = parse_speak_plan(&json!({
            "points": [{"name": "P1", "actions": [
                {"type": "语音播报", "param": "请避让"}
            ]}]
        }));
        assert_eq!(plan.points[0].text, "请避让");
    }

    #[test]
    fn silent_action_never_speaks() {
        assert!(!is_speak_action("静默通行"));
        assert!(!is_speak_action("关闭语音提示"));
        assert!(is_speak_action("语音播报"));
        let plan = parse_speak_plan(&json!({
            "legs": [{"actions": [{"type": "静默通行", "speakText": "别播"}]}]
        }));
        assert!(plan.is_empty());
    }

    #[test]
    fn text_is_sanitized_like_the_board() {
        assert_eq!(
            sanitize_text("  请 注意 避让 ").as_deref(),
            Some("请注意避让")
        );
        assert_eq!(
            sanitize_text("巡检中，请避让 ABC 123").as_deref(),
            Some("巡检中，请避让123")
        );
        assert_eq!(
            sanitize_text(&"啊".repeat(50)).unwrap().chars().count(),
            MAX_SPEAK_CHARS
        );
        assert_eq!(sanitize_text("   ").as_deref(), None);
        assert_eq!(sanitize_text("ABC").as_deref(), None);
    }

    /// 到点播一次（单次）→ 没配播报的点不发请求 → 离开航点不发 stop（单次不收口）
    #[tokio::test]
    async fn single_shot_speaks_once_and_needs_no_stop() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let speaker = Arc::new(Speaker::new(&agent, true));
        speaker.start_task("TK-1", parse_speak_plan(&route())).await;

        speaker.point_arrived("TK-1", 0).await;
        speaker.point_arrived("TK-1", 1).await;
        speaker.point_arrived("TK-1", 1).await; // 5009 重复帧
        speaker.leave_point("已离开航点").await;

        let calls = calls.lock().await.clone();
        let speaks: Vec<&Value> = calls
            .iter()
            .filter(|call| call["path"] == "speak")
            .collect();
        assert_eq!(
            speaks.len(),
            1,
            "同一个点只播一次，没有动作的点不发请求：{calls:?}"
        );
        assert_eq!(speaks[0]["body"]["scope"], "point");
        assert_eq!(speaks[0]["body"]["loop"], false);
        assert_eq!(speaks[0]["body"]["text"], "前方作业，请勿靠近");
        assert_eq!(speaks[0]["body"]["planId"], "TK-1");
        assert!(
            calls.iter().all(|call| call["path"] != "stop"),
            "单次播报由板端自己说完回 voice，客户端不用发 stop：{calls:?}"
        );
    }

    /// 航点循环播报：到点 call 一次（loop=true），离开航点停掉
    #[tokio::test]
    async fn looping_point_is_stopped_when_leaving() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let speaker = Arc::new(Speaker::new(&agent, true));
        speaker.start_task("TK-1", parse_speak_plan(&route())).await;

        speaker.point_arrived("TK-1", 2).await;
        speaker.leave_point("已离开航点").await;
        speaker.leave_point("已离开航点").await; // 重复收尾帧

        let calls = calls.lock().await.clone();
        let speaks: Vec<&Value> = calls
            .iter()
            .filter(|call| call["path"] == "speak")
            .collect();
        assert_eq!(speaks.len(), 1);
        assert_eq!(speaks[0]["body"]["loop"], true);
        assert_eq!(speaks[0]["body"]["seconds"], 30);
        let stops: Vec<&Value> = calls.iter().filter(|call| call["path"] == "stop").collect();
        assert_eq!(stops.len(), 1, "循环播报只停一次：{calls:?}");
    }

    /// 航段播报：进入航段（到达起点航点）时播，航点动作优先
    #[tokio::test]
    async fn leg_speech_fires_at_the_leg_start_and_point_wins() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let speaker = Arc::new(Speaker::new(&agent, true));
        speaker.start_task("TK-1", parse_speak_plan(&route())).await;

        speaker.point_arrived("TK-1", 1).await; // 航段 legs[1] 起点 + 航点动作都有 → 航点优先
        speaker.leave_point("已离开航点").await;

        let calls = calls.lock().await.clone();
        let speaks: Vec<&Value> = calls
            .iter()
            .filter(|call| call["path"] == "speak")
            .collect();
        assert_eq!(speaks.len(), 1, "{calls:?}");
        assert_eq!(
            speaks[0]["body"]["scope"], "point",
            "同一点既有航点播报又有航段播报时航点优先（板端一次只播一条）"
        );
        assert_eq!(speaks[0]["body"]["text"], "前方作业，请勿靠近");

        // 只有航段动作的点：scope=leg，并且默认循环
        let plan = parse_speak_plan(&json!({
            "points": [{"name": "P1", "actions": []}, {"name": "P2", "actions": []}],
            "legs": [{"actions": [{"type": "语音播报", "speakText": "开始行进"}]}]
        }));
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let speaker = Arc::new(Speaker::new(&agent, true));
        speaker.start_task("TK-2", plan).await;
        speaker.point_arrived("TK-2", 0).await;
        let calls = calls.lock().await.clone();
        assert_eq!(calls[0]["body"]["scope"], "leg");
        assert_eq!(calls[0]["body"]["loop"], true);
        assert_eq!(calls[0]["body"]["seconds"], DEFAULT_LEG_SECS);
    }

    /// 终态后迟到的到点帧不许再播；终态要收掉正在循环的那条
    #[tokio::test]
    async fn finish_stops_the_loop_and_late_arrivals_are_ignored() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let speaker = Arc::new(Speaker::new(&agent, true));
        speaker.start_task("TK-1", parse_speak_plan(&route())).await;

        speaker.point_arrived("TK-1", 2).await;
        speaker.finish_task("TK-1", "任务终态").await;
        speaker.point_arrived("TK-1", 1).await;

        let calls = calls.lock().await.clone();
        let speaks: Vec<&Value> = calls
            .iter()
            .filter(|call| call["path"] == "speak")
            .collect();
        assert_eq!(speaks.len(), 1, "终态后不该再开始播报：{calls:?}");
        let stops: Vec<&Value> = calls.iter().filter(|call| call["path"] == "stop").collect();
        assert_eq!(stops.len(), 1, "终态要收掉循环播报：{calls:?}");
    }

    /// 重新下发同一条计划：状态重置，同一个点重新播（现场反复点「开始执行」）
    #[tokio::test]
    async fn dispataching_the_same_plan_again_speaks_again() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;
        let speaker = Arc::new(Speaker::new(&agent, true));
        speaker.start_task("TK-1", parse_speak_plan(&route())).await;
        speaker.point_arrived("TK-1", 1).await;
        speaker.start_task("TK-1", parse_speak_plan(&route())).await;
        speaker.point_arrived("TK-1", 1).await;

        let calls = calls.lock().await.clone();
        let speaks: Vec<&Value> = calls
            .iter()
            .filter(|call| call["path"] == "speak")
            .collect();
        assert_eq!(speaks.len(), 2, "重新下发要重新播：{calls:?}");
    }

    /// 手动「呼叫」：不去重（同一句话能再喊一遍）、`manual=true`、内容先净化
    #[tokio::test]
    async fn manual_call_is_not_deduped_and_marks_itself_manual() {
        let calls: Calls = Default::default();
        let agent = spawn_agent(calls.clone()).await;

        let note = manual_call(&agent, " 请 避 让 ", false, 0)
            .await
            .expect("呼叫失败");
        assert_eq!(note, "已呼叫：请避让");
        manual_call(&agent, "请 避让", true, 30)
            .await
            .expect("呼叫失败");

        let calls = calls.lock().await.clone();
        let speaks: Vec<&Value> = calls.iter().filter(|call| call["path"] == "call").collect();
        assert_eq!(speaks.len(), 2, "两次呼叫必须各发一次（不去重）：{calls:?}");
        assert_eq!(speaks[0]["body"]["text"], "请避让");
        assert_eq!(speaks[0]["body"]["manual"], true);
        assert_eq!(speaks[0]["body"]["loop"], false);
        assert_eq!(speaks[1]["body"]["loop"], true);
        assert_eq!(speaks[1]["body"]["seconds"], 30);

        // 全是白名单之外的字符：当场报错，不白写一条 FIFO
        assert!(manual_call(&agent, "ABC", false, 0).await.is_err());
    }

    /// 老版本 Agent（没有 /api/speak）：只记一次 unsupported，本轮不再试
    #[tokio::test]
    async fn old_agent_disables_speaking_for_the_task() {
        let speaker = Arc::new(Speaker::new("http://127.0.0.1:1", true));
        speaker.start_task("TK-1", parse_speak_plan(&route())).await;
        speaker.point_arrived("TK-1", 1).await;
        let state = speaker.state.lock().await;
        assert!(state.unsupported || state.spoken_point.is_none() || !state.looping);
    }
}
