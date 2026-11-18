//! 背包实时视频中继（HTTP-FLV over WebSocket）
//!
//! 板端 relay 把背包本地 RTSP（含本体前置 `dogcam`）转成 FLV、主动推到平台，
//! 浏览器只跟平台同源通信：
//!
//! ```text
//! 背包本地 RTSP ──ffmpeg──► 板端 relay ──ws://平台/ws/device/video──► 业务平台 ──ws──► 浏览器(flv.js)
//! ```
//!
//! 三条约定：
//!
//! 1. **按需推流**：第一个观看者进来才给推流端下发 `video_start`，最后一个离开后留
//!    [`STREAM_IDLE_GRACE`] 宽限再由看门狗 `video_stop`，没人看的时候现场侧不耗 CPU / 带宽。
//! 2. **二进制专用连接**：浏览器这条 WS 只发 FLV 字节，绝不发文本帧 ——
//!    flv.js 的 `WebSocketLoader` 收到文本帧会直接报错。
//! 3. **房间名前缀**：板端推流没有设备台账，房间名用 [`BACKPACK_ROOM_PREFIX`]
//!    （`bp:<stream>`），控制消息直接走它自己那条推流连接（文本帧），
//!    见 `handle_publisher` 里的 `controls`。
//!
//! 设备客户端那条「拉 `devices.video_url` → 平台中继」的旧链路已于 2026-11-18 整体删除：
//! 设备管理 / 计划管理 / 实时视频三处画面统一走背包前置 `dogcam`。
//!
//! 为什么背包也要走中继：现场入口（浏览器直连板端）必须「操作员电脑在同一个内网且
//! 过了 Chrome 的 Local Network Access 许可」，换 WiFi / 远程 / 换浏览器就黑屏。
//! 推流中继只要求**板子能出公网**，浏览器与平台同源，延时 1~2s。

use crate::app::AppState;
use crate::auth::Claims;
use crate::error::{ApiResponse, AppError};
use crate::gateway_types::WsMessage;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Extension, Query, State};
use axum::response::{IntoResponse, Json};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{debug, info, warn};

/// 单个观看者的积压帧上限（设备客户端房间 `RD-*`）：网络慢时丢帧保实时，不把服务端内存拖爆
///
/// 注意这里是**分片**数（推流端每次读 ffmpeg stdout 8KB，一个分片就 8KB），
/// 所以 128 片只有 1MB：1080p/4Mbps 下就是 2 秒。实测现场浏览器只要卡 2 秒
/// （切标签页、Wi-Fi 抖一下、MSE 追加慢一拍）就会触发 `Lagged` → 断开重连，
/// 而重连要等推流端重新起 ffmpeg（3~7s），于是变成「一直重连」的死循环。
/// 512 片 = 4MB，1080p 2.5Mbps 约 13 秒、720p 2Mbps 约 16 秒的余量，
/// 偶发卡顿扛得住，真断网才会走重连。
const VIEWER_BUFFER_FRAMES: usize = 512;

/// 板端背包房间（`bp:`）单观看者的积压上限：128 片 = 1MB
///
/// 背包推的是省流档 500kbps ~ 配置档 1.2Mbps，8KB 一片 → 1MB ≈ 16 秒 @500k。
/// 够扛住浏览器切标签 / 一次重连（128 片跑 4Mbps 才 2 秒，那是设备客户端房间的场景），
/// 又**不允许**像 4MB 那样「65 秒前的画面静默躺在队列里」：2026-11-18 现场就是这种 ——
/// 上行抖 45 秒，板端 ffmpeg 恢复后以 3 倍速把攒下的画面倒出来
/// （日志 `last viewer ... 6.9s / 2964 KB → 3533 kbps`），观众端缓冲吃下十几秒，
/// 于是「越看越慢」，而平台日志一个字都没打。真落后由 [`VIEWER_LAG_DROP`] 兜住。
const BACKPACK_VIEWER_BUFFER_FRAMES: usize = 128;

/// 观看端落后直播多少秒就断开让它重连（[`FlvScanner`] 量出来的「墙钟流逝 - 媒体流逝」）
///
/// 环形缓冲再小也只能**限制**静默积压，不能**发现**它 —— 只有拿 FLV 视频 tag 的时间戳
/// 和墙钟比，才知道这个观看者手上是多旧的画面。超过阈值就断开：前端 250ms 后重连，
/// 服务端补的是当前这一轮的流头（header + 关键帧）＝直播边缘。
/// 取 2s：小于它的抖动交给前端 `liveBufferLatencyChasing`（1.2s）自己追；大于它的卡顿
/// 用户早就看着画面定住了，重连一次不会有观感损失。
const VIEWER_LAG_DROP: f64 = 2.0;

/// FLV 时间轴「跳变」的判据（秒）
///
/// 中途加入补的流头（本轮第一帧，ts≈0）后面紧跟的是直播帧；板端换档重启 ffmpeg 时
/// 时间轴也归零。这些**不是**积压，混进统计会算出几十秒的假延迟。
const FLV_TS_JUMP_SECS: f64 = 5.0;

/// 单个 FLV tag 的字节上限（解析缓冲兜底：坏流不能把内存吃满）
const MAX_FLV_TAG_BYTES: usize = 1 << 20;

/// 该房间单观看者的积压片数上限
///
/// 背包推流码率只有设备客户端的 1/4~1/8，同样片数换算成秒数差着量级，所以按房间分开。
fn viewer_buffer_frames(device_id: &str) -> usize {
    if device_id.starts_with(BACKPACK_ROOM_PREFIX) {
        BACKPACK_VIEWER_BUFFER_FRAMES
    } else {
        VIEWER_BUFFER_FRAMES
    }
}

/// 「最后一个观看者离开」到真正停流之间的宽限。
///
/// 现场痛点：浏览器刷新 / 切换看板 / 弱网重连都只断几秒，所以「最后一个观看者离开」
/// 之后不能立刻停流，否则下一个观看者进来又要重新起 ffmpeg + 等关键帧 —— 现场感受就是
/// 「视频连接特别慢、总要黑一下」。
///
/// 但也不能太长：没人在看也一直在编码，机器狗的电池 / CPU / 4G 流量都在白烧（现场开了
/// 代理后「看一会儿就掉电」很难解释）。 15s 足够盖住「刷新页面 / 切页签 / 断一两秒」，
/// 又能在走开后很快停掉。重连时能拿到多少缓存跟这个宽限无关（靠 `Room::catch_up()` 补
/// 当前 GOP），所以没必要为了 "秒开" 拿 60s。
const STREAM_IDLE_GRACE: std::time::Duration = std::time::Duration::from_secs(15);

/// 推流端多久没有任何消息（二进制帧 / WS ping）就判死。
///
/// 现场板子出公网走 Wi-Fi，掉线后到平台的 TCP 常常是半开：不 FIN、不 RST，平台会一直
/// 以为「推流端在线」，观看者进来永远等不到画面（以前要 60s 才判死，画面就冻住 1 分钟）。
/// 板端 relay 每 10s 一个心跳，这里 30s 即 3 次没到，足够保守也不会误杀。
const PUBLISHER_SILENCE: std::time::Duration = std::time::Duration::from_secs(30);

/// 流头（FLV header + AVC 序列头 + 首个关键帧）解析的上限
///
/// 1200kbps 下 4MB ≈ 26 秒，正常 2 秒内（一个 GOP）就拿到关键帧；
/// 超出说明推上来的不是 FLV/H.264，直接放弃补头，避免内存被吃满。
const MAX_INIT_BYTES: usize = 4 * 1024 * 1024;

/// 「当前 GOP」缓冲的上限：720p15/1000k 一个 GOP（2s）约 250KB，给 4MB 富余
///
/// 超限（编码器不吐关键帧 / 码率失控）就整个丢掉，退回用 `init` 补流 —— 宁可变卡，不能把内存吃满。
const MAX_GOP_BYTES: usize = 4 * 1024 * 1024;

/// 推流会话自增号（见 `VideoHub::publisher_attach`）
static PUBLISHER_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 背包板端推流房间的房间名前缀（`bp:<stream>`）
///
/// 板端推流服务没有设备台账，房间名统一带上这个前缀：带前缀的房间在
/// `handle_publisher` 里会挂上**控制通道**，`video_start` / `video_stop`
/// 直接写进它自己的推流连接（不经过设备网关）。
pub const BACKPACK_ROOM_PREFIX: &str = "bp:";

/// 允许通过中继观看的背包通道（与板端 mediamtx 的 path 一致）
///
/// 白名单而不是透传：房间名来自查询串，不限制就能被拿来刷任意 key 撑爆内存。
///
/// `preview` 是板端 AI 烧框后的那一路（`osd.py` 把中文标签画进画面）。跨网段时
/// 浏览器画不了框（中继流没有坐标对应关系），只能看这一路——所以它必须能被中继。
const BACKPACK_STREAMS: [&str; 7] = [
    "cam0",
    "cam1",
    "cam2",
    "cam3",
    "dogcam",
    "dog-ipcam",
    "preview",
];

/// 系统设置里「中继默认档位」的键（页面：系统设置 → 视频中继）
///
/// 板端 relay 起流时按它选起点（`video_start.payload.profile`），不用再去板子上改
/// `RELAY_PROFILE` 环境变量重启服务。
pub const RELAY_PROFILE_SETTING: &str = "video_relay_profile";

/// 系统设置里「中继是否允许自动降档」的键
pub const RELAY_AUTO_DEGRADE_SETTING: &str = "video_relay_auto_degrade";

/// 可选档位名，必须与 `deploy/backpack-relay/relay_push.py::TIER_LEVELS` 一致
pub const RELAY_TIERS: [&str; 3] = ["light", "degrade", "degrade2"];

/// 默认档位：现场板子出公网主要走 4G 模块（实测上行 0.25~0.6Mbps），
/// 720p15/1000k 的 `light` 起流就会被反压；直接用省流档（640x360@12/500k），
/// 链路再差由自动降档掉到极省流档（320x180@8/200k）。
pub const DEFAULT_RELAY_TIER: &str = "degrade";

/// 当前中继档位配置：`{"tier": "light|degrade|degrade2", "auto": true}`
///
/// 库里的值不认识（手改了 SQL / 老库）就回退默认，不让一个坏值把中继锁在未知状态。
pub async fn relay_profile(state: &AppState) -> serde_json::Value {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT setting_key, setting_value FROM settings WHERE setting_key IN (?1, ?2)",
    )
    .bind(RELAY_PROFILE_SETTING)
    .bind(RELAY_AUTO_DEGRADE_SETTING)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let mut tier = DEFAULT_RELAY_TIER.to_string();
    let mut auto = true;
    for (key, value) in rows {
        let value = value.unwrap_or_default().trim().to_lowercase();
        match key.as_str() {
            RELAY_PROFILE_SETTING if RELAY_TIERS.contains(&value.as_str()) => tier = value,
            RELAY_AUTO_DEGRADE_SETTING => auto = value != "false",
            _ => {}
        }
    }
    json!({ "tier": tier, "auto": auto })
}

/// 设置页改了中继档位：立刻广播 `video_config` 给在线板端
///
/// 正在看的流会就地换档（板端 `video_start` 幂等，但这里要的是**不重连**地换参数），
/// 没人看的流下次 `video_start` 自然带上新配置。返回送达了几条推流连接。
pub async fn broadcast_relay_profile(state: &AppState) -> usize {
    let message = json!({
        "msgType": "video_config",
        "payload": { "profile": relay_profile(state).await },
    })
    .to_string();
    let mut sent = 0;
    for stream in BACKPACK_STREAMS {
        let room = format!("{BACKPACK_ROOM_PREFIX}{stream}");
        if state.video_hub.send_control(&room, message.clone()).await {
            sent += 1;
        }
    }
    sent
}

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
    ///
    /// ⚠️ 它只是**兜底**（这一轮还没攒出完整 GOP 时才用），正常补流走
    /// [`Room::header`] + [`Room::gop`]，原因见 `gop` 的注释。
    init: Option<Arc<[u8]>>,
    /// `init` 去掉「首个关键帧」的部分：FLV header(13) + onMetaData + AVC 序列头
    ///
    /// 补当前 GOP 时要把它接在 GOP 前面（浏览器必须有 FLV 魔数和 SPS/PPS）。
    header: Option<Arc<[u8]>>,
    /// 当前 GOP 的字节：从**最近一个视频关键帧 tag 开头**开始，给中途加入的观看者补流
    ///
    /// 为什么不直接补 `init`（本轮第一个关键帧）：那段时间戳是本轮开头（ts≈0），而直播帧
    /// 已经跑到几百秒 —— 浏览器 MSE 里会出现一个**几百秒的空洞**；而且紧跟在补的关键帧
    /// 后面的是**缺参考帧的 P 帧**（花屏 / 画面定住，要等下一个关键帧才自愈）。
    /// 现场表现就是「字节还在流、画面定住」（2026-11-18 21:53 事故：补的流头 ts=0，
    /// 直播帧已经 ts=435s）。
    ///
    /// 补当前 GOP 则是：时间轴连续、参考帧完整、最多落后一个 GOP（默认 2s）。
    gop: Vec<u8>,
    /// 当前 GOP 第一个字节到达的时刻（诊断用 `gopAgeSecs`）
    gop_at: Option<std::time::Instant>,
    /// `gop` 的组装缓冲：还没对齐到 tag 边界的尾巴（一个 keyframe 往往比 8KB 分片大）
    gop_pending: Vec<u8>,
    /// 流头解析完成前先缓存，避免把不完整的字节发给观看者
    pending: Vec<u8>,
    /// 推流会话标识：`Some(id)` 表示在线，`id` 用来区分「新连接」与「旧连接迟到的清理」
    publisher: Option<u64>,
    /// 最后一个观看者离开的时刻（`None` = 还有人在看）。
    /// 看门狗只有超过 [`STREAM_IDLE_GRACE`] 才会真去停流。
    idle_since: Option<std::time::Instant>,
    /// 最后一帧推上来的时刻（排障用）：`GET /api/backpack/video/state` 里看它就知道
    /// 「推流端在线但画面不动」是推流端没数据，还是观看端没收到。
    last_frame: Option<std::time::Instant>,
    /// 「推流端开了新一轮流」（`video_reset`）的信号：用来把**已经连着的**观看端踢下去
    ///
    /// 板端 relay 重启 ffmpeg 后，同一条推流连接上会接着发**新一轮的 FLV**：时间戳从 0
    /// 重新开始、分辨率还可能变（降档 640x360 ⇄ 配置档 720p）。浏览器里的 flv.js 拿到的
    /// 就是「上一轮的尾巴 + 时间轴倒退的新流」，它不会自己恢复 —— 现场表现是**画面定住、
    /// 字节却还在流**（`pendingBytes` 一直是 0），只能等前端 8s 的假死看门狗重连。
    /// 所以这里主动断开：前端 250ms 起就重连，重连时服务端会补**新流头**。
    reset: Arc<tokio::sync::Notify>,
    /// 观看端「落后直播」的观测值（毫秒）：`handle_viewer` 每收一片就刷新，没有观看者时归零
    ///
    /// `GET /api/backpack/video/state` 的 `viewerLagSecs` 就是它。以前这里什么都没有 ——
    /// 现场「越看越慢」只能翻板端日志猜，因为积压是静默的（不到环形缓冲溢出不打日志）。
    viewer_lag_ms: Arc<AtomicU64>,
}

fn new_room(device_id: &str, publisher: Option<u64>) -> Room {
    Room {
        tx: broadcast::channel(viewer_buffer_frames(device_id)).0,
        init: None,
        header: None,
        gop: Vec::new(),
        gop_at: None,
        gop_pending: Vec::new(),
        pending: Vec::new(),
        publisher,
        idle_since: None,
        last_frame: None,
        reset: Arc::new(tokio::sync::Notify::new()),
        viewer_lag_ms: Arc::new(AtomicU64::new(0)),
    }
}

impl Room {
    /// 中途加入的观看者要补的字节：`FLV header + onMetaData + AVC 序列头 + 当前 GOP`
    ///
    /// - 有完整 GOP → 补当前 GOP（时间轴连续、参考帧完整，最多落后一个 GOP）；
    /// - 这一轮刚起、还没攒出 GOP（或刚被截断）→ 退回 `init`（本轮第一个关键帧）。
    ///   旧行为，只能在极短窗口内出现。
    fn catch_up(&self) -> Option<Arc<[u8]>> {
        match &self.header {
            Some(header) if !self.gop.is_empty() => {
                let mut buf = Vec::with_capacity(header.len() + self.gop.len());
                buf.extend_from_slice(header);
                buf.extend_from_slice(&self.gop);
                Some(Arc::from(buf))
            }
            _ => self.init.clone(),
        }
    }

    /// 维护「当前 GOP」：从最近一个视频关键帧 tag 开头开始累积
    ///
    /// 推上来的 8KB 分片不一定 tag 对齐（一个关键帧往往比分片大），所以先用
    /// `gop_pending` 攒到完整 tag 再决定要不要从它重新开一个 GOP。
    fn gop_feed(&mut self, chunk: &[u8]) {
        self.gop_pending.extend_from_slice(chunk);
        let mut pos = 0;
        while self.gop_pending.len() >= pos + 11 {
            let size = u32::from_be_bytes([
                0,
                self.gop_pending[pos + 1],
                self.gop_pending[pos + 2],
                self.gop_pending[pos + 3],
            ]) as usize;
            let end = pos + 11 + size + 4;
            if self.gop_pending.len() < end {
                break;
            }
            let tag = &self.gop_pending[pos..end];
            if video_keyframe_tag(tag) {
                // 新 GOP：之前攒的（上一个 GOP 的 P 帧）全部作废
                self.gop.clear();
                self.gop_at = Some(std::time::Instant::now());
            }
            if self.gop_at.is_some() {
                self.gop.extend_from_slice(tag);
            }
            pos = end;
        }
        self.gop_pending.drain(..pos);
        if self.gop.len() > MAX_GOP_BYTES || self.gop_pending.len() > MAX_FLV_TAG_BYTES {
            self.gop.clear();
            self.gop_pending.clear();
            self.gop_at = None;
        }
    }

    /// 新一轮流开始：流头、GOP 全部作废
    fn forget_stream_bytes(&mut self) {
        self.init = None;
        self.header = None;
        self.gop.clear();
        self.gop_pending.clear();
        self.gop_at = None;
        self.pending = Vec::new();
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
        let room = rooms
            .entry(device_id.to_string())
            .or_insert_with(|| new_room(device_id, None));
        // 有人回来看了：空闲计时清零，看门狗别把还热着的流停掉
        room.idle_since = None;
        let catch_up = room.catch_up();
        (room.tx.subscribe(), first, catch_up)
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
            match rooms.get_mut(device_id) {
                // 推流端还在：开始计时，宽限内不断流（短时重连立刻能拿到流）
                Some(room) if room.publisher.is_some() => {
                    room.idle_since = Some(std::time::Instant::now());
                    // 没人看了，落后直播的观测值没意义（下一个人进来重新量）
                    room.viewer_lag_ms.store(0, Ordering::Relaxed);
                }
                _ => {
                    rooms.remove(device_id);
                }
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
        room.last_frame = Some(std::time::Instant::now());
        if room.init.is_some() {
            // 维护「当前 GOP」：中途加入的观看者靠它拿到时间轴连续的一段
            room.gop_feed(&frame);
            // 接收端全掉线时 send 报错，忽略即可；下游会自行关闭连接
            let _ = room.tx.send(frame);
            return;
        }

        room.pending.extend_from_slice(&frame);
        match flv_init_span(&room.pending) {
            Some((prefix_len, len)) => {
                let init: Arc<[u8]> = Arc::from(&room.pending[..len]);
                let header: Arc<[u8]> = Arc::from(&room.pending[..prefix_len]);
                // 13 = FLV header(9) + PrevTagSize0(4)，从这之后都是 tag 对齐的
                let init_tail: Vec<u8> = room.pending[13..len].to_vec();
                let rest: Arc<[u8]> = Arc::from(&room.pending[len..]);
                // 先补流头：先到的观看者是「流头还没解析出来就加入」的，
                // 他们的字节被缓存了，这里必须一次性补上，否则永远收不到 FLV 魔数
                let _ = room.tx.send(init.clone());
                room.init = Some(init);
                room.header = Some(header);
                room.pending = Vec::new();
                // 用本轮的第一个关键帧给 GOP 打底
                room.gop_feed(&init_tail);
                if !rest.is_empty() {
                    room.gop_feed(&rest);
                    let _ = room.tx.send(rest);
                }
            }
            None if room.pending.len() > MAX_INIT_BYTES => {
                warn!("video publisher {device_id}: 推上来的不是 FLV/H.264，放弃补流头");
                room.pending = Vec::new();
            }
            None => {}
        }
    }

    /// 推流端注册 / 重连
    ///
    /// 推流端重连意味着**一条新流**（板端 ffmpeg 跟着连接一起被杀，重连后重新起流：
    /// 新 FLV 头、时间戳归零），所以老流头必须丢掉，正在看的观看端要拿新流头。
    /// 但**房间不能整个换掉**（以前是 `rooms.insert(new_room)`）：换掉会 close 广播通道，
    /// 观看端直接收到 `Closed`；而弱网重连时旧连接常常是「半开」僵尸，要到
    /// [`PUBLISHER_SILENCE`] 才被判死，于是新连接一 attach 就把正在看的人踢下去
    /// （2026-11-18 20:22:36 / 20:23:32 连踢两次，现场就是「看着看着黑一下」）。
    /// 现在沿用房间、只换会话号：流头清理与 `video_reset` 同一条路径（`reset` 通知）。
    pub async fn publisher_attach(&self, device_id: &str, session: u64) {
        let mut rooms = self.rooms.write().await;
        match rooms.get_mut(device_id) {
            // 老推流端还挂着：新一轮流，清流头 + 通知观看端重连拿新头
            Some(room) if room.publisher.is_some() => {
                let was_live = room.init.is_some() || room.last_frame.is_some();
                info!("video publisher {device_id} 重连，沿用房间（新一轮流，时间轴归零）");
                room.publisher = Some(session);
                room.forget_stream_bytes();
                room.last_frame = None;
                room.viewer_lag_ms.store(0, Ordering::Relaxed);
                if was_live {
                    room.reset.notify_waiters();
                }
            }
            // 正常首帧流程：观看者先到（video_start 就是它触发的），推流端后到。
            // 这里**不能**清观看端，否则刚进来的观看者立刻被踢掉，画面永远出不来。
            Some(room) => {
                room.publisher = Some(session);
                room.forget_stream_bytes();
                room.viewer_lag_ms.store(0, Ordering::Relaxed);
            }
            None => {
                rooms.insert(device_id.to_string(), new_room(device_id, Some(session)));
            }
        }
    }

    /// 「有人在推、但没人看」的设备：看门狗据此让推流端停掉 ffmpeg
    ///
    /// 只算**曾经被要求起流**的房间（[`VideoHub::mark_streaming`]）：板端 relay 的六条
    /// 连接平时一直挂着，不能让它们每 15s 被停一次。
    /// 已经没人看、且空闲超过 `grace` 的房间（看门狗据此下发 `video_stop`）。
    ///
    /// 宽限由调用方传入（看门狗用 [`STREAM_IDLE_GRACE`]，单测里可以传 0 / 很长），
    /// 避免「刚走开两秒就把推流杀掉」。
    pub async fn idle_publishers(&self, grace: std::time::Duration) -> Vec<String> {
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
            .filter(|device_id| {
                rooms
                    .get(*device_id)
                    .and_then(|room| room.idle_since)
                    .is_some_and(|since| since.elapsed() >= grace)
            })
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
            info!("video publisher {device_id} 断开，房间已丢弃");
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
            // 只有「上一轮真的在播」才需要踢人。冷启动时这条 `video_reset` 是**刚进来的
            // 观看者自己触发的那一次**（平台下发 video_start → 板端起 ffmpeg 前先 reset），
            // 把人家踢掉纯属多余：前端 250ms 后会重连，但首帧凭空慢了一截（2026-11-18
            // 发布后 `measure_bitrate` 探针直接被 1006 断掉，就是这么来的）。
            let was_live = room.init.is_some() || room.last_frame.is_some();
            room.forget_stream_bytes();
            room.last_frame = None;
            // 时间轴重新开始，上一轮的「落后多少秒」没意义了
            room.viewer_lag_ms.store(0, Ordering::Relaxed);
            if was_live {
                room.reset.notify_waiters();
            }
        }
    }

    /// 取「新一轮流」的通知器：观看端在 `select!` 里等它，被叫醒就断开重连
    pub async fn reset_signal(&self, device_id: &str) -> Option<Arc<tokio::sync::Notify>> {
        self.rooms
            .read()
            .await
            .get(device_id)
            .map(|room| room.reset.clone())
    }

    /// 取「观看端落后直播」的记录器（毫秒）：`handle_viewer` 每收一片就把实测值写回去
    ///
    /// 拿的是 `Arc`，观看端循环里不需要每片都去抢房间锁。
    pub async fn lag_meter(&self, device_id: &str) -> Option<Arc<AtomicU64>> {
        self.rooms
            .read()
            .await
            .get(device_id)
            .map(|room| room.viewer_lag_ms.clone())
    }

    /// 诊断快照：每个房间的推流 / 观看 / 起流状态（`GET /api/backpack/video/state`）
    ///
    /// 现场排障靠这个："为什么这一路黑屏"常常是观看者计数没清、或者流头被清了。
    pub async fn debug_state(&self) -> serde_json::Value {
        let rooms = self.rooms.read().await;
        let viewers = self.viewers.read().await;
        let streaming = self.streaming.read().await;
        let controls = self.controls.read().await;
        let mut list: Vec<serde_json::Value> = rooms
            .iter()
            .map(|(key, room)| {
                json!({
                    "room": key,
                    "publisher": room.publisher.is_some(),
                    "initBytes": room.init.as_ref().map(|init| init.len()).unwrap_or(0),
                    // 补流用的「当前 GOP」大小 / 它是多久前开始的（大 = 一个 GOP 太长；
                    // null = 还没攒出 GOP，中途加入的观看者目前只能用本轮第一个关键帧兜底）
                    "gopBytes": room.gop.len(),
                    "gopAgeSecs": room.gop_at.map(|at| at.elapsed().as_secs()),
                    "pendingBytes": room.pending.len(),
                    "viewers": viewers.get(key).copied().unwrap_or(0),
                    // 秒；null = 这一轮还没收到过字节。持续变大就是「推流端没数据」
                    "lastFrameAgeSecs": room.last_frame.map(|at| at.elapsed().as_secs()),
                    // 秒；观看端落后直播多少（>2s 会被主动断开重连）。0 = 不知道或没观看者
                    "viewerLagSecs": room.viewer_lag_ms.load(Ordering::Relaxed) as f64 / 1000.0,
                    "streaming": streaming.contains(key),
                    "control": controls.contains_key(key),
                })
            })
            .collect();
        list.sort_by_key(|item| item["room"].as_str().unwrap_or_default().to_string());
        json!({ "rooms": list })
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

/// 解析 FLV 开头：`(header 长度, 流头长度)`
///
/// - `header` = `FLV header(13) + onMetaData + AVC 序列头`（不含关键帧）—— 补当前 GOP 时
///   要接在它后面，浏览器必须有 FLV 魔数和 SPS/PPS；
/// - `init` = `header` + 首个关键帧 tag（中途加入的观看者靠它解码；也是 GOP 攒不出来时的兜底）。
fn flv_init_span(buf: &[u8]) -> Option<(usize, usize)> {
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
                return Some((pos, body + size + 4));
            }
        }
        pos = body + size + 4;
    }
}

/// 一个 **tag 对齐**的字节段是不是「视频关键帧」
///
/// AVC 的序列头（`packet_type == 0`）也是 `frameType == 1`，它不带图像，不能当 GOP 起点。
fn video_keyframe_tag(tag: &[u8]) -> bool {
    if tag.len() < 13 || tag[0] != 9 {
        return false;
    }
    let size = u32::from_be_bytes([0, tag[1], tag[2], tag[3]]) as usize;
    if size < 2 || tag.len() < 11 + size {
        return false;
    }
    let frame_type = tag[11] >> 4;
    let codec = tag[11] & 0x0f;
    let packet_type = tag[12];
    frame_type == 1 && (codec != 7 || packet_type == 1)
}

/// 增量 FLV tag 解析器：喂入字节流，回调每个**视频** tag 的时间戳
///
/// 用途只有一个：量「这个观看者落后直播多少秒」。平台给每个观看者的广播缓冲在弱网下
/// 会静默攒住几秒~几十秒的画面（不到 4MB 的环形缓冲溢出，不会有一行日志），用户只能
/// 看到「越看越慢」。 FLV 视频 tag 的时间戳是板端采集时刻（板端 ffmpeg 带
/// `-use_wallclock_as_timestamps`），所以「墙钟流逝 - 媒体流逝」的累计值就等于积压。
///
/// tag 可以跨分片（板端每次写 8KB，一个 keyframe 往往比它大），所以要留尾巴。
#[derive(Default)]
struct FlvScanner {
    /// 还没凑成完整 tag 的尾巴
    buf: Vec<u8>,
    /// 是否已经跳过 13 字节的 FLV 头（中途加入的观看者先收到流头）
    header_done: bool,
}

impl FlvScanner {
    /// 把新字节喂进去，对每个完整的视频 tag 调一次 `on_video(ts_ms)`
    fn feed(&mut self, chunk: &[u8], mut on_video: impl FnMut(u32)) {
        self.buf.extend_from_slice(chunk);
        let mut pos = 0;
        if !self.header_done {
            if self.buf.len() < 13 {
                return;
            }
            if &self.buf[..3] != b"FLV" {
                // 不是 FLV（或已经错位）：别再攒了，下一片重新对
                self.buf.clear();
                self.header_done = true;
                return;
            }
            pos = 13;
            self.header_done = true;
        }
        while self.buf.len() >= pos + 11 {
            let tag_type = self.buf[pos];
            let size =
                u32::from_be_bytes([0, self.buf[pos + 1], self.buf[pos + 2], self.buf[pos + 3]])
                    as usize;
            // 时间戳：低 24 位 + 扩展字节（第 4 个字节是高位）
            let ts = u32::from_be_bytes([
                self.buf[pos + 7],
                self.buf[pos + 4],
                self.buf[pos + 5],
                self.buf[pos + 6],
            ]);
            let end = pos + 11 + size + 4;
            if self.buf.len() < end {
                break;
            }
            // 9 = 视频 tag；脚本 tag(18) / 音频 tag(8) 与积压无关
            if tag_type == 9 {
                on_video(ts);
            }
            pos = end;
        }
        if pos > 0 {
            self.buf.drain(..pos);
        }
        // 一直凑不出完整 tag（坏流 / 错位）：丢掉，别把内存吃满
        if self.buf.len() > MAX_FLV_TAG_BYTES {
            self.buf.clear();
        }
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
            for room in state.video_hub.idle_publishers(STREAM_IDLE_GRACE).await {
                // 板端推流房间（`bp:`）的控制通道直连它的推流连接
                let stop = json!({"msgType": "video_stop", "payload": {}}).to_string();
                if !state.video_hub.send_control(&room, stop).await {
                    debug!("relay room {room}: 推流端已断开，跳过 video_stop");
                }
                // 停流后清掉上一轮的流头：否则下一个观看者会先收到一段旧 FLV 头 +
                // 旧关键帧（画面卡住 / 花屏），还可能被「有 init 就别起流」的旧判断误导。
                state.video_hub.reset_stream(&room).await;
                state.video_hub.mark_streaming(&room, false).await;
                info!("board relay {room} has no viewer, stopping stream");
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
/// 没有通道就永久挂起，让 `select!` 只等推流数据；
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
                if last_seen.elapsed() > PUBLISHER_SILENCE {
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
                                info!("video publisher {code} attached");
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
                                    info!("video publisher {code} stream reset（新一轮 ffmpeg）");
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
        info!("video publisher {code} detached after {frames} frames");
        state.video_hub.unregister_control(&code, session).await;
        state.video_hub.publisher_gone(&code, session).await;
        // 房间已经没了：顺手把「起过流」标记清掉，看门狗不用再盯着这条死链
        state.video_hub.mark_streaming(&code, false).await;
    }
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

    Ok(ws.on_upgrade(move |socket| handle_viewer(socket, state, stream, room)))
}

/// 诊断：`GET /api/backpack/video/state` —— 每个视频房间的推流 / 观看 / 起流状态
pub async fn viewer_state(
    State(state): State<AppState>,
    Extension(_claims): Extension<Claims>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    Ok(ApiResponse::ok(state.video_hub.debug_state().await))
}

/// 等「推流端开启新一轮流」的信号；房间没有通知器时永远等下去（不占 CPU）
async fn wait_for_reset(reset: Option<&tokio::sync::Notify>) {
    match reset {
        Some(notify) => notify.notified().await,
        None => std::future::pending().await,
    }
}

/// 观看者的一条中继流（背包板端推流房间 `bp:<stream>`）
///
/// 房间与控制消息都挂在板端那条常驻推流连接上（`handle_publisher` 的 `controls`）。
async fn handle_viewer(socket: WebSocket, state: AppState, stream_name: String, room: String) {
    let (mut sink, mut stream) = socket.split();
    let (mut rx, first, catch_up) = state.video_hub.viewer_join(&room).await;
    // 推流端换新一轮流时要断开（拿不到流头就是「画面定住但字节还在流」）
    let reset = state.video_hub.reset_signal(&room).await;

    // 「落后直播」量化器（每个观看者一份）：FLV 视频 tag 的时间戳是板端采集时刻，
    // 所以「墙钟流逝 - 媒体流逝」的累计值 ≈ 这个观看者手上是多旧的画面。
    //
    // 为什么必须有：平台给每个观看者的广播缓冲是**静默**的 —— 弱网时它能攒住十几秒
    // 画面而一行日志都不打（只有 4MB 环形缓冲溢出才报 `Lagged`），用户只会觉得
    // 「越看越慢」。2026-11-18 现场：上行抖 45s → 板端 ffmpeg 恢复后以 3 倍速把积压
    // 倒出来 → 观众端缓冲吃下十几秒（日志 `6.9s / 2964 KB → 3533 kbps`）→ 一直慢着。
    let lag_meter = state.video_hub.lag_meter(&room).await;
    let mut observe = {
        let mut scanner = FlvScanner::default();
        let mut prev: Option<(std::time::Instant, u32)> = None;
        let mut lag = 0.0_f64;
        // 返回「到目前为止落后直播多少秒」（累计值 = 墙钟流逝 - 媒体流逝）
        move |bytes: &[u8], now: std::time::Instant| -> f64 {
            scanner.feed(bytes, |ts| {
                if let Some((last_at, last_ts)) = prev {
                    let d_wall = now.saturating_duration_since(last_at).as_secs_f64();
                    let d_ts = ts.wrapping_sub(last_ts) as f64 / 1000.0;
                    // 时间轴跳变（补的流头 → 直播帧、板端换档重启）不是积压
                    if d_ts.abs() <= FLV_TS_JUMP_SECS {
                        // **不要每步都 clamp 到 0**：零均值的抖动被单向截断会变成系统性上偏
                        // （12 片/秒 × 每秒多算几十毫秒 = 几秒就误报），所以允许负值（追上了
                        // 就是负的），只把「信用卡」封顶在 -2s，避免一场追赶把真积压遮掉。
                        lag = (lag + d_wall - d_ts).max(-VIEWER_LAG_DROP);
                    }
                }
                prev = Some((now, ts));
            });
            lag
        }
    };

    // 补流：**只要房间里有当前这一轮的字节就补**，不分是不是第一个观看者。
    //
    // 补的内容是「FLV 头 + AVC 配置 + **当前 GOP**」（见 `Room::catch_up`）：
    // 早先补的是「本轮第一个关键帧」，时间戳是本轮开头（ts≈0），而直播帧已经跑到
    // 几百秒 —— 浏览器 MSE 里就多一个几百秒的空洞，且紧跟的 P 帧缺参考帧，
    // 现场就是「字节在流、画面定住」（2026-11-18 21:53：补的 ts=0、直播 ts=435）。
    //
    // 流头不会串轮次：板端 relay 每次起 ffmpeg 前发 `video_reset`，推流端重连时
    // 房间也会清掉流头与 GOP，两种情况都会让旧字节失效。
    if let Some(catch_up) = catch_up {
        // 补的字节也要喂给量化器：它的关键帧是基准点，不喂就从中段字节开始解析（错位）
        observe(&catch_up, std::time::Instant::now());
        if sink
            .send(Message::Binary(catch_up.as_ref().to_vec().into()))
            .await
            .is_err()
        {
            if state.video_hub.viewer_leave(&room).await {
                // 不立刻停流：留给看门狗（宽限期内重看秒开）
                state.video_hub.mark_streaming(&room, true).await;
            }
            return;
        }
    }

    // **每个「第一个观看者」都发 video_start**（不按 init / publisher 猜流在不在跑）。
    //
    // 旧逻辑是「房间里有流头且有推流端就不发」，但流头是上一轮留下的：
    // 最后一个人看完 → 看门狗 video_stop → 板端杀掉 ffmpeg，房间的 `init` 还在。
    // 下一个观看者进来时看到「有 init + publisher 在线」→ 不发 video_start →
    // 板端永远不会重新起流，浏览器只能收到 40KB 旧流头，20s 连接超时后重连，
    // 于是每 23s 断一次（2026-11-18 现场：`last viewer ... left` 精确 23s 一次）。
    //
    // 重复发是安全的：板端 relay 的 `StreamSession.start()` 是幂等的
    // （`if self.running: return`），已经在推的流不会重启。
    if first {
        info!("backpack stream {stream_name} requested, asking board relay to start");
        // 带上平台配置的默认档位（系统设置 → 视频中继）：板端起流就按它选参数，
        // 不用登到板子上改 `RELAY_PROFILE` 再重启服务。老板端忽略这个字段，
        // 仍按自己的环境变量走（向后兼容）。
        let profile = relay_profile(&state).await;
        let sent = state
            .video_hub
            .send_control(
                &room,
                json!({
                    "msgType": "video_start",
                    "payload": {
                        "stream": stream_name,
                        "format": "flv",
                        "profile": profile,
                    }
                })
                .to_string(),
            )
            .await;
        if sent {
            // 只有真的让推流端起过流，才需要让看门狗盯着它
            state.video_hub.mark_streaming(&room, true).await;
        } else {
            warn!("viewer of {room}: video_start 没送出去（推流端未连接）");
        }
    }

    debug!("viewer of {room} attached (first={first})");
    // 观看端吞吐统计：排「中继很卡」时，这行日志能直接看出是码率超过观看端带宽
    // （例如 1080p60 copy ≈ 4~8Mbps，而平台公网出口只有 2~3Mbps）。
    let started = std::time::Instant::now();
    let mut forwarded: u64 = 0;
    loop {
        tokio::select! {
            frame = rx.recv() => match frame {
                Ok(bytes) => {
                    let received_at = std::time::Instant::now();
                    let lag = observe(&bytes, received_at);
                    forwarded += bytes.len() as u64;
                    if sink.send(Message::Binary(bytes.as_ref().to_vec().into())).await.is_err() {
                        break;
                    }
                    if let Some(meter) = &lag_meter {
                        meter.store((lag.max(0.0) * 1000.0) as u64, Ordering::Relaxed);
                    }
                    // 落后直播太多就断开重连（前端 250ms 后重连，补的流头就是直播边缘）。
                    // 不断开的话这个观看者会一直「慢几秒」地看下去 —— 没有任何日志会提醒
                    // 任何人，UI 上也看不出来（画面很流畅，只是比现场晚）。
                    if lag > VIEWER_LAG_DROP {
                        info!(
                            "viewer of {room} 落后直播 {lag:.1}s（缓冲积压），断开让前端重连到直播边缘"
                        );
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // 追不上就**关连接**，别从中途丢帧继续发：FLV 是「tag 头 + tag 体」
                    // 的连续字节流，丢掉任意一段之后后面全部错位，flv.js 只会报解析错误
                    // 然后黑屏重连。断掉让浏览器重连，服务端会把流头 + 关键帧重发给他。
                    warn!(
                        "viewer of {room} lagged, dropped {skipped} frames after {:.1}s / {} KB（推流码率超过观看端带宽）; closing for a clean restart",
                        started.elapsed().as_secs_f64(),
                        forwarded / 1024
                    );
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("viewer of {room}: room closed");
                    break;
                }
            },
            _ = wait_for_reset(reset.as_deref()) => {
                info!(
                    "viewer of {room} 推流端开启新一轮流（video_reset），断开让前端重连拿新流头"
                );
                break;
            }
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
        // 不立刻停（切源无感）：保留 streaming 标记，交给看门狗在宽限后 video_stop
        let seconds = started.elapsed().as_secs_f64().max(0.1);
        info!(
            "last viewer of {room} left after {seconds:.1}s / {} KB（平均 {:.0} kbps）",
            forwarded / 1024,
            forwarded as f64 * 8.0 / seconds / 1000.0
        );
        state.video_hub.mark_streaming(&room, true).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{FlvScanner, VideoHub, flv_init_span, viewer_buffer_frames};
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
        assert_eq!(flv_init_span(&stream), Some((13 + 20, INIT_LEN)));
        // 只给到关键帧 tag 的一半：还解析不出来
        assert_eq!(flv_init_span(&stream[..INIT_LEN - 3]), None);
        assert_eq!(flv_init_span(b"not a flv stream"), None);
    }

    /// 造一段带时间戳的 FLV：header + AVC 序列头 + `frames`（每项 `(时间戳 ms, 是否关键帧)`）
    fn flv_stream_with(frames: &[(u32, bool)]) -> Vec<u8> {
        fn tag(tag_type: u8, ts: u32, body: &[u8]) -> Vec<u8> {
            let mut out = vec![tag_type];
            out.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
            out.extend_from_slice(&ts.to_be_bytes()[1..]); // 时间戳低 24 位
            out.push((ts >> 24) as u8); // 时间戳高位
            out.extend_from_slice(&[0, 0, 0]); // streamId
            out.extend_from_slice(body);
            out.extend_from_slice(&(body.len() as u32 + 11).to_be_bytes());
            out
        }
        let mut out = b"FLV\x01\x05".to_vec();
        out.extend_from_slice(&[0, 0, 0, 9]); // DataOffset = 9
        out.extend_from_slice(&[0, 0, 0, 0]); // PreviousTagSize0
        out.extend(tag(9, 0, &[0x17, 0x00, 0x00, 0x00, 0x00])); // AVC 序列头
        for (ts, keyframe) in frames {
            let head = if *keyframe { 0x17 } else { 0x27 };
            out.extend(tag(9, *ts, &[head, 0x01, 0x00, 0x00, 0x00]));
        }
        out
    }

    /// 把一段 FLV 里的视频 tag 解成 `(时间戳 ms, 是否关键帧)`（序列头算 `(0, false)`）
    fn video_tags(buf: &[u8]) -> Vec<(u32, bool)> {
        let mut out = Vec::new();
        let mut pos = 13;
        while pos + 11 <= buf.len() {
            let size = u32::from_be_bytes([0, buf[pos + 1], buf[pos + 2], buf[pos + 3]]) as usize;
            let end = pos + 11 + size + 4;
            if end > buf.len() {
                break;
            }
            if buf[pos] == 9 && size >= 2 {
                let ts = u32::from_be_bytes([buf[pos + 7], buf[pos + 4], buf[pos + 5], buf[pos + 6]]);
                let keyframe = (buf[pos + 11] >> 4) == 1 && buf[pos + 12] == 1;
                out.push((ts, keyframe));
            }
            pos = end;
        }
        out
    }

    /// 中途加入的观看者必须补「**当前 GOP**」，而不是本轮第一个关键帧。
    ///
    /// 2026-11-18 21:53 现场：补的流头 ts=0，直播帧已经 ts=435s —— MSE 里多出 435 秒
    /// 空洞，且紧跟着的 P 帧缺参考帧，浏览器就是「字节还在流、画面定住」。
    #[tokio::test]
    async fn late_viewer_gets_the_current_gop_not_the_round_first_keyframe() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 1).await;
        // 两个 GOP：ts 0/40/80，然后关键帧 2000 起新 GOP
        let stream = flv_stream_with(&[(0, true), (40, false), (80, false), (2000, true), (2040, false)]);
        hub.publish("bp:dogcam", Arc::from(stream)).await;

        let (_rx, _, catch_up) = hub.viewer_join("bp:dogcam").await;
        let catch_up = catch_up.expect("同一条流的中途加入应补字节");
        assert_eq!(&catch_up[..3], b"FLV", "补的字节要从 FLV 魔数开始");
        assert_eq!(
            video_tags(&catch_up),
            vec![(0, false), (2000, true), (2040, false)],
            "序列头 + 最近一个 GOP（从 2000 的关键帧开始）"
        );
    }

    /// 这一轮只有一个 GOP（刚起流）时，补的就是「本轮第一个关键帧」—— 旧行为仍可用
    #[tokio::test]
    async fn catch_up_with_a_single_gop_starts_at_the_round_keyframe() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 1).await;
        hub.publish("bp:dogcam", Arc::from(flv_stream_with(&[(0, true), (40, false)])))
            .await;

        let (_rx, _, catch_up) = hub.viewer_join("bp:dogcam").await;
        assert_eq!(
            video_tags(&catch_up.expect("应补字节")),
            vec![(0, false), (0, true), (40, false)]
        );
    }

    /// 换一轮流（ffmpeg 重启 / 推流端重连）时 GOP 也要清掉，不能把上一轮的帧补出去
    #[tokio::test]
    async fn new_round_forgets_the_gop() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 1).await;
        hub.publish("bp:dogcam", Arc::from(flv_stream_with(&[(0, true), (40, false)])))
            .await;
        let state = hub.debug_state().await;
        assert!(state["rooms"][0]["gopBytes"].as_u64().unwrap() > 0, "应攒出 GOP");

        hub.reset_stream("bp:dogcam").await;
        assert_eq!(hub.debug_state().await["rooms"][0]["gopBytes"], 0);
        let (_rx, _, catch_up) = hub.viewer_join("bp:dogcam").await;
        assert!(catch_up.is_none(), "新一轮还没字节，不能补上一轮的");
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

    /// 空转判定：起过流 + 没人看 + 空闲超过宽限。**宽限是必须的**：
    /// 浏览器刷新 / 切换页面只断几秒，立刻停流会让下一个观看者重新等 ffmpeg + 关键帧
    /// （现场「视频连接很慢」就是这么来的）。
    #[tokio::test]
    async fn publisher_without_viewer_is_reported_idle_only_after_grace() {
        use std::time::Duration;
        let grace = Duration::from_secs(60);
        let hub = VideoHub::default();
        hub.publisher_attach("RD-001", 1).await;
        // 没见过 video_start 的房间不算空转（板端 relay 的常驻连接就属于这种）
        assert!(hub.idle_publishers(grace).await.is_empty());

        hub.mark_streaming("RD-001", true).await;
        // 推流端自己在推、还没人看过：没有 idle_since，不归看门狗停
        assert!(
            hub.idle_publishers(Duration::ZERO).await.is_empty(),
            "没人看过就谈不上「看完走了」"
        );

        let (_rx, _, _) = hub.viewer_join("RD-001").await;
        assert!(
            hub.idle_publishers(Duration::ZERO).await.is_empty(),
            "有观看者时不算空转"
        );
        assert!(hub.viewer_leave("RD-001").await, "最后一个观看者离开");

        assert!(
            hub.idle_publishers(grace).await.is_empty(),
            "宽限内不许停流（短时重连要能立刻复用）"
        );
        assert_eq!(
            hub.idle_publishers(Duration::ZERO).await,
            vec!["RD-001".to_string()],
            "宽限到期后看门狗应能发现"
        );

        // 又有人回来看了：空闲计时清零
        let (_rx2, _, _) = hub.viewer_join("RD-001").await;
        assert!(hub.idle_publishers(Duration::ZERO).await.is_empty());
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
        // 现在不再销毁房间（见 `publisher_reconnect_keeps_the_room_and_signals_a_new_stream`）：
        // 观看端拿到的是「新一轮流」通知，行为与 `video_reset` 一致。
        let hub = VideoHub::default();
        let (mut rx, _, _) = hub.viewer_join("RD-001").await;
        hub.publisher_attach("RD-001", 1).await;
        hub.publish("RD-001", Arc::from(flv_stream())).await;
        assert!(rx.recv().await.is_ok(), "重连前观看端应正常工作");

        hub.publisher_attach("RD-001", 2).await;
        // 广播通道没有被关掉：房间还在，旧字节之后不会再来新字节（等观看端被 reset 叫醒）
        assert!(
            !matches!(
                rx.try_recv(),
                Err(tokio::sync::broadcast::error::TryRecvError::Closed)
            ),
            "重连不应销毁房间、关掉观看端"
        );
    }

    /// 推流端重连（弱网抖动）要**沿用房间**，只换会话号 + 通知观看端「换新流了」。
    ///
    /// 以前是 `rooms.insert(new_room)` 整间换掉：广播通道被 close，正在看的浏览器直接
    /// 被踢；而旧连接常常是半开僵尸（要到 `PUBLISHER_SILENCE` 才判死），新连接一 attach
    /// 就踢人 —— 2026-11-18 20:22:36 / 20:23:32 连踢两次，现场就是「看着看着黑一下」。
    #[tokio::test]
    async fn publisher_reconnect_keeps_the_room_and_signals_a_new_stream() {
        let hub = VideoHub::default();
        let (mut rx, _, _) = hub.viewer_join("RD-001").await;
        hub.publisher_attach("RD-001", 1).await;
        let stream = flv_stream();
        hub.publish("RD-001", Arc::from(stream.clone())).await;
        assert_eq!(
            rx.recv().await.expect("应收到流头").as_ref(),
            &stream[..INIT_LEN]
        );

        // 观看端挂着「新一轮流」的通知器（真实链路上是 `handle_viewer` 的 select 分支）
        let reset = hub.reset_signal("RD-001").await.expect("房间应还在");
        let waiter = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(2), reset.notified())
                .await
                .is_ok()
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        hub.publisher_attach("RD-001", 2).await;

        assert!(
            waiter.await.expect("等待任务不能 panic"),
            "重连要通知观看端「换新流」，不能靠销毁房间"
        );
        assert!(hub.has_publisher("RD-001").await, "房间要沿用，不能重建");
        assert_eq!(hub.viewers("RD-001").await, 1, "观看计数不能被清掉");
        let (_rx2, first, init) = hub.viewer_join("RD-001").await;
        assert!(!first, "房间沿用，第二个观看者仍不该触发 video_start");
        assert!(init.is_none(), "重连后不能补上一轮的关键帧");
    }

    /// 背包房间的环形缓冲要比设备客户端房间浅：128 片 = 1MB ≈ 16 秒 @500k，
    /// 而 4MB 那套能在没人察觉的情况下攒住 **65 秒**画面（2026-11-18「越看越慢」的根源）。
    #[test]
    fn backpack_rooms_get_a_shallower_viewer_buffer() {
        assert_eq!(viewer_buffer_frames("bp:dogcam"), 128);
        assert_eq!(viewer_buffer_frames("RD-001"), 512);
    }

    /// 造一段带指定时间戳的 FLV：header + 每帧一个视频 tag
    fn flv_tags(timestamps: &[u32]) -> Vec<u8> {
        fn tag(ts: u32, body: &[u8]) -> Vec<u8> {
            let mut out = vec![9_u8];
            let size = (body.len() as u32).to_be_bytes();
            out.extend_from_slice(&size[1..]);
            // FLV 时间戳：低 24 位大端 + 扩展字节（高位）
            out.extend_from_slice(&[
                (ts >> 16) as u8,
                (ts >> 8) as u8,
                ts as u8,
                (ts >> 24) as u8,
            ]);
            out.extend_from_slice(&[0, 0, 0]); // streamId
            out.extend_from_slice(body);
            out.extend_from_slice(&((body.len() as u32 + 11).to_be_bytes()));
            out
        }
        let mut out = b"FLV\x01\x05".to_vec();
        out.extend_from_slice(&[0, 0, 0, 9]);
        out.extend_from_slice(&[0, 0, 0, 0]);
        for ts in timestamps {
            out.extend(tag(*ts, &[0x27, 0x01, 0x00, 0x00, 0x00]));
        }
        out
    }

    /// 量化器要能跨分片拼 tag（板端每次写 8KB，keyframe 往往比它大），只认视频 tag
    #[test]
    fn flv_scanner_reports_video_timestamps_across_chunks() {
        let stream = flv_tags(&[0, 100, 200, 300]);
        let mut scanner = FlvScanner::default();
        let mut seen = Vec::new();
        for chunk in stream.chunks(7) {
            scanner.feed(chunk, |ts| seen.push(ts));
        }
        assert_eq!(seen, vec![0, 100, 200, 300], "跨分片的 tag 一个都不能丢");

        // 不是 FLV（或者已经错位）时不能 panic，也不能报假时间戳
        let mut scanner = FlvScanner::default();
        let mut none = Vec::new();
        scanner.feed(b"not a flv stream at all", |ts| none.push(ts));
        assert!(none.is_empty());
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
        assert!(
            hub.viewer_leave("bp:dogcam").await,
            "应判定为最后一个观看者"
        );
        assert!(
            hub.has_publisher("bp:dogcam").await,
            "推流端还在就不能丢房间"
        );

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

    /// 后加入的观看者拿到的补流以「本轮第一个关键帧」开头（这一轮只有一个 GOP 时，
    /// 它同时也是当前 GOP 的起点），后面会接着这一轮已经推过的帧。
    #[tokio::test]
    async fn late_viewer_gets_init_prefix() {
        let hub = VideoHub::default();
        let (_first_rx, _, _) = hub.viewer_join("RD-001").await;
        let stream = flv_stream();
        hub.publish("RD-001", Arc::from(stream.clone())).await;

        let (_rx, _, catch_up) = hub.viewer_join("RD-001").await;
        let catch_up = catch_up.expect("后加入的观看者应拿到补流字节");
        assert!(
            catch_up.starts_with(&stream[..INIT_LEN]),
            "必须以 FLV 头 + AVC 序列头 + 本轮关键帧开头"
        );
        // 流量后面接着本 GOP 的 P 帧（时间轴连续，不是只给一个孤立关键帧）
        assert_eq!(catch_up.len(), stream.len());
        assert_eq!(catch_up.as_ref(), &stream[..]);
    }

    /// 板端 relay 房间（`bp:`）：控制消息通过推流连接下发，且旧连接迟到的清理
    /// 不能把新连接的通道删掉（否则重连后观看端再也起不了流）。
    #[tokio::test]
    async fn board_relay_control_channel_follows_the_live_session() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 7).await;
        assert!(
            hub.has_publisher("bp:dogcam").await,
            "relay 常驻连接应算在线"
        );

        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        hub.register_control("bp:dogcam", 7, tx).await;
        assert!(
            hub.send_control("bp:dogcam", "video_start".to_string())
                .await,
            "板端房间应能收到控制消息"
        );
        assert_eq!(rx.recv().await.as_deref(), Some("video_start"));

        // 旧会话（6）迟到的清理不应动到当前会话（7）的通道
        hub.unregister_control("bp:dogcam", 6).await;
        assert!(hub.send_control("bp:dogcam", "still".to_string()).await);

        hub.unregister_control("bp:dogcam", 7).await;
        assert!(!hub.send_control("bp:dogcam", "gone".to_string()).await);
    }

    /// 排障快照要能把「推流端在线但不动」和「观看端没收到」分开：
    /// `lastFrameAgeSecs` 持续变大 = 板端没有新字节（Wi-Fi 掉线 / 源卡住）。
    #[tokio::test]
    async fn debug_state_reports_last_frame_age() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 1).await;
        let state = hub.debug_state().await;
        assert!(
            state["rooms"][0]["lastFrameAgeSecs"].is_null(),
            "本轮还没推出字节时应为 null"
        );

        hub.publish("bp:dogcam", Arc::from(flv_stream())).await;
        let state = hub.debug_state().await;
        assert!(
            state["rooms"][0]["lastFrameAgeSecs"].as_u64().is_some(),
            "推过帧之后应给出距最后一帧的秒数"
        );
    }

    /// 设备客户端的房间没有控制通道（控制走设备网关），看门狗与观看端据此分流。
    #[tokio::test]
    async fn device_room_has_no_control_channel() {
        let hub = VideoHub::default();
        hub.publisher_attach("RD-001", 1).await;
        assert!(!hub.send_control("RD-001", "video_stop".to_string()).await);
        assert!(hub.has_publisher("RD-001").await);
        assert!(
            !hub.has_publisher("bp:cam0").await,
            "没有推流端时不应报在线"
        );
    }
}
