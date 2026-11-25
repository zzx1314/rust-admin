//! 网页对讲：浏览器 ⇄ 板端音频中继（经设备客户端）
//!
//! 板端 `voice_call` 只会跟**同网段的 UDP 对端**说话（16kHz/S16/20ms 裸 PCM），
//! 公网浏览器既发不了 UDP，板子在 4G 后面也没有入站端口。所以让设备客户端
//! （与板端同机、`127.0.0.1` 一定通）当那个「对端」，音频借用既有的设备 WS 上行：
//!
//! ```text
//! 浏览器 ──WS /api/devices/{id}/audio/ws──▶ 平台（本文件）──device WS──▶ 客户端
//!         ◀──── 二进制 PCM 帧 ──────────          ◀── phone_audio ──
//!                                              客户端 ──UDP 127.0.0.1:5000──▶ 板端
//! ```
//!
//! 三条硬规则（改这块先看）：
//!
//! 1. **一次只允许一路**：板端 FIFO / UDP 都是单通道，两个人同时打只会互相踩。
//!    后来者接管，先来的那条连接收到「已被接管」并挂断。
//! 2. **只转音频，不做混音 / 存储**：WS 上就是裸 PCM（浏览器侧二进制帧，设备侧 base64
//!    装在 JSON 里 —— 设备 WS 是纯文本协议）。平台留不下任何通话内容。
//! 3. **异常必须收尾**：浏览器关页 / 断网 / 被接管 / 超时，都要给客户端发
//!    `phone_stream stop`，否则板子一直停在 phone 模式（唤醒词失效、还占着 FIFO）。

use crate::app::AppState;
use crate::auth::Claims;
use crate::error::AppError;
use crate::gateway;
use crate::models::Device;
use axum::Extension;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, oneshot};
use tracing::{debug, info, warn};

/// 板端没在 `PHONE_START_TIMEOUT` 内报到（切 phone 模式要先 `ensure_asr`，实测量秒级）
const PHONE_START_TIMEOUT: Duration = Duration::from_secs(10);

/// 广播队列长度：50 帧/秒，256 ≈ 5 秒——浏览器慢到这种程度就该被踢了
const EVENT_QUEUE: usize = 256;

/// 设备上行的一帧：要么是音频字节，要么是状态（started / error / stopped）
#[derive(Debug, Clone)]
pub enum PhoneEvent {
    /// 裸 PCM（16kHz / 单声道 / S16_LE），按 20ms 一块
    Audio(Vec<u8>),
    /// 客户端的通话状态（含错误原因）
    State(Value),
}

/// 对讲中继的房间表（进程一份）
///
/// 一条浏览器连接 = 一个会话；同一设备同一时刻只允许一个会话。
pub struct PhoneAudioHub {
    /// 设备码 →(会话 id, 「请挂断」通道)
    sessions: Mutex<HashMap<String, (u64, oneshot::Sender<()>)>>,
    /// 设备上行广播：`(设备码, 事件)`，每个浏览器会话按自己的设备码过滤
    events: broadcast::Sender<(String, PhoneEvent)>,
    seq: AtomicU64,
}

impl Default for PhoneAudioHub {
    fn default() -> Self {
        let (events, _) = broadcast::channel(EVENT_QUEUE);
        Self {
            sessions: Mutex::new(HashMap::new()),
            events,
            seq: AtomicU64::new(1),
        }
    }
}

impl PhoneAudioHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, PhoneEvent)> {
        self.events.subscribe()
    }

    /// 设备上行的一帧音频（`{"data": "<base64>"}`，客户端已把 12 字节 RTP 头剥掉）
    pub fn publish_audio(&self, device: &str, payload: &Value) {
        let Some(data) = payload.get("data").and_then(Value::as_str) else {
            warn!("对讲：{device} 的 phone_audio 没有 data 字段，丢弃");
            return;
        };
        match STANDARD.decode(data) {
            Ok(bytes) if !bytes.is_empty() => {
                // 没有观看者时 send 会返回 Err（无人订阅），静默丢掉即可
                let _ = self
                    .events
                    .send((device.to_string(), PhoneEvent::Audio(bytes)));
            }
            Ok(_) => {}
            Err(error) => warn!("对讲：{device} 的 phone_audio 不是合法 base64（{error}）"),
        }
    }

    /// 设备上报的通话状态（`phone_stream {state: started|error|stopped, ...}`）
    pub fn publish_state(&self, device: &str, payload: &Value) {
        let state = payload.get("state").and_then(Value::as_str).unwrap_or("?");
        debug!("对讲：{device} 状态 → {state}");
        let body = json!({
            "type": "phone_state",
            "device": device,
            "state": state,
            "error": payload.get("error"),
            "detail": payload.get("detail"),
        });
        let _ = self
            .events
            .send((device.to_string(), PhoneEvent::State(body)));
    }

    /// 登记本次会话；已有会话先请它挂断（一次只允许一路）
    async fn attach(&self, device: &str) -> (u64, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        let id = self.seq.fetch_add(1, Ordering::Relaxed);
        let mut sessions = self.sessions.lock().await;
        if let Some((_, previous)) = sessions.insert(device.to_string(), (id, tx)) {
            let _ = previous.send(());
        }
        (id, rx)
    }

    /// 摘掉**自己这条**会话（被接管时不要误删后来者的登记）
    async fn detach(&self, device: &str, id: u64) {
        let mut sessions = self.sessions.lock().await;
        if sessions
            .get(device)
            .is_some_and(|(current, _)| *current == id)
        {
            sessions.remove(device);
        }
    }

    /// 当前有没有人在打（诊断用）
    pub async fn active(&self, device: &str) -> bool {
        self.sessions.lock().await.contains_key(device)
    }
}

/// `GET /api/devices/{id}/audio/ws?token=…` —— 浏览器对讲通道
///
/// 鉴权走中间件的 `?token=`（浏览器 WS 带不上 `Authorization` 头），
/// 连上之后：平台给设备客户端下发 `phone_stream start`，客户端把板端切到 phone
/// 模式并起本地 UDP 对端；接下来的帧就在这条 WS 与设备 WS 之间来回搬。
pub async fn browser_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let device = crate::handlers::devices::fetch_device(&state, id).await?;
    crate::auth::ensure_device_access(&state, &claims, &device.device_id).await?;
    if !gateway::is_device_connected(&state.device_connections, &device.device_id).await {
        return Err(AppError::BadRequest(
            "设备客户端未连接：对讲要靠它把音频从板子带上来".to_string(),
        ));
    }
    Ok(ws.on_upgrade(move |socket| handle_call(socket, state, device)))
}

async fn handle_call(socket: WebSocket, state: AppState, device: Device) {
    let code = device.device_id.clone();
    let hub: Arc<PhoneAudioHub> = state.phone_audio.clone();
    let (mut sink, mut incoming) = socket.split();

    let (session_id, mut kicked) = hub.attach(&code).await;
    let mut events = hub.subscribe();
    info!("对讲开始：{code}（会话 {session_id}）");

    // 让客户端起本地对端 + 把板端切到 phone 模式
    let started = gateway::send_to_device(
        &state.device_connections,
        &code,
        "phone_stream",
        json!({ "action": "start", "codec": "pcm16", "rate": 16000 }),
    )
    .await;
    if !started {
        let _ = send_state(&mut sink, "error", Some("设备客户端未连接")).await;
        hub.detach(&code, session_id).await;
        return;
    }

    // 等客户端报到（切 phone 模式要先确保 asr 在跑，实测量秒级）
    let deadline = tokio::time::Instant::now() + PHONE_START_TIMEOUT;
    let mut ready = false;
    let mut reason = "浏览器断开";

    loop {
        tokio::select! {
            // 浏览器 → 设备（上行音频）
            msg = incoming.next() => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    if !bytes.is_empty() && !uplink(&state, &code, &bytes).await {
                        reason = "设备连接已断";
                        break;
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    // 浏览器只发心跳类的文本，回一句 pong 让页面知道链路是活的
                    if text.contains("\"ping\"") {
                        let _ = sink
                            .send(Message::Text(json!({"type":"pong"}).to_string().into()))
                            .await;
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(error)) => {
                    debug!("对讲 WS 出错：{error}");
                    break;
                }
                _ => {}
            },
            // 设备 → 浏览器（下行音频 / 状态）
            event = events.recv() => match event {
                Ok((device, PhoneEvent::Audio(bytes))) if device == code => {
                    if sink.send(Message::Binary(bytes.into())).await.is_err() {
                        break;
                    }
                }
                Ok((device, PhoneEvent::State(value))) if device == code => {
                    match value.get("state").and_then(Value::as_str) {
                        Some("started") => ready = true,
                        Some("error") | Some("stopped") => {
                            let _ = sink.send(Message::Text(value.to_string().into())).await;
                            reason = "板端结束通话";
                            break;
                        }
                        _ => {}
                    }
                    if sink.send(Message::Text(value.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    warn!("对讲音频积压丢弃 {missed} 帧（浏览器太慢）");
                }
                Err(_) => break,
            },
            // 被同一设备的另一条会话接管
            _ = &mut kicked => {
                let _ = send_state(&mut sink, "error", Some("另一个页面接管了这次通话")).await;
                reason = "被新会话接管";
                break;
            }
            // 迟迟不报到：别让页面一直停在「连接中」
            _ = tokio::time::sleep_until(deadline), if !ready => {
                let _ = send_state(&mut sink, "error", Some("板端没有响应（asr 是否在跑？）")).await;
                reason = "板端起流超时";
                break;
            }
        }
    }

    // 收尾：客户端负责停 UDP 对端 + `phone stop`，它自己也会兜（这里只是显式收口）
    gateway::send_to_device(
        &state.device_connections,
        &code,
        "phone_stream",
        json!({ "action": "stop", "reason": reason }),
    )
    .await;
    hub.detach(&code, session_id).await;
    info!("对讲结束：{code}（{reason}）");
}

/// 浏览器上行一帧 → 设备指令；设备掉线时返回 false（调用方收尾）
async fn uplink(state: &AppState, device: &str, bytes: &[u8]) -> bool {
    gateway::send_to_device(
        &state.device_connections,
        device,
        "phone_audio",
        json!({ "data": STANDARD.encode(bytes) }),
    )
    .await
}

/// 给浏览器回一条状态（`{"type":"phone_state","state":…}`）
async fn send_state<S>(sink: &mut S, state: &str, error: Option<&str>) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
{
    let body = json!({ "type": "phone_state", "state": state, "error": error });
    sink.send(Message::Text(body.to_string().into()))
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_session_kicks_the_first_one() {
        let hub = PhoneAudioHub::new();
        let (first_id, mut first) = hub.attach("RD-001").await;
        assert!(hub.active("RD-001").await);
        let (second_id, _second) = hub.attach("RD-001").await;
        // 先来的那条应该被叫走
        assert!(first.try_recv().is_ok(), "后来的会话要接管并踢掉前一条");
        // 旧会话收尾时不能把新会话的登记删掉
        hub.detach("RD-001", first_id).await;
        assert!(hub.active("RD-001").await);
        hub.detach("RD-001", second_id).await;
        assert!(!hub.active("RD-001").await);
    }

    #[tokio::test]
    async fn audio_is_forwarded_only_to_the_matching_device() {
        let hub = PhoneAudioHub::new();
        let mut events = hub.subscribe();
        let pcm = STANDARD.encode([1u8, 2, 3, 4]);
        hub.publish_audio("RD-002", &json!({"data": pcm}));
        let (device, event) = events.recv().await.expect("应收到音频");
        assert_eq!(device, "RD-002");
        match event {
            PhoneEvent::Audio(bytes) => assert_eq!(bytes, vec![1, 2, 3, 4]),
            other => panic!("应是一帧音频：{other:?}"),
        }
        // 坏 base64 不能把进程搞挂（只记一条 warn）
        hub.publish_audio("RD-002", &json!({"data": "not-base64!!"}));
    }

    #[tokio::test]
    async fn state_messages_are_tagged_for_the_page() {
        let hub = PhoneAudioHub::new();
        let mut events = hub.subscribe();
        hub.publish_state("RD-003", &json!({"state": "started"}));
        let (device, event) = events.recv().await.expect("应收到状态");
        assert_eq!(device, "RD-003");
        match event {
            PhoneEvent::State(value) => {
                assert_eq!(value["type"], "phone_state");
                assert_eq!(value["state"], "started");
                assert_eq!(value["device"], "RD-003");
            }
            other => panic!("应是状态消息：{other:?}"),
        }
    }
}
