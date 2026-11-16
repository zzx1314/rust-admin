//! RTSP 实时视频中继（HTTP-FLV over WebSocket）
//!
//! 智航 Pro 协议没有视频接口，本体只对外提供 RTSP；浏览器不能直连 RTSP，
//! 所以由**能访问内网的那一端**把流转成 FLV、主动推到平台，浏览器只跟平台同源通信：
//!
//! ```text
//! 机器狗本体 RTSP ──ffmpeg──► 设备客户端 ──ws://平台/ws/device/video──► 业务平台 ──ws──► 浏览器(flv.js)
//!  rtsp://…:8554/test          转封装 FLV            二进制帧中继              MSE 播放
//! 背包本地 RTSP   ──ffmpeg──► 板端 relay   ──ws://平台/ws/device/video──┘
//! ```
//!
//! 四条约定：
//!
//! 1. **按需推流**：第一个观看者进来才给推流端下发 `video_start`，最后一个离开立刻
//!    `video_stop`，没人看的时候现场侧不耗 CPU / 带宽。
//! 2. **二进制专用连接**：浏览器这条 WS 只发 FLV 字节，绝不发文本帧 ——
//!    flv.js 的 `WebSocketLoader` 收到文本帧会直接报错。
//! 3. **地址唯一来源**（设备客户端）：拉流地址就是设备档案里的 `video_url`，
//!    运营在「设备管理」里改一次即可，不需要再动客户端配置。
//! 4. **两条控制通道**：设备客户端听设备网关上的 `video_start`；背包板端没有设备台账，
//!    房间名用 [`BACKPACK_ROOM_PREFIX`] 前缀，控制消息直接走它自己那条推流连接
//!    （文本帧），见 `handle_publisher` 里的 `controls`。
//!
//! 为什么背包也要走中继：现场入口（浏览器直连板端）必须「操作员电脑在同一个内网且
//! 过了 Chrome 的 Local Network Access 许可」，换 WiFi / 远程 / 换浏览器就黑屏。
//! 推流中继只要求**板子能出公网**，浏览器与平台同源，延时 1~2s。

use crate::app::AppState;
use crate::auth::{self, Claims};
use crate::error::AppError;
use crate::gateway;
use crate::gateway_types::WsMessage;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Path, Query, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{debug, info, warn};

/// 单个观看者的积压帧上限：网络慢时丢帧保实时，不把服务端内存拖爆
const VIEWER_BUFFER_FRAMES: usize = 128;

/// 流头（FLV header + AVC 序列头 + 首个关键帧）解析的上限
///
/// 1200kbps 下 4MB ≈ 26 秒，正常 2 秒内（一个 GOP）就拿到关键帧；
/// 超出说明推上来的不是 FLV/H.264，直接放弃补头，避免内存被吃满。
const MAX_INIT_BYTES: usize = 4 * 1024 * 1024;

/// 推流会话自增号（见 `VideoHub::publisher_attach`）
static PUBLISHER_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 背包板端推流房间的房间名前缀（`bp:<stream>`）
///
/// 设备客户端的房间名就是 `devices.device_id`；板端推流服务没有设备台账，用这个
/// 前缀区分开：带前缀的房间在 `handle_publisher` 里会挂上**控制通道**，
/// 控制消息直接写进它自己的推流连接，不经过设备网关。
pub const BACKPACK_ROOM_PREFIX: &str = "bp:";

/// 允许通过中继观看的背包通道（与板端 mediamtx 的 path 一致）
///
/// 白名单而不是透传：房间名来自查询串，不限制就能被拿来刷任意 key 撑爆内存。
const BACKPACK_STREAMS: [&str; 6] = ["cam0", "cam1", "cam2", "cam3", "dogcam", "dog-ipcam"];

/// 推流端控制通道表：`room -> (会话号, 文本帧发送端)`
type ControlChannels = Arc<RwLock<HashMap<String, (u64, mpsc::Sender<String>)>>>;

/// 视频中继房间：一台设备一个房间
///
/// 生产者只有一个（该设备的客户端推流连接），消费者是任意多个浏览器。
#[derive(Clone, Default)]
pub struct VideoHub {
    rooms: Arc<RwLock<HashMap<String, Room>>>,
    /// 观看者计数，**独立于房间**存在
    ///
    /// 房间会随推流端断连被丢弃（ffmpeg 抖动时很常见），如果计数跟着房间走，
    /// 最后一个观看者离开时就数不到人，`video_stop` 永远发不出去 —— 客户端会
    /// 一直重连推流、白白编码。
    viewers: Arc<RwLock<HashMap<String, usize>>>,
    /// 推流端控制通道：只有板端推流房间有（`bp:` 前缀）
    ///
    /// 值里带会话号：重连后的新连接会覆盖旧的，旧连接迟到的清理
    /// 只删自己那一份，见 `unregister_control`。
    controls: ControlChannels,
    /// 已经下发过 `video_start`、但可能没人看的房间（看门狗只盯这些）
    ///
    /// 推流端是常驻连接（板端 relay 六条），不能因为它们「有人推没人看」就每 15s
    /// 发一次 `video_stop`：那样日志会刷屏，而且真正在看的那一路也会被误杀。
    streaming: Arc<RwLock<HashSet<String>>>,
}

struct Room {
    tx: broadcast::Sender<Arc<[u8]>>,
    /// 可补发给「中途加入」的观看者的流头：FLV header + AVC 序列头 + 首个关键帧
    ///
    /// 推流是持续的，后打开页面的浏览器只能收到中途的字节；没有这个头，
    /// flv.js 连 `FLV` 魔数和 AVC 配置都看不到，直接黑屏。
    init: Option<Arc<[u8]>>,
    /// 流头解析完成前先缓存，避免把不完整的字节发给观看者
    pending: Vec<u8>,
    /// 推流会话标识：`Some(id)` 表示在线，`id` 用来区分「新连接」与「旧连接迟到的清理」
    publisher: Option<u64>,
}

fn new_room(session: u64) -> Room {
    Room {
        tx: broadcast::channel(VIEWER_BUFFER_FRAMES).0,
        init: None,
        pending: Vec::new(),
        publisher: Some(session),
    }
}

impl VideoHub {
    /// 观看者加入：返回订阅端、是否「第一个观看者」、以及可补发的流头
    pub async fn viewer_join(
        &self,
        device_id: &str,
    ) -> (broadcast::Receiver<Arc<[u8]>>, bool, Option<Arc<[u8]>>) {
        let mut viewers = self.viewers.write().await;
        let count = viewers.entry(device_id.to_string()).or_insert(0);
        *count += 1;
        let first = *count == 1;
        drop(viewers);

        let mut rooms = self.rooms.write().await;
        let room = rooms.entry(device_id.to_string()).or_insert_with(|| Room {
            tx: broadcast::channel(VIEWER_BUFFER_FRAMES).0,
            init: None,
            pending: Vec::new(),
            publisher: None,
        });
        let init = room.init.clone();
        (room.tx.subscribe(), first, init)
    }

    /// 观看者离开：返回是否是「最后一个观看者」
    ///
    /// 没人看了就把**空房间**清掉；但推流端还挂着时不丢（板端 relay 是常驻连接，
    /// 丢了房间等于把它的注册一起丢了：`has_publisher` 变 false，下一次观看会被
    /// 误判成「推流服务未连接」）。
    pub async fn viewer_leave(&self, device_id: &str) -> bool {
        let last = {
            let mut viewers = self.viewers.write().await;
            match viewers.get_mut(device_id) {
                Some(count) => {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        viewers.remove(device_id);
                        true
                    } else {
                        false
                    }
                }
                None => false,
            }
        };
        if last {
            let mut rooms = self.rooms.write().await;
            let no_publisher = rooms
                .get(device_id)
                .map(|room| room.publisher.is_none())
                .unwrap_or(true);
            if no_publisher {
                rooms.remove(device_id);
            }
        }
        last
    }

    /// 客户端推来的 FLV 分片：没有观看者就丢掉（没人看不必占内存）
    ///
    /// 流头解析完成之前先缓存：这样第一个观看者拿到的字节从 `FLV` 魔数开始，
    /// 不会出现「只收到半截流」的黑屏。
    pub async fn publish(&self, device_id: &str, frame: Arc<[u8]>) {
        let mut rooms = self.rooms.write().await;
        let Some(room) = rooms.get_mut(device_id) else {
            return;
        };
        if room.init.is_some() {
            // 接收端全掉线时 send 报错，忽略即可；下游会自行关闭连接
            let _ = room.tx.send(frame);
            return;
        }

        room.pending.extend_from_slice(&frame);
        match flv_init_len(&room.pending) {
            Some(len) => {
                let init: Arc<[u8]> = Arc::from(&room.pending[..len]);
                let rest: Arc<[u8]> = Arc::from(&room.pending[len..]);
                // 先补流头：先到的观看者是「流头还没解析出来就加入」的，
                // 他们的字节被缓存了，这里必须一次性补上，否则永远收不到 FLV 魔数
                let _ = room.tx.send(init.clone());
                room.init = Some(init);
                room.pending = Vec::new();
                if !rest.is_empty() {
                    let _ = room.tx.send(rest);
                }
            }
            None if room.pending.len() > MAX_INIT_BYTES => {
                warn!("device {device_id} video stream is not FLV/H.264, header capture abandoned");
                room.pending = Vec::new();
            }
            None => {}
        }
    }

    /// 推流端注册 / 重连
    ///
    /// 会先把旧房间整个扔掉：ffmpeg 重启后推的是**新的一条流**（新的 FLV 头、
    /// 新的时间戳），继续往老观看者的 socket 里灌会让他们卡在解析错误上。
    /// 丢掉房间 → 观看端 `recv` 返回 `Closed` → 前端断开后自动重连，拿到干净的流。
    pub async fn publisher_attach(&self, device_id: &str, session: u64) {
        let mut rooms = self.rooms.write().await;
        match rooms.get(device_id).map(|room| room.publisher.is_some()) {
            // 老推流端还挂着（ffmpeg 重连）：新旧两条流的 FLV 头/时间戳不能混在
            // 同一批观看者上，整个房间换掉，让浏览器重连拿干净的新流。
            Some(true) => {
                info!("device {device_id} video publisher reconnected, restarting room");
                rooms.insert(device_id.to_string(), new_room(session));
            }
            // 正常首帧流程：观看者先到（video_start 就是它触发的），推流端后到。
            // 这里**不能**换房间，否则刚进来的观看者立刻被踢掉，画面永远出不来。
            Some(false) => {
                if let Some(room) = rooms.get_mut(device_id) {
                    room.publisher = Some(session);
                    room.init = None;
                    room.pending = Vec::new();
                }
            }
            None => {
                rooms.insert(device_id.to_string(), new_room(session));
            }
        }
    }

    /// 「有人在推、但没人看」的设备：看门狗据此让推流端停掉 ffmpeg
    ///
    /// 只算**曾经被要求起流**的房间（[`VideoHub::mark_streaming`]）：板端 relay 的六条
    /// 连接平时一直挂着，不能让它们每 15s 被停一次。
    pub async fn idle_publishers(&self) -> Vec<String> {
        let rooms = self.rooms.read().await;
        let viewers = self.viewers.read().await;
        let streaming = self.streaming.read().await;
        streaming
            .iter()
            .filter(|device_id| {
                rooms
                    .get(*device_id)
                    .map(|room| room.publisher.is_some())
                    .unwrap_or(false)
            })
            .filter(|device_id| viewers.get(*device_id).copied().unwrap_or(0) == 0)
            .cloned()
            .collect()
    }

    /// 标记某房间是否已经让推流端起过流（看门狗只处理这些）
    pub async fn mark_streaming(&self, device_id: &str, streaming: bool) {
        let mut set = self.streaming.write().await;
        if streaming {
            set.insert(device_id.to_string());
        } else {
            set.remove(device_id);
        }
    }

    /// 客户端推流连接断开：清掉房间，让观看端的 `recv` 立刻返回 `Closed`
    ///
    /// 只在会话仍然匹配时清理：旧连接迟到的清理不能把新连接刚建好的房间删掉。
    pub async fn publisher_gone(&self, device_id: &str, session: u64) {
        let mut rooms = self.rooms.write().await;
        let stale = rooms
            .get(device_id)
            .map(|room| room.publisher != Some(session))
            .unwrap_or(false);
        if stale {
            return;
        }
        if rooms.remove(device_id).is_some() {
            info!("device {device_id} video publisher detached, room dropped");
        }
    }

    /// 登记推流端的控制通道（板端推流服务用）
    pub async fn register_control(&self, room: &str, session: u64, tx: mpsc::Sender<String>) {
        self.controls
            .write()
            .await
            .insert(room.to_string(), (session, tx));
    }

    /// 推流连接断开：只清理自己那条通道，旧连接迟到的清理不能删掉新连接的
    pub async fn unregister_control(&self, room: &str, session: u64) {
        let mut controls = self.controls.write().await;
        if let Some((owner, _)) = controls.get(room)
            && *owner == session
        {
            controls.remove(room);
        }
    }

    /// 给推流端发一条控制消息（文本帧）；没有通道（普通设备客户端）返回 false
    pub async fn send_control(&self, room: &str, message: String) -> bool {
        let controls = self.controls.read().await;
        match controls.get(room) {
            Some((_, tx)) => tx.send(message).await.is_ok(),
            None => false,
        }
    }

    /// 该房间当前是否有推流端在线（板端 relay 常驻连接，用来判断能不能看）
    pub async fn has_publisher(&self, room: &str) -> bool {
        self.rooms
            .read()
            .await
            .get(room)
            .map(|room| room.publisher.is_some())
            .unwrap_or(false)
    }

    /// 推流端开始一条**新流**（ffmpeg 重启）：清掉旧流头与缓存字节
    ///
    /// 板端 relay 是常驻推流连接，但每次起流都会换一条 FLV（新头、新时间戳）。
    /// 不清旧头的话，中途加入的观看者会拿到「上一轮的关键帧 + 这一轮的中段字节」，
    /// flv.js 解析错位后卡在 `readyState=0`（黑屏）。
    pub async fn reset_stream(&self, device_id: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(device_id) {
            room.init = None;
            room.pending = Vec::new();
        }
    }

    /// 当前观看者数量（日志 / 排障用）
    pub async fn viewers(&self, device_id: &str) -> usize {
        self.viewers
            .read()
            .await
            .get(device_id)
            .copied()
            .unwrap_or(0)
    }
}

/// 解析 FLV 流头长度：`FLV header(9) + PrevTagSize0(4) + AVC 序列头 + 首个关键帧`
///
/// 返回 `Some(字节数)` 表示从流开头到首个关键帧 tag 结束的完整前缀，
/// 中途加入的观看者补发这一段就能正常解码；数据还不完整（或不是 FLV）返回 `None`。
fn flv_init_len(buf: &[u8]) -> Option<usize> {
    // FLV header: "FLV" + version + flags，再跟 4 字节 PreviousTagSize0
    if buf.len() < 9 || &buf[..3] != b"FLV" {
        return None;
    }
    let mut pos = 13;
    let mut header_tag = false;
    loop {
        // tag header 固定 11 字节：type(1) + datasize(3) + timestamp(3) + exts(1) + streamid(3)
        if buf.len() < pos + 11 {
            return None;
        }
        let tag_type = buf[pos];
        let size = u32::from_be_bytes([0, buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
        let body = pos + 11;
        if buf.len() < body + size + 4 {
            return None;
        }
        if tag_type == 9 && size >= 2 {
            // 视频 tag: frameType(4bit) + codecId(4bit)，AVCPacketType 紧跟其后
            let frame_type = buf[body] >> 4;
            let codec = buf[body] & 0x0f;
            let packet_type = buf[body + 1];
            // codec 7 = AVC，packet_type 0 = 序列头（SPS/PPS）
            // 注意：序列头 tag 的 frameType 也是 1，不能在这里收尾，
            // 否则补出去的流里只有 SPS/PPS、没有 IDR，中途加入照样黑屏。
            if codec == 7 && packet_type == 0 {
                header_tag = true;
            } else if frame_type == 1 && (codec != 7 || header_tag) {
                // 关键帧 NALU（AVC 是 packet_type = 1）。AVC 必须已经拿到序列头，
                // 否则补出去的是没有 SPS/PPS 的裸 IDR，播放器照样解不出来。
                return Some(body + size + 4);
            }
        }
        pos = body + size + 4;
    }
}

/// 推流看门狗：兜住「有人在推、但已经没人看」的空转
///
/// 正常路径是最后一个观看者断开时立刻下发 `video_stop`；但下面这些竞态会让那条
/// 通知落空：观看者断线时推流端正好在重连（房间已经被丢弃，数不到观看者）、
/// 浏览器崩溃没走 Close 帧。10 分钟不看却一直在编码，机器狗电池撑不住。
pub fn spawn_watchdog(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            ticker.tick().await;
            for device_id in state.video_hub.idle_publishers().await {
                // 板端推流房间（`bp:`）的控制通道直连它的推流连接；设备客户端只听设备网关
                let stop = json!({"msgType": "video_stop", "payload": {}}).to_string();
                if state.video_hub.send_control(&device_id, stop).await {
                    state.video_hub.mark_streaming(&device_id, false).await;
                    info!("board relay {device_id} has no viewer, stopping stream");
                    continue;
                }
                state.video_hub.mark_streaming(&device_id, false).await;
                info!("device {device_id} has no viewer, stopping video stream");
                gateway::send_video_control(&state.device_connections, &device_id, None).await;
            }
        }
    });
}

/// 设备端推流通道：`GET /ws/device/video`
///
/// 首帧必须是文本 `{"msgType":"video_register","payload":{"deviceId":"…"}}`，
/// 之后全部是二进制 FLV 分片（客户端 ffmpeg 的 stdout 直传）。
pub async fn publisher_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_publisher(socket, state))
}

/// 推流端控制通道的下一条消息
///
/// 没有通道（普通设备客户端）就永久挂起，让 `select!` 只等推流数据；
/// 通道关闭返回 `None`，调用方会把 `controls` 置空后重新挂起，不会空转。
async fn next_control(controls: &mut Option<mpsc::Receiver<String>>) -> Option<String> {
    match controls {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

async fn handle_publisher(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let mut device_id: Option<String> = None;
    // 推流端存活探测：现场是 4G / 弱网 wifi，链路"半死"时 TCP 未必立刻报错，
    // 而平台会一直认为有推流在线（观看者进来永远等不到画面）。
    // 客户端每 20s 一个 WS ping，60s 一个消息都没有就判死。
    let mut last_seen = std::time::Instant::now();
    let mut liveness = tokio::time::interval(std::time::Duration::from_secs(15));
    // 会话标识：同一台设备重连时，旧连接迟到的清理不会误删新房间
    let session = PUBLISHER_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut frames: u64 = 0;
    // 只有板端推流房间（`bp:`）才拿控制通道：观看者的 `video_start` / `video_stop`
    // 通过它写到这条推流连接上（设备客户端的控制走设备网关）
    let mut controls: Option<mpsc::Receiver<String>> = None;

    loop {
        tokio::select! {
            _ = liveness.tick() => {
                if last_seen.elapsed() > std::time::Duration::from_secs(60) {
                    warn!(
                        "video publisher {:?} silent for {:?}, closing",
                        device_id,
                        last_seen.elapsed()
                    );
                    break;
                }
            }
            incoming = stream.next() => {
                let Some(msg) = incoming else { break };
                last_seen = std::time::Instant::now();
                match msg {
                    Ok(Message::Text(text)) => {
                        let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) else {
                            warn!("video publisher sent invalid JSON: {text}");
                            continue;
                        };
                        match ws_msg.msg_type.as_str() {
                            "video_register" => {
                                let code = ws_msg.payload["deviceId"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .trim()
                                    .to_string();
                                if code.is_empty() {
                                    let _ = sink
                                        .send(Message::Text(
                                            json!({"msgType":"error","payload":{"message":"video_register 缺少 deviceId"}})
                                                .to_string()
                                                .into(),
                                        ))
                                        .await;
                                    continue;
                                }
                                info!("device {code} video publisher attached");
                                state.video_hub.publisher_attach(&code, session).await;
                                if code.starts_with(BACKPACK_ROOM_PREFIX) {
                                    let (tx, rx) = mpsc::channel(8);
                                    state.video_hub.register_control(&code, session, tx).await;
                                    controls = Some(rx);
                                }
                                device_id = Some(code);
                            }
                            "video_reset" => {
                                // 常驻推流连接里的 ffmpeg 重启了：清掉上一轮的流头，
                                // 否则后加入的观看者会拿到错位的字节（黑屏）
                                if let Some(ref code) = device_id {
                                    info!("device {code} video stream reset (new ffmpeg run)");
                                    state.video_hub.reset_stream(code).await;
                                }
                            }
                            other => warn!("unknown video publisher message: {other}"),
                        }
                    }
                    Ok(Message::Binary(data)) => {
                        if let Some(ref code) = device_id {
                            frames += 1;
                            state
                                .video_hub
                                .publish(code, Arc::from(data.as_ref()))
                                .await;
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                    Err(error) => {
                        warn!("video publisher socket error: {error}");
                        break;
                    }
                }
            }
            message = next_control(&mut controls) => match message {
                Some(text) => {
                    // 观看者要求起/停流：写到推流端这条连接上（板端 relay 据此起 ffmpeg）
                    if sink.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                None => controls = None,
            },
        }
    }

    if let Some(code) = device_id {
        info!("device {code} video publisher detached after {frames} frames");
        state.video_hub.unregister_control(&code, session).await;
        state.video_hub.publisher_gone(&code, session).await;
    }
}

/// 浏览器观看通道：`GET /api/devices/{id}/video/ws?token=…`
///
/// 浏览器 WS 不能带 `Authorization` 头，鉴权中间件因此也接受 `?token=`。
/// 升级前的校验（设备存在 / 权限 / 在线 / 已配置地址）失败会走普通 HTTP 错误，
/// 前端 `onerror` 里能直接给出提示，不必等推流超时。
pub async fn viewer_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT device_id, video_url, video_type, status FROM devices WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (device_code, video_url, video_type, status) =
        row.ok_or_else(|| AppError::NotFound("设备不存在".to_string()))?;

    auth::ensure_device_access(&state, &claims, &device_code).await?;

    if status == "离线" {
        return Err(AppError::BadRequest(
            "设备离线，无法拉取实时视频".to_string(),
        ));
    }
    // 拉流地址就是设备档案里填的那个（设备管理 → 编辑 → 视频地址）
    let url = video_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest("设备未配置视频地址，请在设备管理里填写 RTSP 地址".to_string())
        })?
        .to_string();
    let kind = video_type.unwrap_or_else(|| "rtsp".to_string());

    Ok(ws.on_upgrade(move |socket| {
        handle_viewer(socket, state, VideoSource::DeviceClient {
            code: device_code,
            url,
            kind,
        })
    }))
}

/// 背包中继观看通道：`GET /api/backpack/video/ws?token=…&stream=dogcam`
///
/// 板端 relay 常驻推流连接（见 `handle_publisher`），有人在看才真正拉流编码：
/// 第一个观看者进来时通过推流连接下发 `video_start`，最后一个离开下发 `video_stop`。
/// 浏览器只跟平台通信，因此**不依赖操作员电脑在内网、也不需要浏览器本地网络许可**。
pub async fn backpack_viewer_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let stream = params
        .get("stream")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("dogcam")
        .to_string();
    if !BACKPACK_STREAMS.contains(&stream.as_str()) {
        return Err(AppError::BadRequest(format!("未知的背包通道：{stream}")));
    }
    let room = format!("{BACKPACK_ROOM_PREFIX}{stream}");
    if !state.video_hub.has_publisher(&room).await {
        return Err(AppError::BadRequest(
            "板端推流服务未连接：确认背包上 backpack-relay 服务在运行".to_string(),
        ));
    }

    Ok(ws.on_upgrade(move |socket| {
        handle_viewer(socket, state, VideoSource::BoardPusher { stream, room })
    }))
}

/// 观看者的拉流方式（两者的房间与观看者逻辑一样，只是控制消息的通道不同）
enum VideoSource {
    /// 设备客户端：客户端自己拉 `devices.video_url`，控制消息走设备网关连接
    DeviceClient {
        code: String,
        url: String,
        kind: String,
    },
    /// 板端推流服务：它自己拉本地流，控制消息写回它自己那条推流连接
    BoardPusher { stream: String, room: String },
}

impl VideoSource {
    fn room(&self) -> &str {
        match self {
            VideoSource::DeviceClient { code, .. } => code,
            VideoSource::BoardPusher { room, .. } => room,
        }
    }

    /// 有人开始看：让推流端开始拉流 / 编码
    async fn start(&self, state: &AppState) -> bool {
        let sent = match self {
            VideoSource::DeviceClient { code, url, kind } => {
                info!("device {code} video requested, asking client to pull {kind} stream");
                if gateway::send_video_control(&state.device_connections, code, Some(url)).await {
                    true
                } else {
                    // 客户端没挂在设备网关上（如只连了推流连接）：回退到推流连接
                    state
                        .video_hub
                        .send_control(
                            code,
                            json!({"msgType": "video_start", "payload": {"url": url, "format": "flv"}})
                                .to_string(),
                        )
                        .await
                }
            }
            VideoSource::BoardPusher { stream, room } => {
                info!("backpack stream {stream} requested, asking board relay to start");
                state
                    .video_hub
                    .send_control(
                        room,
                        json!({"msgType": "video_start", "payload": {"stream": stream, "format": "flv"}})
                            .to_string(),
                    )
                    .await
            }
        };
        // 只有真的让推流端起过流，才需要让看门狗盯着它
        if sent {
            state.video_hub.mark_streaming(self.room(), true).await;
        }
        sent
    }

    /// 没人看了：让推流端停掉拉流 / 编码
    async fn stop(&self, state: &AppState) -> bool {
        state.video_hub.mark_streaming(self.room(), false).await;
        match self {
            VideoSource::DeviceClient { code, .. } => {
                gateway::send_video_control(&state.device_connections, code, None).await
            }
            VideoSource::BoardPusher { room, .. } => {
                state
                    .video_hub
                    .send_control(
                        room,
                        json!({"msgType": "video_stop", "payload": {}}).to_string(),
                    )
                    .await
            }
        }
    }
}

async fn handle_viewer(socket: WebSocket, state: AppState, source: VideoSource) {
    let (mut sink, mut stream) = socket.split();
    let room = source.room().to_string();
    let (mut rx, first, init) = state.video_hub.viewer_join(&room).await;

    // 中途加入：先补一个流头（FLV 魔数 + AVC 配置 + 关键帧），否则 flv.js 黑屏。
    //
    // **第一个观看者例外**：它的加入才触发推流端起流（板端 relay 是常驻连接，
    // 每次起流都换一条新 FLV），手上这份流头属于上一轮 —— 补出去会让 flv.js
    // 先按旧头初始化、再撞上新流的中段字节，解析错位后卡在黑屏。
    if !first
        && let Some(init) = init
        && sink
            .send(Message::Binary(init.as_ref().to_vec().into()))
            .await
            .is_err()
    {
        if state.video_hub.viewer_leave(&room).await {
            source.stop(&state).await;
        }
        return;
    }

    if first && !source.start(&state).await {
        warn!("viewer of {room}: video_start 没送出去（推流端未连接）");
    }

    debug!("viewer of {room} attached (first={first})");
    loop {
        tokio::select! {
            frame = rx.recv() => match frame {
                Ok(bytes) => {
                    if sink.send(Message::Binary(bytes.as_ref().to_vec().into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // 浏览器侧追不上：丢帧保实时
                    warn!("viewer of {room} lagged, dropped {skipped} frames");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("viewer of {room}: room closed");
                    break;
                }
            },
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Close(_))) | None => {
                    debug!("viewer of {room}: browser closed the socket");
                    break;
                }
                Some(Err(error)) => {
                    debug!("viewer of {room}: socket error {error}");
                    break;
                }
                // 浏览器不该发数据，忽略
                _ => {}
            },
        }
    }

    if state.video_hub.viewer_leave(&room).await {
        info!("last viewer of {room} left, asking publisher to stop stream");
        source.stop(&state).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{VideoHub, flv_init_len};
    use std::sync::Arc;

    /// 造一段 FLV：header + AVC 序列头 tag + 关键帧 tag + 普通帧 tag
    fn flv_stream() -> Vec<u8> {
        fn tag(tag_type: u8, body: &[u8]) -> Vec<u8> {
            let mut out = vec![tag_type];
            let size = (body.len() as u32).to_be_bytes();
            out.extend_from_slice(&size[1..]);
            out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]); // 时间戳 + streamId
            out.extend_from_slice(body);
            out.extend_from_slice(&((body.len() as u32 + 11).to_be_bytes()));
            out
        }

        // FLV header 共 13 字节：魔数 + version + flags + DataOffset(4) + PreviousTagSize0(4)
        let mut out = b"FLV\x01\x05".to_vec();
        out.extend_from_slice(&[0, 0, 0, 9]); // DataOffset = 9
        out.extend_from_slice(&[0, 0, 0, 0]); // PreviousTagSize0
        out.extend(tag(9, &[0x17, 0x00, 0x00, 0x00, 0x00])); // AVC 序列头
        out.extend(tag(9, &[0x17, 0x01, 0x00, 0x00, 0x00])); // 关键帧
        out.extend(tag(9, &[0x27, 0x01, 0x00, 0x00, 0x00])); // 普通帧
        out
    }

    #[test]
    fn init_prefix_stops_right_after_the_keyframe_tag() {
        let stream = flv_stream();
        assert_eq!(flv_init_len(&stream), Some(INIT_LEN));
        // 只给到关键帧 tag 的一半：还解析不出来
        assert_eq!(flv_init_len(&stream[..INIT_LEN - 3]), None);
        assert_eq!(flv_init_len(b"not a flv stream"), None);
    }

    #[tokio::test]
    async fn first_viewer_requests_stream_and_last_one_stops_it() {
        let hub = VideoHub::default();

        let (_rx1, first, init) = hub.viewer_join("RD-001").await;
        assert!(first, "第一个观看者应触发推流");
        assert!(init.is_none(), "刚开始推流时还没有流头可补");
        assert_eq!(hub.viewers("RD-001").await, 1);

        let (_rx2, first, _) = hub.viewer_join("RD-001").await;
        assert!(!first, "第二个观看者不该重复下发 video_start");
        assert_eq!(hub.viewers("RD-001").await, 2);

        assert!(!hub.viewer_leave("RD-001").await);
        assert!(hub.viewer_leave("RD-001").await, "最后一个离开应停止推流");
        assert_eq!(hub.viewers("RD-001").await, 0);
    }

    #[tokio::test]
    async fn publish_without_viewer_is_dropped() {
        let hub = VideoHub::default();
        // 没人观看时推上来的帧直接丢弃，不建房间
        hub.publish("RD-001", Arc::from(vec![1_u8, 2, 3])).await;
        assert_eq!(hub.viewers("RD-001").await, 0);
    }

    /// 流头长度：FLV header(13) + AVC 序列头 tag(20) + 关键帧 tag(20)
    const INIT_LEN: usize = 13 + 20 + 20;

    #[tokio::test]
    async fn first_viewer_gets_stream_from_the_flv_header() {
        let hub = VideoHub::default();
        let (mut rx, _, _) = hub.viewer_join("RD-001").await;
        let stream = flv_stream();

        // 流头还没解析完：字节先缓存，不能直接发给观看者
        hub.publish("RD-001", Arc::from(&stream[..10])).await;
        assert!(rx.try_recv().is_err(), "流头不完整时不应下发字节");

        hub.publish("RD-001", Arc::from(&stream[10..])).await;
        let head = rx.recv().await.expect("应收到补齐后的流头");
        assert_eq!(
            head.as_ref(),
            &stream[..INIT_LEN],
            "应从 FLV 魔数开始完整下发"
        );

        let rest = rx.recv().await.expect("流头之后的帧继续下发");
        assert_eq!(rest.as_ref(), &stream[INIT_LEN..]);
    }

    #[tokio::test]
    async fn publisher_without_viewer_is_reported_idle() {
        let hub = VideoHub::default();
        hub.publisher_attach("RD-001", 1).await;
        // 没见过 video_start 的房间不算空转（板端 relay 的常驻连接就属于这种）
        assert!(hub.idle_publishers().await.is_empty());

        hub.mark_streaming("RD-001", true).await;
        assert_eq!(
            hub.idle_publishers().await,
            vec!["RD-001".to_string()],
            "起过流又没人看时看门狗应能发现"
        );

        let (_rx, _, _) = hub.viewer_join("RD-001").await;
        assert!(hub.idle_publishers().await.is_empty(), "有观看者时不算空转");
    }

    /// 现场真实流程：观看者先进（它的加入才触发 video_start），推流端后到 ——
    /// 推流端接入时绝不能把等待中的观看者踢掉。
    #[tokio::test]
    async fn publisher_attach_keeps_viewers_that_joined_first() {
        let hub = VideoHub::default();
        let (mut rx, first, _) = hub.viewer_join("RD-001").await;
        assert!(first);
        hub.publisher_attach("RD-001", 1).await;
        assert_eq!(hub.viewers("RD-001").await, 1, "观看者不该被清掉");

        let stream = flv_stream();
        hub.publish("RD-001", Arc::from(stream.clone())).await;
        let head = rx.recv().await.expect("推流后观看者应收到字节");
        assert_eq!(head.as_ref(), &stream[..INIT_LEN]);
    }

    #[tokio::test]
    async fn publisher_reconnect_closes_old_viewers() {
        let hub = VideoHub::default();
        let (mut rx, _, _) = hub.viewer_join("RD-001").await;
        hub.publisher_attach("RD-001", 1).await;
        // 推流端重连 → 旧房间被丢弃，观看端收到 Closed（前端据此自动重连）
        hub.publisher_attach("RD-001", 2).await;
        assert!(matches!(
            rx.recv().await,
            Err(tokio::sync::broadcast::error::RecvError::Closed)
        ));
    }

    #[tokio::test]
    async fn stale_publisher_cleanup_keeps_the_new_room() {
        let hub = VideoHub::default();
        hub.publisher_attach("RD-001", 1).await;
        // 推流端重连：新会话接管房间
        hub.publisher_attach("RD-001", 2).await;
        // 旧连接迟到的清理不能把新房间删掉
        hub.publisher_gone("RD-001", 1).await;
        assert!(hub.has_publisher("RD-001").await);

        hub.publisher_gone("RD-001", 2).await;
        assert!(!hub.has_publisher("RD-001").await, "会话匹配时应清掉房间");
    }

    /// 最后一个观看者离开时，如果推流端还挂着（板端 relay 常驻）不能把房间删掉：
    /// 删了 `has_publisher` 就变 false，下一次观看会被误判成「推流服务未连接」。
    #[tokio::test]
    async fn room_with_publisher_survives_last_viewer_leaving() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 1).await;
        let (_rx, first, _) = hub.viewer_join("bp:dogcam").await;
        assert!(first);
        assert!(hub.viewer_leave("bp:dogcam").await, "应判定为最后一个观看者");
        assert!(hub.has_publisher("bp:dogcam").await, "推流端还在就不能丢房间");

        // 再来一个观看者：仍然认得这路推流，推上来的字节能收到
        let (mut rx, first, _) = hub.viewer_join("bp:dogcam").await;
        assert!(first);
        hub.publish("bp:dogcam", Arc::from(flv_stream())).await;
        assert!(rx.recv().await.is_ok(), "房间保留时观看者应能收到流");
    }

    /// 常驻推流连接里 ffmpeg 重启（换了一条新流）：旧流头必须丢掉，
    /// 否则后加入的观看者会拿到「上一轮关键帧 + 这一轮中段字节」，解析错位黑屏。
    #[tokio::test]
    async fn reset_stream_clears_the_previous_run_header() {
        let hub = VideoHub::default();
        let (_rx, _, _) = hub.viewer_join("bp:dogcam").await;
        hub.publisher_attach("bp:dogcam", 1).await;
        hub.publish("bp:dogcam", Arc::from(flv_stream())).await;

        let (_rx, _, init) = hub.viewer_join("bp:dogcam").await;
        assert!(init.is_some(), "同一条流里中途加入应拿到流头");

        hub.reset_stream("bp:dogcam").await;
        let (_rx, _, init) = hub.viewer_join("bp:dogcam").await;
        assert!(init.is_none(), "新一轮起流后不能再补上一轮的关键帧");
    }

    /// 没有推流端的房间在最后一个观看者离开后照旧被清掉（设备客户端场景）
    #[tokio::test]
    async fn room_without_publisher_is_dropped_with_the_last_viewer() {
        let hub = VideoHub::default();
        let (_rx, _, _) = hub.viewer_join("RD-001").await;
        assert!(hub.viewer_leave("RD-001").await);
        assert!(!hub.has_publisher("RD-001").await);
    }

    /// 推流端抖动（房间被丢弃）时，最后一个观看者离开仍必须触发 `video_stop`，
    /// 否则客户端会一直重连推流 —— 现场真机上踩过：相机断一下，狗就一直编码。
    #[tokio::test]
    async fn last_viewer_still_reported_after_room_was_dropped() {
        let hub = VideoHub::default();
        let (_rx, _, _) = hub.viewer_join("RD-001").await;
        hub.publisher_attach("RD-001", 1).await;
        hub.publisher_gone("RD-001", 1).await; // 房间没了，但观看者还连着
        assert!(hub.viewer_leave("RD-001").await, "应判定为最后一个观看者");
        assert_eq!(hub.viewers("RD-001").await, 0);
    }

    #[tokio::test]
    async fn late_viewer_gets_init_prefix() {
        let hub = VideoHub::default();
        let (_first_rx, _, _) = hub.viewer_join("RD-001").await;
        let stream = flv_stream();
        hub.publish("RD-001", Arc::from(stream.clone())).await;

        let (_rx, _, init) = hub.viewer_join("RD-001").await;
        let init = init.expect("后加入的观看者应拿到流头");
        assert_eq!(init.as_ref(), &stream[..INIT_LEN]);
    }

    /// 板端 relay 房间（`bp:`）：控制消息通过推流连接下发，且旧连接迟到的清理
    /// 不能把新连接的通道删掉（否则重连后观看端再也起不了流）。
    #[tokio::test]
    async fn board_relay_control_channel_follows_the_live_session() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 7).await;
        assert!(hub.has_publisher("bp:dogcam").await, "relay 常驻连接应算在线");

        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        hub.register_control("bp:dogcam", 7, tx).await;
        assert!(
            hub.send_control("bp:dogcam", "video_start".to_string()).await,
            "板端房间应能收到控制消息"
        );
        assert_eq!(rx.recv().await.as_deref(), Some("video_start"));

        // 旧会话（6）迟到的清理不应动到当前会话（7）的通道
        hub.unregister_control("bp:dogcam", 6).await;
        assert!(hub.send_control("bp:dogcam", "still".to_string()).await);

        hub.unregister_control("bp:dogcam", 7).await;
        assert!(!hub.send_control("bp:dogcam", "gone".to_string()).await);
    }

    /// 设备客户端的房间没有控制通道（控制走设备网关），看门狗与观看端据此分流。
    #[tokio::test]
    async fn device_room_has_no_control_channel() {
        let hub = VideoHub::default();
        hub.publisher_attach("RD-001", 1).await;
        assert!(!hub.send_control("RD-001", "video_stop".to_string()).await);
        assert!(hub.has_publisher("RD-001").await);
        assert!(!hub.has_publisher("bp:cam0").await, "没有推流端时不应报在线");
    }
}
