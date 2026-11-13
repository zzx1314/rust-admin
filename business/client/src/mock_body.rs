//! 联调用「假本体」：进程内的智航 Pro 网关模拟器（`--robot mock`）。
//!
//! 现场没有真机时，`--robot sim` 会在「拿到 client」那一步就 return，5000 之类的
//! 报文一条都不会生成。这个模块补上缺口：在客户端进程内起一个本地 WS 服务，
//! 实现真机必经的那几条链路，然后**照真机的方式连它**：
//!
//! ```text
//! control   → 1201(client_id/session_id/bulkdata_bind_token) → 1300/1301 心跳
//! bulkdata  → 1202 → 1203
//! 状态推送  → 1315（1Hz）/ 1207（模块可用性）/ 5009（500ms V2 快照）
//! 控制权    → 1204 → 1205(status_code=0, client_id=本端)
//! 地图      → 4012 → 4013（map_path_list / map_route_exist / charge_point_exist）
//! 定位      → 4030 → 4031，随后 5009 的 loc_state.status 收敛到 100
//! 任务      → 5000 kind=2 → 5001(task_id) → 5009 running → 5009 success
//!             5002 → 5003（建图时补一条 4001 保存结果）
//!             5004/5006 → 5005/5007
//!             4044 → 4045，5014 → 5015
//! ```
//!
//! 所以客户端打印的 `5000 载荷`、这里打印的 `← 5000 …` 都是**真实报文**，
//! 和接真机时逐字段一致，唯一的差别是对端是模拟器。
//!
//! 可调项（环境变量，仅联调用）：
//!
//! | 变量 | 默认 | 说明 |
//! | --- | --- | --- |
//! | `ROBODOG_MOCK_PORT` | 0（随机空闲端口） | 固定端口，便于用别的工具连它 |
//! | `ROBODOG_MOCK_STEP_MS` | 1500 | 单个航点「走完」耗时 |
//! | `ROBODOG_MOCK_DIRS` | `2026_11_10_16_15_06` | 4013 里「本体现有地图目录」，逗号分隔 |
//! | `ROBODOG_MOCK_NO_ROUTE` | 空 | 设 1 时地图上报 `map_route_exist=false`（强制 2D 栅格） |

use crate::zhpro::protocol::*;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tracing::{debug, info, warn};

/// `--robot` 里触发假本体的取值
const MOCK_ALIASES: [&str; 3] = ["mock", "mock-body", "fake"];

/// 假本体的地图根目录（与真机 M1 一致）
const MAP_ROOT: &str = "/ota/alg_data/map";
/// 4013 默认上报的地图目录名（= RT-009 冻结的真实目录名）
const DEFAULT_MAP_DIR: &str = "2026_11_10_16_15_06";
/// 5009 快照间隔（真机 500ms）
const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(500);
/// 1315 状态间隔（真机 1Hz）
const STATUS_INTERVAL_TICKS: u32 = 2;

pub fn is_mock_alias(raw: &str) -> bool {
    let value = raw.trim().to_ascii_lowercase();
    MOCK_ALIASES.contains(&value.as_str())
}

/// 按环境变量启动假本体，返回可交给 `ZhProClient::connect` 的地址
///
/// 必须带 `ws://` 前缀：`host_base` 只对裸主机补 `:10010`，带端口的裸地址会被补成
/// `127.0.0.1:40827:10010`（`HTTP format error: invalid authority`）。
pub async fn start_from_env() -> Result<String, String> {
    let port = env_u64("ROBODOG_MOCK_PORT").unwrap_or(0) as u16;
    let step_ms = env_u64("ROBODOG_MOCK_STEP_MS").unwrap_or(1500).max(100);
    let dirs = std::env::var("ROBODOG_MOCK_DIRS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|items: &Vec<String>| !items.is_empty())
        .unwrap_or_else(|| vec![DEFAULT_MAP_DIR.to_string()]);
    let route_exist = std::env::var("ROBODOG_MOCK_NO_ROUTE").is_err();
    start(port, Duration::from_millis(step_ms), dirs, route_exist).await
}

fn env_u64(key: &str) -> Option<u64> {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok())
}

/// 起假本体，返回 `ws://127.0.0.1:<port>`
pub async fn start(
    port: u16,
    step: Duration,
    map_dirs: Vec<String>,
    route_exist: bool,
) -> Result<String, String> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("假本体绑定 127.0.0.1:{port} 失败: {e}"))?;
    let addr = listener
        .local_addr()
        .map_err(|e| format!("读取假本体端口失败: {e}"))?;
    info!("假本体已监听 ws://{addr}/control（联调模式，非真机）");

    let state = Arc::new(Mutex::new(MockState::new(step, map_dirs, route_exist)));
    let client_seq = Arc::new(AtomicU64::new(1));

    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            let state = state.clone();
            let client_seq = client_seq.clone();
            tokio::spawn(async move {
                let mut path = String::new();
                // tungstenite 的握手回调签名固定，Err 类型不能换，这里显式放开 lint
                #[allow(clippy::result_large_err)]
                let callback = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                    path = req.uri().path().to_string();
                    Ok(resp)
                };
                let ws = match tokio_tungstenite::accept_hdr_async(stream, callback).await {
                    Ok(ws) => ws,
                    Err(e) => {
                        warn!("假本体握手失败（{peer}）: {e}");
                        return;
                    }
                };
                let path = path.clone();
                if path.contains("bulkdata") {
                    run_bulkdata(ws, peer.to_string()).await;
                } else {
                    let client_id = client_seq.fetch_add(1, Ordering::SeqCst) as u32;
                    run_control(ws, peer.to_string(), client_id, state).await;
                }
            });
        }
    });

    Ok(format!("ws://{addr}"))
}

/// 一次任务（5000 / 5014 创建）
struct MockTask {
    id: String,
    name: String,
    task_type: &'static str,
    kind: u32,
    lifecycle: String,
    stage: String,
    progress: f64,
    index: i64,
    total: i64,
    event_seq: i64,
    /// None = 不会自己结束（建图等外部取消）
    ends_at: Option<tokio::time::Instant>,
    started_at: tokio::time::Instant,
    /// 每个目标点耗时（一段任务可能有多个 goals，真机逐个 goal_arrived）
    goal_step: Duration,
}

impl MockTask {
    fn is_running(&self) -> bool {
        self.lifecycle == "running"
    }

    fn to_value(&self) -> Value {
        json!({
            "task_id": self.id,
            "name": self.name,
            "type": self.task_type,
            "lifecycle": self.lifecycle,
            "stage": self.stage,
            "progress": self.progress,
            "last_event": {
                "seq": self.event_seq,
                "type": self.stage,
                "data": { "index": self.index, "total": self.total }
            },
            "detail": { "last_arrived_goal": { "index": self.index, "total": self.total } }
        })
    }
}

struct MockState {
    boot_id: String,
    snapshot_seq: i64,
    task_seq: u64,
    current: Option<MockTask>,
    terminals: VecDeque<MockTask>,
    loc_status: i64,
    mode: String,
    ctrl_source: i64,
    owner: Option<u32>,
    map_dirs: Vec<String>,
    route_exist: bool,
    step: Duration,
    battery: f64,
    odometer: f64,
    pose: (f64, f64),
}

impl MockState {
    fn new(step: Duration, map_dirs: Vec<String>, route_exist: bool) -> Self {
        Self {
            boot_id: format!("mock-boot-{}", now_ms()),
            snapshot_seq: 0,
            task_seq: 0,
            current: None,
            terminals: VecDeque::new(),
            loc_status: 100,
            mode: "remote".to_string(),
            ctrl_source: 2,
            owner: None,
            map_dirs,
            route_exist,
            step,
            battery: 86.0,
            odometer: 12.4,
            pose: (0.0, 0.0),
        }
    }

    fn next_task_id(&mut self, prefix: &str) -> String {
        self.task_seq += 1;
        format!("{prefix}-{}", self.task_seq)
    }

    /// 推进一步：到点结束当前任务，并按航线进度推进
    fn tick(&mut self) {
        let Some(task) = self.current.as_mut() else {
            return;
        };
        let Some(ends_at) = task.ends_at else {
            return;
        };
        let now = tokio::time::Instant::now();
        // 逐目标推进：每个 goal_step 到点一次，发 navigation.goal_arrived（index/total）
        let goals = task.total.max(1);
        let elapsed = now.saturating_duration_since(task.started_at);
        let done = (elapsed.as_secs_f64() / task.goal_step.as_secs_f64()).floor() as i64;
        let done = done.clamp(0, goals);
        if done > task.index {
            task.index = done;
            task.event_seq += 1;
            task.stage = "navigation.goal_arrived".to_string();
        }
        if now < ends_at {
            task.progress =
                (task.index as f64 / goals as f64 * 100.0).min(99.0);
            return;
        }
        task.lifecycle = "success".to_string();
        task.stage = format!(
            "{}.completed",
            task.stage.split('.').next().unwrap_or("task")
        );
        task.progress = 100.0;
        task.event_seq += 1;
        let done = self.current.take().expect("current task exists");
        self.terminals.push_front(done);
        while self.terminals.len() > 3 {
            self.terminals.pop_back();
        }
        self.pose = (self.pose.0 + 1.0, self.pose.1);
    }

    fn snapshot(&mut self) -> Value {
        self.snapshot_seq += 1;
        json!({
            "schema_version": 2,
            "boot_id": self.boot_id,
            "snapshot_seq": self.snapshot_seq,
            "revision": self.snapshot_seq,
            "reason": "heartbeat",
            "robot": {
                "task_state": if self.current.is_some() { "running" } else { "idle" },
                "loc_state": { "status": self.loc_status, "fresh": true, "age_ms": 12 }
            },
            "current_task": self.current.as_ref().map(MockTask::to_value),
            "pending_task": Value::Null,
            "recent_terminal_tasks": self.terminals.iter().map(MockTask::to_value).collect::<Vec<_>>()
        })
    }

    fn robot_status(&mut self) -> Value {
        self.battery = (self.battery - 0.01).max(5.0);
        json!({
            "battery": {
                "power1": self.battery,
                "power2": 0.0,
                "present2": false,
                "power_supply_status1": 2,
                "temperature1": 31.5
            },
            "temp": 32.0,
            "mile_data": self.odometer,
            "mode": self.mode,
            "ctrl_source": self.ctrl_source,
            "motion_status": "stand",
            "estop": { "software": false },
            "charging_pile_connect": false,
            "fill_light": { "front": false },
            "pose": {
                "position": { "x": self.pose.0, "y": self.pose.1 },
                "orientation": { "x": 0.0, "y": 0.0, "z": 0.0, "w": 1.0 }
            }
        })
    }

    fn map_list(&self) -> Value {
        let items: Vec<Value> = self
            .map_dirs
            .iter()
            .map(|dir| {
                json!({
                    "path": format!("{MAP_ROOT}/history_map/{dir}"),
                    "map_name": dir,
                    "map_route_exist": self.route_exist,
                    "charge_point_exist": true,
                    "modify_time": "2026-11-10 16:15:06"
                })
            })
            .collect();
        json!({
            "code": 0,
            "msg": "ok",
            "map_dir_prefix": MAP_ROOT,
            "map_path_list": items
        })
    }
}

fn src_for(req_type: u32) -> u32 {
    match req_type {
        1200..=1299 => SRC_GATEWAY,
        1300..=1399 => SRC_MACHINE,
        _ => SRC_ALG,
    }
}

fn response(req_type: u32, msg_id: Option<u32>, data: Value) -> Message {
    let env = Envelope {
        head: Head {
            msg_type: req_type + 1,
            time: now_ms(),
            src: src_for(req_type),
            msg_id,
        },
        data,
    };
    Message::Text(env.to_text())
}

fn notify(msg_type: u32, src: u32, data: Value) -> Message {
    let env = Envelope {
        head: Head {
            msg_type,
            time: now_ms(),
            src,
            msg_id: None,
        },
        data,
    };
    Message::Text(env.to_text())
}

fn ok_ack(req_type: u32, msg_id: Option<u32>) -> Message {
    response(req_type, msg_id, json!({ "code": 0, "msg": "ok" }))
}

/// control 通道：握手 → 收请求 → 推 1315/5009
async fn run_control<S>(
    ws: tokio_tungstenite::WebSocketStream<S>,
    peer: String,
    client_id: u32,
    state: Arc<Mutex<MockState>>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let session_id = format!("mock-session-{client_id}-{}", now_ms());
    let token = format!("mock-token-{client_id}-{}", now_ms());
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::channel::<Message>(256);

    // 建链第一帧必须是 1201，客户端只等这一条
    let hello = notify(
        T_HANDSHAKE_ACK,
        SRC_GATEWAY,
        json!({
            "client_id": client_id,
            "session_id": session_id,
            "bulkdata_bind_token": token,
            "control_path": "/control",
            "bulkdata_path": "/bulkdata",
            "bind_timeout_ms": 30_000,
            "type": "ZSL-2-Ultra(MOCK)"
        }),
    );
    if sink.send(hello).await.is_err() {
        return;
    }
    info!("假本体: control 已建立 client_id={client_id}（{peer}）");

    // 写循环。发出的帧同样打出来（与客户端侧 `← 智航完整帧` 是同一条报文的两端），
    // 1Hz 的 1315 和 500ms 的 5009 太吵，只在 TRACE 打。
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Message::Text(text) = &msg {
                let high_freq = ["\"type\":1315", "\"type\":5009"]
                    .iter()
                    .any(|needle| text.contains(needle));
                if high_freq {
                    tracing::trace!("假本体: → {text}");
                } else {
                    debug!("假本体: → {text}");
                }
            }
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // 状态推送：1315 每 1s、5009 每 500ms、1207 建链后立即一条
    let ticker_tx = tx.clone();
    let ticker_state = state.clone();
    let ticker = tokio::spawn(async move {
        let module = notify(
            T_MODULE_NOTIFY,
            SRC_GATEWAY,
            json!({ "modules": [
                { "name": "sdk_node", "available": true },
                { "name": "slam", "available": true },
                { "name": "planner", "available": true }
            ]}),
        );
        if ticker_tx.send(module).await.is_err() {
            return;
        }
        let mut ticker = tokio::time::interval(SNAPSHOT_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut tick: u32 = 0;
        loop {
            ticker.tick().await;
            tick += 1;
            let (snapshot, status) = {
                let mut guard = ticker_state.lock().await;
                guard.tick();
                let snapshot = guard.snapshot();
                let status = if tick.is_multiple_of(STATUS_INTERVAL_TICKS) {
                    Some(guard.robot_status())
                } else {
                    None
                };
                (snapshot, status)
            };
            if ticker_tx
                .send(notify(T_TASK_STATUS, SRC_ALG, snapshot))
                .await
                .is_err()
            {
                break;
            }
            if let Some(status) = status
                && ticker_tx
                    .send(notify(T_ROBOT_STATUS, SRC_MACHINE, status))
                    .await
                    .is_err()
            {
                break;
            }
        }
    });

    // 读循环
    let mut heartbeats: u64 = 0;
    let mut teleop_frames: u64 = 0;
    while let Some(frame) = stream.next().await {
        let Ok(frame) = frame else { break };
        let Some(text) = frame.to_text().ok() else {
            continue;
        };
        let Ok(env) = serde_json::from_str::<Envelope>(text) else {
            warn!("假本体: 收到无法解析的报文: {text}");
            continue;
        };
        let req_type = env.head.msg_type;
        let msg_id = env.head.msg_id;
        // 心跳 / 摇杆太吵，只按次数打，其余报文一律原样打出来
        match req_type {
            T_HEARTBEAT => {
                heartbeats += 1;
                if heartbeats % 30 == 1 {
                    debug!("假本体: ← 1300 心跳（第 {heartbeats} 帧）");
                }
                // 1301 按文档只带 type/time/src，**不回显 msg_id**：回显会撞上
                // 客户端正在等 5001 的号段，把 1301 误投给任务调用方。
                let _ = tx
                    .send(notify(T_HEARTBEAT_ACK, SRC_GATEWAY, json!({})))
                    .await;
                continue;
            }
            T_REMOTE_CONTROL => {
                teleop_frames += 1;
                if teleop_frames % 50 == 1 {
                    debug!("假本体: ← 1304 摇杆（第 {teleop_frames} 帧）{}", env.data);
                }
                continue;
            }
            _ => {
                info!("假本体: ← {req_type} {text}");
            }
        }

        let mut guard = state.lock().await;
        match req_type {
            T_HANDSHAKE => {
                let _ = tx
                    .send(notify(
                        T_HANDSHAKE_ACK,
                        SRC_GATEWAY,
                        json!({
                            "client_id": client_id,
                            "session_id": guard.boot_id,
                            "bulkdata_bind_token": "mock-rebind",
                            "control_path": "/control",
                            "bulkdata_path": "/bulkdata"
                        }),
                    ))
                    .await;
            }
            T_TAKE_CONTROL => {
                guard.owner = Some(client_id);
                guard.ctrl_source = 1;
                // 1205 是广播，客户端按 client_id 判定「是不是自己拿到了」
                let _ = tx
                    .send(notify(
                        T_TAKE_CONTROL_NOTIFY,
                        SRC_GATEWAY,
                        json!({ "status_code": 0, "client_id": client_id, "message": "ok" }),
                    ))
                    .await;
                let _ = tx
                    .send(notify(
                        T_ROAMER_CONTROL_NOTIFY,
                        SRC_GATEWAY,
                        json!({ "client_id": client_id }),
                    ))
                    .await;
            }
            T_COMMAND => {
                // 1302：固件实测不回 1303，这里按文档回一条不带 msg_id 的受理通知
                // （客户端的 1302 走 send，不等人，所以这条只是日志价值）
                let cmd = env.data.get("cmd").and_then(Value::as_u64).unwrap_or(0);
                if cmd == 62 {
                    guard.mode = "remote".to_string();
                }
                let _ = tx
                    .send(notify(
                        T_COMMAND_ACK,
                        SRC_MACHINE,
                        json!({ "status_code": 0, "message": "ok", "cmd": cmd }),
                    ))
                    .await;
            }
            T_MAP_LIST => {
                let list = guard.map_list();
                let _ = tx.send(response(T_MAP_LIST, msg_id, list)).await;
            }
            T_LOCATE => {
                guard.loc_status = 100;
                let _ = tx.send(ok_ack(T_LOCATE, msg_id)).await;
            }
            T_SET_AVOID_LEVEL => {
                let level = env.data.get("level").and_then(Value::as_u64).unwrap_or(40);
                let _ = tx
                    .send(response(
                        T_SET_AVOID_LEVEL,
                        msg_id,
                        json!({ "code": 0, "msg": "ok", "effective_level": level, "reason": "" }),
                    ))
                    .await;
            }
            T_TASK_CREATE => {
                let kind = env.data.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32;
                // 真机上一条任务没结束就再下发会回 13897（capability busy）
                if guard.current.as_ref().is_some_and(MockTask::is_running) {
                    let _ = tx
                        .send(response(
                            T_TASK_CREATE,
                            msg_id,
                            json!({ "code": 13897, "msg": "resource conflict (capability busy)" }),
                        ))
                        .await;
                    debug!("假本体: ← 5000 被拒（能力忙）");
                } else {
                    let (task_type, stage) = match kind {
                        1 => ("mapping", "mapping.running"),
                        3 => ("recharge", "recharge.charging"),
                        5 => ("undock", "undock.driving"),
                        _ => ("navigation", "navigation.navigating"),
                    };
                    let id = guard.next_task_id("mock");
                    let name = env
                        .data
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("mock task")
                        .to_string();
                    // 真机一段任务可以带多个 goals（`params.goals` 有序数组），
                    // 逐点到点发 `navigation.goal_arrived`；假本体要照着模拟，
                    // 否则本地联调看不到多航点进度。
                    let total = env
                        .data
                        .get("params")
                        .and_then(|p| p.get("goals"))
                        .and_then(Value::as_array)
                        .map(|goals| goals.len().max(1))
                        .unwrap_or(1) as i64;
                    let goal_step = guard.step;
                    let started_at = tokio::time::Instant::now();
                    let ends_at = if kind == 1 {
                        None
                    } else {
                        Some(started_at + goal_step * total as u32)
                    };
                    guard.current = Some(MockTask {
                        id: id.clone(),
                        name,
                        task_type,
                        kind,
                        lifecycle: "running".to_string(),
                        stage: stage.to_string(),
                        progress: 0.0,
                        index: 0,
                        total,
                        event_seq: 1,
                        ends_at,
                        started_at,
                        goal_step,
                    });
                    let _ = tx
                        .send(response(
                            T_TASK_CREATE,
                            msg_id,
                            json!({ "code": 0, "msg": "ok", "task_id": id }),
                        ))
                        .await;
                }
            }
            T_TASK_CANCEL => {
                let cancelled = guard.current.take();
                let _ = tx.send(ok_ack(T_TASK_CANCEL, msg_id)).await;
                match cancelled {
                    Some(mut task) if task.kind == 1 => {
                        // 建图取消后真机会回 4001 保存结果，客户端据此拿目录名
                        let dir = format!("mock_map_{}", now_ms());
                        guard.map_dirs.insert(0, dir.clone());
                        task.lifecycle = "cancelled".to_string();
                        task.stage = "mapping.saved".to_string();
                        task.progress = 100.0;
                        let _ = tx
                            .send(notify(
                                T_MAP_SAVED,
                                SRC_ALG,
                                json!({
                                    "code": 0,
                                    "msg": "ok",
                                    "map_path": format!("{MAP_ROOT}/history_map/{dir}"),
                                    "map_name": dir
                                }),
                            ))
                            .await;
                    }
                    Some(mut task) => {
                        task.lifecycle = "cancelled".to_string();
                        task.stage = "navigation.cancelled".to_string();
                        task.ends_at = None;
                        guard.terminals.push_front(task);
                        while guard.terminals.len() > 3 {
                            guard.terminals.pop_back();
                        }
                    }
                    None => {}
                }
            }
            T_TASK_PAUSE => {
                if let Some(task) = guard.current.as_mut() {
                    task.lifecycle = "paused".to_string();
                    task.stage = "navigation.paused".to_string();
                }
                let _ = tx.send(ok_ack(T_TASK_PAUSE, msg_id)).await;
            }
            T_TASK_RESUME => {
                let step = guard.step;
                if let Some(task) = guard.current.as_mut() {
                    task.lifecycle = "running".to_string();
                    task.stage = "navigation.navigating".to_string();
                    task.ends_at = Some(tokio::time::Instant::now() + step);
                }
                let _ = tx.send(ok_ack(T_TASK_RESUME, msg_id)).await;
            }
            T_RETURN_TO_CHARGE => {
                let id = guard.next_task_id("mock-rc");
                guard.current = Some(MockTask {
                    id: id.clone(),
                    name: "返航充电".to_string(),
                    task_type: "return_to_charge",
                    kind: 3,
                    lifecycle: "running".to_string(),
                    stage: "return_to_charge.navigating".to_string(),
                    progress: 0.0,
                    index: 0,
                    total: 1,
                    event_seq: 1,
                    ends_at: Some(tokio::time::Instant::now() + guard.step),
                    started_at: tokio::time::Instant::now(),
                    goal_step: guard.step,
                });
                let _ = tx
                    .send(response(
                        T_RETURN_TO_CHARGE,
                        msg_id,
                        json!({ "code": 0, "msg": "ok", "task_id": id }),
                    ))
                    .await;
            }
            other => {
                warn!("假本体: 未实现的请求 {other}，回一条 ok 兜底");
                if msg_id.is_some() {
                    let _ = tx.send(ok_ack(other, msg_id)).await;
                }
            }
        }
    }

    info!("假本体: control 断开 client_id={client_id}（{peer}）");
    ticker.abort();
    drop(tx);
    let _ = writer.await;
}

/// bulkdata 通道：首帧 1202 → 回 1203，之后只保持连接
async fn run_bulkdata<S>(ws: tokio_tungstenite::WebSocketStream<S>, peer: String)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (mut sink, mut stream) = ws.split();
    let Some(Ok(first)) = stream.next().await else {
        return;
    };
    let Some(text) = first.to_text().ok() else {
        return;
    };
    let Ok(env) = serde_json::from_str::<Envelope>(text) else {
        return;
    };
    if env.head.msg_type != T_BIND_BULKDATA {
        warn!("假本体: bulkdata 首帧不是 1202，而是 {}", env.head.msg_type);
        return;
    }
    debug!("假本体: ← 1202 绑定请求 {}", env.data);
    if sink
        .send(response(
            T_BIND_BULKDATA,
            env.head.msg_id,
            json!({ "code": 0, "msg": "ok", "channel": "bulkdata" }),
        ))
        .await
        .is_err()
    {
        return;
    }
    info!("假本体: bulkdata 已绑定（{peer}）");
    // 真机的 1401/1403/1405/1407 是可选推送，这里只保持连接
    while let Some(frame) = stream.next().await {
        if frame.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> MockState {
        MockState::new(
            Duration::from_millis(1500),
            vec![DEFAULT_MAP_DIR.to_string()],
            true,
        )
    }

    #[test]
    fn mock_alias_matches_only_dev_values() {
        assert!(is_mock_alias("mock"));
        assert!(is_mock_alias(" MOCK "));
        assert!(!is_mock_alias("192.168.234.234"));
        assert!(!is_mock_alias("sim"));
    }

    #[test]
    fn map_list_uses_body_directory_names() {
        let list = state().map_list();
        let items = list["map_path_list"].as_array().unwrap();
        assert_eq!(
            items[0]["path"],
            json!(format!("{MAP_ROOT}/history_map/{DEFAULT_MAP_DIR}"))
        );
        assert_eq!(items[0]["charge_point_exist"], json!(true));
    }

    #[test]
    fn snapshot_reports_localized_and_idle() {
        let mut st = state();
        let frame = st.snapshot();
        assert_eq!(frame["schema_version"], json!(2));
        assert_eq!(frame["robot"]["loc_state"]["status"], json!(100));
        assert_eq!(frame["current_task"], Value::Null);
    }

    #[test]
    fn task_finishes_after_step() {
        let mut st = state();
        st.step = Duration::from_millis(0);
        st.current = Some(MockTask {
            id: "mock-1".to_string(),
            name: "前往 航点".to_string(),
            task_type: "navigation",
            kind: 2,
            lifecycle: "running".to_string(),
            stage: "navigation.navigating".to_string(),
            progress: 0.0,
            index: 0,
            total: 1,
            event_seq: 1,
            ends_at: Some(tokio::time::Instant::now()),
            started_at: tokio::time::Instant::now(),
            goal_step: Duration::from_millis(0),
        });
        st.tick();
        assert!(st.current.is_none());
        assert_eq!(st.terminals[0].lifecycle, "success");
        assert_eq!(st.terminals[0].stage, "navigation.completed");
    }

    /// 一段任务带多个 goals 时，假本体要逐个到点（真机发 `navigation.goal_arrived`）。
    #[test]
    fn multi_goal_task_advances_goal_by_goal() {
        let mut st = state();
        st.current = Some(MockTask {
            id: "mock-5".to_string(),
            name: "5 个航点".to_string(),
            task_type: "navigation",
            kind: 2,
            lifecycle: "running".to_string(),
            stage: "navigation.navigating".to_string(),
            progress: 0.0,
            index: 0,
            total: 5,
            event_seq: 1,
            ends_at: Some(tokio::time::Instant::now() + Duration::from_secs(10)),
            started_at: tokio::time::Instant::now() - Duration::from_secs(2),
            goal_step: Duration::from_secs(1),
        });
        st.tick();
        let task = st.current.as_ref().expect("任务还在跑");
        assert_eq!(task.index, 2, "2s / 每点 1s → 已到第 2 点");
        assert_eq!(task.stage, "navigation.goal_arrived");
        assert!((task.progress - 40.0).abs() < 0.01, "2/5 = 40%: {}", task.progress);
        assert!(task.is_running());
    }

    #[test]
    fn responses_pair_on_msg_id() {
        let Message::Text(text) = response(T_MAP_LIST, Some(7), json!({"code": 0})) else {
            panic!("response must be text");
        };
        let env: Envelope = serde_json::from_str(&text).unwrap();
        assert_eq!(env.head.msg_type, T_MAP_LIST_ACK);
        assert_eq!(env.head.msg_id, Some(7));
        assert!(env.is_ok());
    }
}
