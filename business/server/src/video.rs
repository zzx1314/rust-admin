//! RTSP 实时视频中继（HTTP-FLV over WebSocket）
//!
//! 智航 Pro 协议没有视频接口，本体只对外提供 RTSP；浏览器不能直连 RTSP，
//! 而能访问本体网段的是**设备客户端**。所以推拉流都走客户端：
//!
//! ```text
//! 机器狗本体 RTSP ──ffmpeg──► 设备客户端 ──ws://平台/ws/device/video──► 业务平台 ──ws──► 浏览器(flv.js)
//!  rtsp://…:8554/test          转封装 FLV            二进制帧中继              MSE 播放
//! ```
//!
//! 三条约定：
//!
//! 1. **按需推流**：第一个观看者进来才给客户端下发 `video_start`（带 `devices.video_url`），
//!    最后一个离开立刻 `video_stop`，没人看的时候机器狗侧不耗 CPU / 带宽。
//! 2. **二进制专用连接**：浏览器这条 WS 只发 FLV 字节，绝不发文本帧 ——
//!    flv.js 的 `WebSocketLoader` 收到文本帧会直接报错。
//! 3. **地址唯一来源**：拉流地址就是设备档案里的 `video_url`，
//!    运营在「设备管理」里改一次即可，不需要再动客户端配置。

use crate::app::AppState;
use crate::auth::{self, Claims};
use crate::error::AppError;
use crate::gateway;
use crate::gateway_types::WsMessage;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Path, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};

/// 单个观看者的积压帧上限：网络慢时丢帧保实时，不把服务端内存拖爆
const VIEWER_BUFFER_FRAMES: usize = 128;

/// 流头（FLV header + AVC 序列头 + 首个关键帧）解析的上限
///
/// 1200kbps 下 4MB ≈ 26 秒，正常 2 秒内（一个 GOP）就拿到关键帧；
/// 超出说明推上来的不是 FLV/H.264，直接放弃补头，避免内存被吃满。
const MAX_INIT_BYTES: usize = 4 * 1024 * 1024;

/// 推流会话自增号（见 `VideoHub::publisher_attach`）
static PUBLISHER_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 视频中继房间：一台设备一个房间
///
/// 生产者只有一个（该设备的客户端推流连接），消费者是任意多个浏览器。
#[derive(Clone, Default)]
pub struct VideoHub {
    rooms: Arc<RwLock<HashMap<String, Room>>>,
}

struct Room {
    tx: broadcast::Sender<Arc<[u8]>>,
    /// 当前观看者数量：0 → 1 时要求客户端开始推流，回到 0 时要求停止
    viewers: usize,
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

impl VideoHub {
    /// 观看者加入：返回订阅端、是否「第一个观看者」、以及可补发的流头
    pub async fn viewer_join(
        &self,
        device_id: &str,
    ) -> (broadcast::Receiver<Arc<[u8]>>, bool, Option<Arc<[u8]>>) {
        let mut rooms = self.rooms.write().await;
        let room = rooms
            .entry(device_id.to_string())
            .or_insert_with(|| Room {
                tx: broadcast::channel(VIEWER_BUFFER_FRAMES).0,
                viewers: 0,
                init: None,
                pending: Vec::new(),
                publisher: None,
            });
        room.viewers += 1;
        let init = room.init.clone();
        (room.tx.subscribe(), room.viewers == 1, init)
    }

    /// 观看者离开：返回是否是「最后一个观看者」（此时房间已被移除）
    pub async fn viewer_leave(&self, device_id: &str) -> bool {
        let mut rooms = self.rooms.write().await;
        let Some(room) = rooms.get_mut(device_id) else {
            return false;
        };
        room.viewers = room.viewers.saturating_sub(1);
        if room.viewers == 0 {
            rooms.remove(device_id);
            return true;
        }
        false
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
        rooms.insert(
            device_id.to_string(),
            Room {
                tx: broadcast::channel(VIEWER_BUFFER_FRAMES).0,
                viewers: 0,
                init: None,
                pending: Vec::new(),
                publisher: Some(session),
            },
        );
    }

    /// 「有人在推、但没人看」的设备：看门狗据此让客户端停掉 ffmpeg
    pub async fn idle_publishers(&self) -> Vec<String> {
        self.rooms
            .read()
            .await
            .iter()
            .filter(|(_, room)| room.publisher.is_some() && room.viewers == 0)
            .map(|(device_id, _)| device_id.clone())
            .collect()
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

    /// 当前是否有观看者（日志 / 排障用）
    pub async fn viewers(&self, device_id: &str) -> usize {
        self.rooms
            .read()
            .await
            .get(device_id)
            .map(|room| room.viewers)
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
pub async fn publisher_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_publisher(socket, state))
}

async fn handle_publisher(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let mut device_id: Option<String> = None;
    // 会话标识：同一台设备重连时，旧连接迟到的清理不会误删新房间
    let session = PUBLISHER_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut frames: u64 = 0;

    while let Some(msg) = stream.next().await {
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
                        device_id = Some(code);
                    }
                    other => warn!("unknown video publisher message: {other}"),
                }
            }
            Ok(Message::Binary(data)) => {
                if let Some(ref code) = device_id {
                    frames += 1;
                    state.video_hub.publish(code, Arc::from(data.as_ref())).await;
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

    if let Some(code) = device_id {
        info!("device {code} video publisher detached after {frames} frames");
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
        handle_viewer(socket, state, device_code, url, kind)
    }))
}

async fn handle_viewer(
    socket: WebSocket,
    state: AppState,
    device_code: String,
    url: String,
    kind: String,
) {
    let (mut sink, mut stream) = socket.split();
    let (mut rx, first, init) = state.video_hub.viewer_join(&device_code).await;

    // 中途加入：先补一个流头（FLV 魔数 + AVC 配置 + 关键帧），否则 flv.js 黑屏
    if let Some(init) = init
        && sink
            .send(Message::Binary(init.as_ref().to_vec().into()))
            .await
            .is_err()
    {
        if state.video_hub.viewer_leave(&device_code).await {
            gateway::send_video_control(&state.device_connections, &device_code, None).await;
        }
        return;
    }

    if first {
        info!("device {device_code} video requested, asking client to pull {kind} stream");
        let delivered = gateway::send_video_control(
            &state.device_connections,
            &device_code,
            Some(&url),
        )
        .await;
        if !delivered {
            warn!("device {device_code} not connected, video_start dropped");
        }
    }

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
                    warn!("viewer of {device_code} lagged, dropped {skipped} frames");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                // 浏览器不该发数据，忽略
                _ => {}
            },
        }
    }

    if state.video_hub.viewer_leave(&device_code).await {
        info!("last viewer of {device_code} left, asking client to stop stream");
        gateway::send_video_control(&state.device_connections, &device_code, None).await;
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
        assert_eq!(head.as_ref(), &stream[..INIT_LEN], "应从 FLV 魔数开始完整下发");

        let rest = rx.recv().await.expect("流头之后的帧继续下发");
        assert_eq!(rest.as_ref(), &stream[INIT_LEN..]);
    }

    #[tokio::test]
    async fn publisher_without_viewer_is_reported_idle() {
        let hub = VideoHub::default();
        hub.publisher_attach("RD-001", 1).await;
        assert_eq!(
            hub.idle_publishers().await,
            vec!["RD-001".to_string()],
            "有人推、没人看时看门狗应能发现"
        );

        let (_rx, _, _) = hub.viewer_join("RD-001").await;
        assert!(hub.idle_publishers().await.is_empty(), "有观看者时不算空转");
    }

    #[tokio::test]
    async fn publisher_reconnect_closes_old_viewers() {
        let hub = VideoHub::default();
        let (mut rx, _, _) = hub.viewer_join("RD-001").await;
        // 推流端重连 → 旧房间被丢弃，观看端收到 Closed（前端据此自动重连）
        hub.publisher_attach("RD-001", 1).await;
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
        assert_eq!(hub.idle_publishers().await, vec!["RD-001".to_string()]);

        hub.publisher_gone("RD-001", 2).await;
        assert!(hub.idle_publishers().await.is_empty(), "会话匹配时应清掉房间");
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
}
