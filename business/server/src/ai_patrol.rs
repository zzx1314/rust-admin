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
//! 动作怎么配：路线编辑器「执行参数」直接取**「实时视频 → AI 识别」的模型目录**，
//! 写成动作的 `scene`（模型包 id）+ `labels`（勾选的类别 `name`）+ `threshold`（置信度）；
//! 没有这两个字段的历史数据仍按 `param` 里的中文关键词猜场景（兼容老路线）。
//!
//! 窗口时长 = `dwell.max(4) + 8` 秒，夹在 10~30 s：Agent 加载模型要 1~3 s，
//! 太短会「还没开始识别狗就走了」，太长会在离开航点后继续把别的画面当目标。
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
use crate::gateway_types::TaskProgressPayload;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

/// 到点后识别窗口的基础时长（秒）：`dwell.max(4) + 8`，再夹到 `[10, 30]`。
const WINDOW_MIN_SECS: u64 = 10;
const WINDOW_MAX_SECS: u64 = 30;
/// 推理帧率：巡检到点停留才几秒，3 fps 足够抓目标，又不会占满板端算力。
const DETECT_FPS: u32 = 3;
/// 单条 SSE 缓冲上限：Agent 事件每条几十字节，超过说明流里没有 `\n\n`，直接丢弃重来。
const SSE_BUF_LIMIT: usize = 64 * 1024;

/// 到点 AI 识别的进程级状态（Agent 单路推理 → 窗口也只能有一个）。
static GLOBAL: LazyLock<AiPatrol> = LazyLock::new(AiPatrol::new);

pub fn global() -> &'static AiPatrol {
    &GLOBAL
}

/// 已经在跑的一个识别窗口
struct Window {
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
    /// 停止信号：`notify_one` 会留 permit，窗口任务即使在 `chunk().await` 里也能立刻醒
    stop: Arc<Notify>,
    /// 本窗口已上报的类别（同一类别只报一次，避免 3 fps 刷屏）
    hits: Arc<Mutex<Vec<Hit>>>,
}

#[derive(Clone)]
struct Hit {
    at: String,
    label: String,
    conf: f64,
}

pub struct AiPatrol {
    active: Mutex<Option<Window>>,
}

impl AiPatrol {
    fn new() -> Self {
        Self {
            active: Mutex::new(None),
        }
    }

    /// 停掉当前窗口（任务终态、设备断线、新窗口抢占时调用）。
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
                    "deviceCode": window.device_code,
                    "taskCode": window.task_code,
                    "pointIndex": window.point_index,
                    "pointName": window.point_name,
                    "scene": window.scene,
                    "labels": window.labels,
                    "threshold": window.threshold,
                    "cam": window.cam,
                    "startedAt": window.started_at,
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
            self.stop("任务终态").await;
            return;
        }
        let stage = payload.stage.as_deref().unwrap_or_default();
        if !is_arrival_stage(stage) {
            return;
        }
        let Some(point_index) = payload.point_index else {
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
            self.stop("到达无 AI 动作的航点").await;
            return;
        };

        let threshold = parse_conf(&action.threshold).unwrap_or(0.0);
        let window_secs = window_secs(point.dwell);
        let stop = Arc::new(Notify::new());
        let hits = Arc::new(Mutex::new(Vec::new()));

        // 抢占：Agent 单路，先关旧窗口再开新窗口
        {
            let mut guard = self.active.lock().await;
            if let Some(previous) = guard.take() {
                previous.stop.notify_one();
            }
            *guard = Some(Window {
                device_code: device_code.to_string(),
                task_code: payload.task_code.clone(),
                point_index,
                point_name: point.name.clone(),
                scene: action.hint.clone(),
                labels: Vec::new(),
                threshold,
                cam: String::new(),
                started_at: now(),
                stop: stop.clone(),
                hits: hits.clone(),
            });
        }

        let config = WindowConfig {
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
            secs: window_secs,
        };
        let state = state.clone();
        tokio::spawn(async move {
            run_window(state, config, stop, hits).await;
        });
    }
}

/// 一个窗口的启动参数（`on_progress` 里算好，窗口任务只负责执行）
struct WindowConfig {
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
    secs: u64,
}

/// 打开识别 → 收 SSE → 检出即告警 → 到期/抢占收尾
async fn run_window(
    state: AppState,
    config: WindowConfig,
    stop: Arc<Notify>,
    hits: Arc<Mutex<Vec<Hit>>>,
) {
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
    consume_events(&state, &base, &config, threshold, &stop, &hits, deadline).await;

    // 只在「本窗口还是当前窗口」时才去停 Agent：
    // - 被新窗口顶替时，新窗口已经接管（且可能有操作员的识别在跑），不能停；
    // - 操作员本来开着识别（was_running）时，窗口结束也不该把人家的停掉。
    if was_running {
        info!("航点 AI 识别窗口结束（保留操作员已开启的识别）");
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
    hits: &Arc<Mutex<Vec<Hit>>>,
    deadline: tokio::time::Instant,
) {
    let Ok(client) = backpack::stream_client() else {
        return;
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
            return;
        }
        Err(error) => {
            warn!("AI 识别：连不上 Agent 事件流: {error}");
            return;
        }
    };

    let notified = stop.notified();
    tokio::pin!(notified);
    let mut buffer = String::new();
    let mut seen: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            _ = &mut notified => break,
            _ = tokio::time::sleep_until(deadline) => break,
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
                            let Some((label, conf)) = detect_hit(&event, threshold) else {
                                continue;
                            };
                            if !seen.insert(label.clone()) {
                                continue;
                            }
                            record_hit(state, config, &label, conf, hits).await;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        warn!("AI 识别：事件流中断: {error}");
                        break;
                    }
                }
            }
        }
    }
}

/// 检出 → 告警 + 任务事件
async fn record_hit(
    state: &AppState,
    config: &WindowConfig,
    label: &str,
    conf: f64,
    hits: &Arc<Mutex<Vec<Hit>>>,
) {
    let at = now();
    hits.lock().await.push(Hit {
        at: at.clone(),
        label: label.to_string(),
        conf,
    });

    let content = format!(
        "AI 识别：任务「{}」航点 {}「{}」检出 {}（置信度 {:.2}）",
        config.task_name, config.point_index, config.point_name, label, conf
    );
    info!("{content}");

    match insert_alert(&state.db, config, &content).await {
        Ok(alert_code) => crate::notify::alert_created(state, &alert_code).await,
        Err(error) => warn!("AI 识别：写告警失败: {error}"),
    }
    if let Err(error) = insert_task_event(&state.db, config, &at, &content).await {
        warn!("AI 识别：写任务事件失败: {error}");
    }
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
async fn insert_alert(
    db: &sqlx::SqlitePool,
    config: &WindowConfig,
    content: &str,
) -> Result<String, sqlx::Error> {
    let next_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM alerts")
        .fetch_one(db)
        .await?;
    let timestamp = now();
    let alert_code = format!("AL-{next_id:03}");
    sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, device_id, park_id, status,
            occurred_at, handled_at, created_at)
         VALUES (?1, '警告', ?2, ?3, ?4, '未处理', ?5, NULL, ?5)",
    )
    .bind(&alert_code)
    .bind(content)
    .bind(config.device_id)
    .bind(config.park_id)
    .bind(&timestamp)
    .execute(db)
    .await?;
    Ok(alert_code)
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
    .bind(format!(
        "AI 识别 · 航点 {}「{}」",
        config.point_index, config.point_name
    ))
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
    matches!(status, "已完成" | "异常" | "已取消" | "已暂停")
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

fn window_secs(dwell: i64) -> u64 {
    let base = dwell.max(4) + 8;
    (base as u64).clamp(WINDOW_MIN_SECS, WINDOW_MAX_SECS)
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
    fn window_follows_dwell_but_stays_bounded() {
        // dwell 0 → max(4)+8 = 12，夹到 [10,30]
        assert_eq!(window_secs(0), 12);
        assert_eq!(window_secs(6), 14);
        // 下限 4 s 的 dwell 已经给出 12 s，所以 2 s 的点也是 12 s（模型加载要 1~3 s）
        assert_eq!(window_secs(2), 12);
        assert_eq!(window_secs(120), WINDOW_MAX_SECS);
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
            &["flame".to_string(), "person".to_string(), "ghost".to_string()],
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
        assert_eq!(action.labels, vec!["flame".to_string(), "smoke".to_string()]);
        assert_eq!(parse_conf(&action.threshold), Some(0.4));

        let plain = PointRow {
            name: "过道".to_string(),
            dwell: 0,
            actions: vec![json!({"type": "静默通行"})],
        };
        assert!(plain.find_ai_action().is_none());
    }
}
