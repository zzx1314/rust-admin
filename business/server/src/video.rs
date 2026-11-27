//! 背包实时视频的**控制面**（按需起停 + 档位下发 + 观看者计数）
//!
//! 媒体**不经过本模块**：板端 relay 用 RTSP/TCP 把流转推到平台 mediamtx
//! （path 名 `bp_<stream>`），浏览器经 `/api/backpack/whep/<stream>` 同源反代用 WHEP
//! 拉 WebRTC（见 [`crate::whep`]）。本模块只剩下一条常驻 WS：
//!
//! ```text
//! 板端 relay ──ws://平台/ws/device/video──► 业务平台（房间 bp:<stream>，只有文本帧）
//! 浏览器 ──ws://平台/api/backpack/video/ws──► 同上（**只计数**，一个媒体字节都不转发）
//! 浏览器 ⇄ 平台 mediamtx（UDP 8189，SRTP）
//! ```
//!
//! 多条设备（多台背包）时房间名 / mediamtx path 都带设备维度：
//! `bp:<device>:<stream>` / `bp_<device>_<stream>`（见 [`room_of`] / [`mediamtx_path_of`]）。
//! 不配设备号的板子仍是 `bp:<stream>` / `bp_<stream>`，升级不会影响现场已部署的单机。
//!
//! 三条约定：
//!
//! 1. **按需推流**：第一个观看者进来才给推流端下发 `video_start`，最后一个离开后留
//!    [`STREAM_IDLE_GRACE`] 宽限再由看门狗 `video_stop`，没人看的时候现场侧不耗 CPU / 带宽。
//! 2. **观看通道只存在性**：`/api/backpack/video/ws` 不再发二进制帧。没有它就没有
//!    「第一个观看者」→ 平台不下发 `video_start`，看门狗也会在宽限后把流停掉。
//! 3. **房间名 = 设备 + 通道**：板端推流没有设备台账，房间名用
//!    [`BACKPACK_ROOM_PREFIX`]（`bp:<stream>` / `bp:<device>:<stream>`），控制消息直接走
//!    它自己那条连接（文本帧），见 `handle_publisher` 里的 `controls`。
//!
//! 历史：2026-11-19 之前媒体走「HTTP-FLV over WebSocket」（平台环形缓冲扇出给 flv.js），
//! 那一整套（补流头 / 当前 GOP / 积压量化 / 滞后断开 / `media=` 开关）已随 WebRTC 出口
//! 上线**整体删除**。别再把它加回来：`business/docs/backpack-video-integration.md` §6
//! 记着那套设计的每一个坑（补头错位、静默积压、时间轴跳变……），而 §7 是现在的链路。

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
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};

/// 「最后一个观看者离开」到真正停流之间的宽限。
///
/// 现场痛点：浏览器刷新 / 切换看板 / 弱网重连都只断几秒，所以「最后一个观看者离开」
/// 之后不能立刻停流，否则下一个观看者进来又要重新起 ffmpeg + 等关键帧 —— 现场感受就是
/// 「视频连接特别慢、总要黑一下」。
///
/// 但也不能太长：没人在看也一直在编码，机器狗的电池 / CPU / 4G 流量都在白烧（现场开了
/// 代理后「看一会儿就掉电」很难解释）。
///
/// 15s（2026-11-18 从 60s 收下来的值）在现场**偏紧**：手机切个页 / 看板上翻两屏 / 网络
/// 抖一下就超过 15s，回来时 `video_stop` 已经下发，板端 ffmpeg 被杀 → 重新走一遍冷启动
/// （ffmpeg + 等关键帧 1~7s，低档位 GOP 更长），现场就是「起流黑好大一会」。
/// 30s 能盖住这类日常操作，代价是白编码最多多 15s；再往上就没必要了。
const STREAM_IDLE_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// 推流端多久没有任何消息（WS ping / 文本帧）就判死。
///
/// 现场板子出公网走 Wi-Fi / 4G，掉线后到平台的 TCP 常常是半开：不 FIN、不 RST，平台会
/// 一直以为「推流端在线」，观看者进来永远等不到画面（以前要 60s 才判死，画面就冻住
/// 1 分钟）。板端 relay 每 10s 一个心跳，这里 30s 即 3 次没到，足够保守也不会误杀。
const PUBLISHER_SILENCE: std::time::Duration = std::time::Duration::from_secs(30);

/// 推流会话自增号（见 [`VideoHub::publisher_attach`]）
static PUBLISHER_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 背包板端推流房间的房间名前缀（`bp:<stream>`）
///
/// 板端推流服务没有设备台账，房间名统一带上这个前缀：带前缀的房间在
/// `handle_publisher` 里会挂上**控制通道**，`video_start` / `video_stop` /
/// `video_config` 直接写进它自己的推流连接（不经过设备网关）。
pub const BACKPACK_ROOM_PREFIX: &str = "bp:";

/// 平台 mediamtx 上的 path 名前缀（`bp_dogcam` / `bp_cam0` …）
///
/// 与 [`BACKPACK_ROOM_PREFIX`] 是两套命名空间，别混：房间名是平台的（带冒号，走 WS
/// 控制通道），path 名是 mediamtx 的（下划线，走 RTSP/WebRTC —— RTSP 的 URL 里冒号要
/// 转义，且 mediamtx 的 path 不允许冒号）。
///
/// `deploy/external-hz/mediamtx/mediamtx.yml` 的推流鉴权就是按这个前缀放行的
/// （`path: "~^bp_"`），改这里必须同步改那份配置。
pub const BACKPACK_MEDIAMTX_PATH_PREFIX: &str = "bp_";

/// 允许通过中继观看的背包通道（与板端 mediamtx 的 path 一致）
///
/// 白名单而不是透传：通道名来自查询串，不限制就能被拿来让 mediamtx 创建任意 path。
///
/// `preview` 是板端 AI 烧框后的那一路（`osd.py` 把中文标签画进画面）。跨网段时
/// 浏览器画不了框（中继流没有坐标对应关系），只能看这一路——所以它必须能被中继。
pub const BACKPACK_STREAMS: [&str; 7] = [
    "cam0",
    "cam1",
    "cam2",
    "cam3",
    "dogcam",
    "dog-ipcam",
    "preview",
];

/// 设备编号里允许出现的字符（**不要放宽**）
///
/// 同一个值会出现在三个地方：mediamtx 的 path 片段（`bp_<device>_<stream>`）、
/// 平台房间名（`bp:<device>:<stream>`）、WHEP 反代的查询参数
/// （`/api/backpack/whep/<stream>/whep?device=<device>`）。三处的限制取交集：
/// mediamtx 的 path 不能带冒号、URL 段不能带 `/` 与 `..`、房间名用 `:` 分隔。
/// 所以只放行「字母数字开头 + 字母数字/`.`/`_`/`-`」，长度 1~32。
fn device_chars_ok(raw: &str) -> bool {
    let mut chars = raw.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    raw.len() <= 32 && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// 规范化设备编号：空 / 非法一律 `None`（= 默认设备，沿用旧命名）
///
/// 调用方**不要**自己 `trim` 后直接拼名字：非法值会污染房间名与 mediamtx path，
/// 让一台板子的推流把另一台顶掉（多设备下这是最难查的一类故障）。
pub fn normalize_device(raw: &str) -> Option<String> {
    let value = raw.trim();
    device_chars_ok(value).then(|| value.to_string())
}

/// 推流房间名：无设备号 = `bp:<stream>`（旧单机命名），有设备号 = `bp:<device>:<stream>`
///
/// 房间名是平台侧唯一的「这是哪台板子的哪一路」标识，看门狗、观看计数、
/// `video_start` / `video_stop` 全部按它找推流端，**必须带设备维度**，否则
/// 两台板子的 `cam0` 会被当成同一路（后注册的那台把控制通道接管，
/// 观看者看到的是另一台狗的画面，而被顶掉的那台永远收不到 `video_start`）。
pub fn room_of(device: Option<&str>, stream: &str) -> String {
    match device.map(str::trim).filter(|value| !value.is_empty()) {
        Some(device) => format!("{BACKPACK_ROOM_PREFIX}{device}:{stream}"),
        None => format!("{BACKPACK_ROOM_PREFIX}{stream}"),
    }
}

/// 平台 mediamtx 上的 path 名：`bp_<stream>` / `bp_<device>_<stream>`
///
/// 板端 relay 用 `RELAY_RTSP_URL` 的前缀拼这个名字推上来（见
/// `deploy/backpack-relay/relay_push.py::rtsp_publish_plan`），WHEP 反代按同一规则
/// 找它，两边**只能有一处实现**（就是这里）。
pub fn mediamtx_path_of(device: Option<&str>, stream: &str) -> String {
    match device.map(str::trim).filter(|value| !value.is_empty()) {
        Some(device) => format!("{BACKPACK_MEDIAMTX_PATH_PREFIX}{device}_{stream}"),
        None => format!("{BACKPACK_MEDIAMTX_PATH_PREFIX}{stream}"),
    }
}

/// 房间名 → `(设备编号, 通道)`；不是背包房间（没有 `bp:` 前缀）返回 `None`
///
/// 默认设备的房间没有第二段（`bp:dogcam`），设备房间是 `bp:RD-001:dogcam`。
pub fn split_room(room: &str) -> Option<(String, String)> {
    let rest = room.strip_prefix(BACKPACK_ROOM_PREFIX)?;
    Some(match rest.split_once(':') {
        Some((device, stream)) => (device.to_string(), stream.to_string()),
        None => (String::new(), rest.to_string()),
    })
}

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

/// `video_start` 控制消息。
///
/// 观看者进来、以及推流端重连后补发，都是这同一条 —— 两处各写一份 JSON 迟早会走样
/// （板端只认 `payload.stream` 和 `payload.profile`）。
fn video_start_message(stream: &str, profile: serde_json::Value) -> String {
    json!({
        "msgType": "video_start",
        "payload": {
            "stream": stream,
            "profile": profile,
        }
    })
    .to_string()
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
    // 按**已登记的控制通道**广播，而不是按固定的七路通道名：
    // 多设备部署时房间名是 `bp:<device>:<stream>`，按名字枚举会漏掉全部设备房间，
    // 现场表现就是「改了中继档位，正在看的另一台狗没换档」。
    for room in state.video_hub.control_rooms().await {
        if state.video_hub.send_control(&room, message.clone()).await {
            sent += 1;
        }
    }
    sent
}

/// 推流端控制通道表：`room -> (会话号, 文本帧发送端)`
type ControlChannels = Arc<RwLock<HashMap<String, (u64, mpsc::Sender<String>)>>>;

/// 视频房间表：一台设备一个房间（**只记状态，不搬字节**）
///
/// 媒体在 mediamtx 里，这里管的是「谁在线、谁在看、谁该起流」。
#[derive(Clone, Default)]
pub struct VideoHub {
    rooms: Arc<RwLock<HashMap<String, Room>>>,
    /// 观看者计数，**独立于房间**存在
    ///
    /// 房间会随推流端断连被丢弃（relay 抖一下）。
    /// 如果计数跟着房间走，最后一个观看者离开时就数不到人，`video_stop` 永远发不出去、
    /// `video_start` 也会在推流端重连时漏发 —— 客户端会一直重连推流、白白编码。
    viewers: Arc<RwLock<HashMap<String, usize>>>,
    /// 推流端控制通道：只有板端推流房间有（`bp:` 前缀）
    ///
    /// 值里带会话号：重连后的新连接会覆盖旧的，旧连接迟到的清理
    /// 只删自己那一份，见 `unregister_control`。
    controls: ControlChannels,
    /// 已经下发过 `video_start`、但可能没人看的房间（看门狗只盯这些）
    ///
    /// 推流端是常驻连接（板端 relay 七条），不能因为它们「有人推没人看」就每 15s
    /// 发一次 `video_stop`：那样日志会刷屏，而且真正在看的那一路也会被误杀。
    streaming: Arc<RwLock<HashSet<String>>>,
}

struct Room {
    /// 推流会话标识：`Some(id)` 表示在线，`id` 用来区分「新连接」与「旧连接迟到的清理」
    publisher: Option<u64>,
    /// 最后一个观看者离开的时刻（`None` = 还有人在看）。
    /// 看门狗只有超过 [`STREAM_IDLE_GRACE`] 才会真去停流。
    idle_since: Option<std::time::Instant>,
    /// 推流端上报的媒体通路（`video_transport`，见 `relay_push.py`）
    ///
    /// 现在只可能是 `"rtsp"`（WebRTC 出口）。留着是为了**排障与升级过渡**：
    /// `None` / `"ws"` 说明那台板子的 relay 还是老版本（媒体走已删除的 FLV 通路），
    /// 前端据此直接把话说清楚，而不是让人对着「起流中」干等。
    transport: Option<String>,
}

fn new_room(publisher: Option<u64>) -> Room {
    Room {
        publisher,
        idle_since: None,
        transport: None,
    }
}

/// 一路背包通道的对外快照（`GET /api/backpack/devices` 用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelState {
    /// 设备编号；空字符串 = 默认设备（旧命名 `bp:<stream>`）
    pub device: String,
    pub stream: String,
    /// 板端 relay 的推流连接是否在线
    pub publisher: bool,
    pub viewers: usize,
    /// `rtsp` = WebRTC 出口；`None`/`ws` = 板端 relay 没升级
    pub transport: Option<String>,
}

impl VideoHub {
    /// 观看者加入：返回是否「第一个观看者」（据此决定要不要下发 `video_start`）
    pub async fn viewer_join(&self, device_id: &str) -> bool {
        let mut viewers = self.viewers.write().await;
        let count = viewers.entry(device_id.to_string()).or_insert(0);
        *count += 1;
        let first = *count == 1;
        drop(viewers);

        let mut rooms = self.rooms.write().await;
        let room = rooms
            .entry(device_id.to_string())
            .or_insert_with(|| new_room(None));
        // 有人回来看了：空闲计时清零，看门狗别把还热着的流停掉
        room.idle_since = None;
        first
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
                }
                _ => {
                    rooms.remove(device_id);
                }
            }
        }
        last
    }

    /// 推流端注册 / 重连
    ///
    /// **房间不能整个换掉**：换掉会让 `has_publisher` 瞬间变 false、看门狗与观看端跟着抖；
    /// 而弱网重连时旧连接常常是「半开」僵尸，要到 [`PUBLISHER_SILENCE`] 才被判死，
    /// 于是新连接一 attach 就把正在看的人踢下去（2026-11-18 20:22:36 / 20:23:32 连踢两次）。
    /// 现在沿用房间、只换会话号。
    pub async fn publisher_attach(&self, device_id: &str, session: u64) {
        let mut rooms = self.rooms.write().await;
        match rooms.get_mut(device_id) {
            Some(room) => {
                if room.publisher.is_some() {
                    info!("video publisher {device_id} 重连，沿用房间");
                }
                room.publisher = Some(session);
            }
            None => {
                rooms.insert(device_id.to_string(), new_room(Some(session)));
            }
        }
    }

    /// 已经没人看、且空闲超过 `grace` 的房间（看门狗据此下发 `video_stop`）。
    ///
    /// 只算**曾经被要求起流**的房间（[`VideoHub::mark_streaming`]）：板端 relay 的七条
    /// 连接平时一直挂着，不能让它们每 15s 被停一次。
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

    /// 客户端推流连接断开：清掉房间，`has_publisher` 跟着变 false
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

    /// 当前有控制通道的房间（= 在线板端的全部通道，带设备维度）
    ///
    /// `broadcast_relay_profile` 用它广播：多设备下房间名不再是固定的七路，
    /// 按名字枚举必漏。
    pub async fn control_rooms(&self) -> Vec<String> {
        let mut rooms: Vec<String> = self.controls.read().await.keys().cloned().collect();
        rooms.sort();
        rooms
    }

    /// 所有**背包**通道的现场快照（`GET /api/backpack/devices` 用）
    ///
    /// 只看 `bp:` 房间：设备客户端那条老视频通路已经删除，它们不是背包相机。
    /// 通道名不在 [`BACKPACK_STREAMS`] 里的房间也不给前端：WHEP 反代会拒绝它
    /// （不限制就能被拿来让 mediamtx 创建任意 path）。
    pub async fn channels(&self) -> Vec<ChannelState> {
        let rooms = self.rooms.read().await;
        let viewers = self.viewers.read().await;
        let mut list: Vec<ChannelState> = rooms
            .iter()
            .filter_map(|(room, state)| {
                let (device, stream) = split_room(room)?;
                if !BACKPACK_STREAMS.contains(&stream.as_str()) {
                    return None;
                }
                Some(ChannelState {
                    device,
                    stream,
                    publisher: state.publisher.is_some(),
                    viewers: viewers.get(room).copied().unwrap_or(0),
                    transport: state.transport.clone(),
                })
            })
            .collect();
        list.sort_by(|a, b| {
            (a.device.as_str(), a.stream.as_str()).cmp(&(b.device.as_str(), b.stream.as_str()))
        });
        list
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

    /// 给推流端发一条控制消息（文本帧）；没有通道返回 false
    pub async fn send_control(&self, room: &str, message: String) -> bool {
        let controls = self.controls.read().await;
        match controls.get(room) {
            Some((_, tx)) => tx.send(message).await.is_ok(),
            None => false,
        }
    }

    /// 登记推流端上报的媒体通路（`video_transport` 消息，见 `relay_push.py`）
    pub async fn set_transport(&self, room: &str, transport: Option<String>) {
        let mut rooms = self.rooms.write().await;
        if let Some(entry) = rooms.get_mut(room) {
            entry.transport = transport;
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

    /// 诊断快照：每个房间的推流 / 观看 / 起流状态（`GET /api/backpack/video/state`）
    ///
    /// 现场排障靠这个："为什么这一路黑屏"常常是观看者计数没清、或者推流端没连上。
    pub async fn debug_state(&self) -> serde_json::Value {
        let rooms = self.rooms.read().await;
        let viewers = self.viewers.read().await;
        let streaming = self.streaming.read().await;
        let controls = self.controls.read().await;
        let mut list: Vec<serde_json::Value> = rooms
            .iter()
            .map(|(key, room)| {
                let split = split_room(key);
                json!({
                    "room": key,
                    // 设备编号 / 通道：`null` = 不是背包房间（老通路已删除）
                    "device": split.as_ref().map(|(device, _)| device.clone()),
                    "stream": split.as_ref().map(|(_, stream)| stream.clone()),
                    "publisher": room.publisher.is_some(),
                    "viewers": viewers.get(key).copied().unwrap_or(0),
                    // 秒；null = 还有人在看（或从没人看过）
                    "idleSecs": room.idle_since.map(|at| at.elapsed().as_secs()),
                    "streaming": streaming.contains(key),
                    "control": controls.contains_key(key),
                    // 板端上报的媒体通路。`rtsp` = WebRTC 出口；null/`ws` = 板端 relay 没升级
                    "transport": room.transport,
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

    /// 有人在等这一路（推流端重连后据此决定要不要**补发** `video_start`）。
    ///
    /// 观看者可能是在「上一条推流连接已经死了、平台还没判死」的空窗里进来的，它那条
    /// `video_start` 打在僵尸通道上丢了（`send_control` 只看到 sender 还在就返回成功），
    /// 板端根本没收到。媒体走 mediamtx 之后平台看不见「有没有字节」，所以判据就是
    /// 「有没有人在看」—— 板端 `StreamSession.start()` 幂等，重复下发不会重启 ffmpeg。
    pub async fn viewers_waiting(&self, device_id: &str) -> bool {
        self.viewers(device_id).await > 0
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
                let stop = json!({"msgType": "video_stop", "payload": {}}).to_string();
                if !state.video_hub.send_control(&room, stop).await {
                    debug!("relay room {room}: 推流端已断开，跳过 video_stop");
                }
                state.video_hub.mark_streaming(&room, false).await;
                info!("board relay {room} has no viewer, stopping stream");
            }
        }
    });
}

/// 板端推流通道：`GET /ws/device/video`（**只走文本帧**：注册 / 控制 / 心跳）
///
/// 首帧必须是文本 `{"msgType":"video_register","payload":{"deviceId":"bp:<stream>"}}`。
/// 媒体在 RTSP/WebRTC 那条路上，这里收到二进制帧只会被丢弃并告警一次（老板端 relay）。
pub async fn publisher_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_publisher(socket, state))
}

/// 推流端控制通道的下一条消息
///
/// 没有通道就永久挂起，让 `select!` 只等推流端的文本帧；
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
    // 客户端每 10s 一个 WS ping，30s 一个消息都没有就判死。
    let mut last_seen = std::time::Instant::now();
    let mut liveness = tokio::time::interval(std::time::Duration::from_secs(15));
    // 会话标识：同一台设备重连时，旧连接迟到的清理不会误删新房间
    let session = PUBLISHER_SESSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // 老板端还会往这条连接上推 FLV 字节：只告警一次，别每 8KB 刷一行
    let mut binary_warned = false;
    // 只有板端推流房间（`bp:`）才拿控制通道：观看者的 `video_start` / `video_stop`
    // 通过它写到这条连接上
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
                                    // 还有人在等：这个观看者的 `video_start` 可能打在了上一条
                                    // （已经死掉但平台还没判死的）推流连接上，板端根本没收到。
                                    // 这里补发一次，否则他要一直黑到前端的重连（现场＝「起流黑
                                    // 好大一会」）。板端 `StreamSession.start()` 幂等。
                                    if state.video_hub.viewers_waiting(&code).await {
                                        // 房间名带设备维度（`bp:<device>:<stream>`）：
                                        // 补发的 `video_start.stream` 只能是通道名，
                                        // 直接 strip 前缀会发出 `RD-001:cam0`，板端认不出来。
                                        let stream_name = split_room(&code)
                                            .map(|(_, stream)| stream)
                                            .unwrap_or_else(|| code.clone());
                                        let profile = relay_profile(&state).await;
                                        let resent = state
                                            .video_hub
                                            .send_control(
                                                &code,
                                                video_start_message(&stream_name, profile),
                                            )
                                            .await;
                                        if resent {
                                            info!(
                                                "video publisher {code} 重连后仍有观看者在等，补发 video_start（stream={stream_name}）"
                                            );
                                            state.video_hub.mark_streaming(&code, true).await;
                                        }
                                    }
                                }
                                device_id = Some(code);
                            }
                            // 板端起流前发一条，平台据此知道「新一轮流」开始。媒体在
                            // mediamtx 里，这里没有流头可清 —— 只留日志，排障时能对上板端
                            // 每次起 ffmpeg 的时刻。
                            "video_reset" => {
                                if let Some(ref code) = device_id {
                                    info!("video publisher {code} 新一轮流（板端起 ffmpeg）");
                                }
                            }
                            // 板端上报媒体通路。现在只可能是 rtsp（WebRTC 出口）；
                            // ws/空 = 那台板子的 relay 还是老版本，前端会据此报错。
                            "video_transport" => {
                                if let Some(ref code) = device_id {
                                    let transport = ws_msg.payload["transport"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .trim()
                                        .to_string();
                                    let transport = if transport.is_empty() {
                                        None
                                    } else {
                                        Some(transport)
                                    };
                                    match transport.as_deref() {
                                        Some("rtsp") => info!(
                                            "video publisher {code} 媒体通路 = rtsp（平台 WebRTC）"
                                        ),
                                        other => warn!(
                                            "video publisher {code} 上报的媒体通路是 {other:?}：\
                                             平台已只支持 WebRTC，这台板子的 backpack-relay 需要升级"
                                        ),
                                    }
                                    state.video_hub.set_transport(code, transport).await;
                                }
                            }
                            other => warn!("unknown video publisher message: {other}"),
                        }
                    }
                    Ok(Message::Binary(bytes)) => {
                        if !binary_warned {
                            binary_warned = true;
                            warn!(
                                "video publisher {:?} 仍在推 FLV 字节（{}B）：平台已只保留 WebRTC，\
                                 这些字节被丢弃。请把板端 backpack-relay 升级到 rtsp 通路",
                                device_id,
                                bytes.len()
                            );
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
        info!("video publisher {code} detached");
        state.video_hub.unregister_control(&code, session).await;
        state.video_hub.publisher_gone(&code, session).await;
        // 房间已经没了：顺手把「起过流」标记清掉，看门狗不用再盯着这条死链
        state.video_hub.mark_streaming(&code, false).await;
    }
}

/// 背包观看通道：`GET /api/backpack/video/ws?token=…&stream=dogcam[&device=RD-001]`
///
/// **只当在场心跳用**：一个媒体字节都不转发（媒体是浏览器 ⇄ mediamtx 的 WebRTC）。
/// 它仍然必须存在，否则：
///   - `video_start` 永远不发出，板端不会起 ffmpeg（按需推流就没了）；
///   - 看门狗数不到观看者，宽限后下发 `video_stop` 把流停掉。
///
/// `device` 是设备编号（与板端 relay 的 `RELAY_DEVICE_ID`、设备客户端的
/// `ROBODOG_DEVICE_ID` 同一个值）。不传 = 默认设备，走旧房间名 `bp:<stream>`；
/// 传了就是 `bp:<device>:<stream>` —— 同一个 `cam0` 在多台背包上是不同的房间。
///
/// 历史上这条连接承载过 FLV 媒体（`?media=1` 的老前端会这么连）；参数现在被忽略，
/// 老前端连上来只会拿到一条安静的连接，不会报错。
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
    let device = match params.get("device").map(|value| value.trim()) {
        Some(raw) if !raw.is_empty() => Some(normalize_device(raw).ok_or_else(|| {
            AppError::BadRequest(format!("设备编号不合法：{raw}（只允许字母数字与 . _ -）"))
        })?),
        _ => None,
    };
    let room = room_of(device.as_deref(), &stream);
    if !state.video_hub.has_publisher(&room).await {
        return Err(AppError::BadRequest(format!(
            "{} 的推流服务未连接：确认背包上 backpack-relay 服务在运行",
            device
                .as_deref()
                .map(|device| format!("设备 {device}"))
                .unwrap_or_else(|| "板端".to_string())
        )));
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

/// 观看者的一条在场连接（背包板端推流房间 `bp:<stream>`）
async fn handle_viewer(socket: WebSocket, state: AppState, stream_name: String, room: String) {
    let (_sink, mut stream) = socket.split();
    let first = state.video_hub.viewer_join(&room).await;

    // **每个「第一个观看者」都发 video_start**（不要去猜流在不在跑）。
    //
    // 旧逻辑是「房间里有流头且有推流端就不发」，但流头是上一轮留下的：
    // 最后一个人看完 → 看门狗 video_stop → 板端杀掉 ffmpeg，平台却以为还在推。
    // 下一个观看者进来时看到「publisher 在线」→ 不发 video_start → 板端永远不会
    // 重新起流（2026-11-18 现场：`last viewer ... left` 精确 23s 断一次）。
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
            .send_control(&room, video_start_message(&stream_name, profile))
            .await;
        if sent {
            // 只有真的让推流端起过流，才需要让看门狗盯着它
            state.video_hub.mark_streaming(&room, true).await;
        } else {
            warn!("viewer of {room}: video_start 没送出去（推流端未连接）");
        }
    }

    debug!("viewer of {room} attached (first={first})");
    // 这条连接上不会有任何字节要转发：只等它被关掉（浏览器关页 / 切通道 / 网络断）。
    while let Some(Ok(message)) = stream.next().await {
        if matches!(message, Message::Close(_)) {
            break;
        }
        // 浏览器不该发数据，忽略
    }

    if state.video_hub.viewer_leave(&room).await {
        // 不立刻停（切源无感）：保留 streaming 标记，交给看门狗在宽限后 video_stop
        info!("last viewer of {room} left（WebRTC 会话结束，交给看门狗宽限后停流）");
        state.video_hub.mark_streaming(&room, true).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_viewer_requests_stream_and_last_one_stops_it() {
        let hub = VideoHub::default();

        let first = hub.viewer_join("RD-001").await;
        assert!(first, "第一个观看者应触发推流");
        assert_eq!(hub.viewers("RD-001").await, 1);

        let first = hub.viewer_join("RD-001").await;
        assert!(!first, "第二个观看者不该重复下发 video_start");
        assert_eq!(hub.viewers("RD-001").await, 2);

        assert!(!hub.viewer_leave("RD-001").await);
        assert!(hub.viewer_leave("RD-001").await, "最后一个离开应停止推流");
        assert_eq!(hub.viewers("RD-001").await, 0);
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

        hub.viewer_join("RD-001").await;
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
        hub.viewer_join("RD-001").await;
        assert!(hub.idle_publishers(Duration::ZERO).await.is_empty());
    }

    /// 现场真实流程：观看者先进（它的加入才触发 video_start），推流端后到 ——
    /// 推流端接入时绝不能把等待中的观看者踢掉。
    #[tokio::test]
    async fn publisher_attach_keeps_viewers_that_joined_first() {
        let hub = VideoHub::default();
        let first = hub.viewer_join("RD-001").await;
        assert!(first);
        hub.publisher_attach("RD-001", 1).await;
        assert_eq!(hub.viewers("RD-001").await, 1, "观看者不该被清掉");
    }

    /// 推流端重连（弱网抖动）要**沿用房间**，只换会话号 + 观看者计数不动。
    ///
    /// 以前是 `rooms.insert(new_room)` 整间换掉：正在看的浏览器瞬间看到
    /// `has_publisher=false`；而旧连接常常是半开僵尸（要到 `PUBLISHER_SILENCE` 才判死），
    /// 新连接一 attach 就又抖一次 —— 2026-11-18 20:22:36 / 20:23:32 连踢两次。
    #[tokio::test]
    async fn publisher_reconnect_keeps_the_room_and_the_viewers() {
        let hub = VideoHub::default();
        hub.viewer_join("RD-001").await;
        hub.publisher_attach("RD-001", 1).await;

        // 观看端挂着的「有人在看」在重连期间不能掉
        hub.publisher_attach("RD-001", 2).await;
        assert!(hub.has_publisher("RD-001").await, "房间要沿用，不能重建");
        assert_eq!(hub.viewers("RD-001").await, 1, "观看计数不能被清掉");
        assert!(
            !hub.viewer_join("RD-001").await,
            "房间沿用，第二个观看者仍不该触发 video_start"
        );
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
        assert!(hub.viewer_join("bp:dogcam").await);
        assert!(
            hub.viewer_leave("bp:dogcam").await,
            "应判定为最后一个观看者"
        );
        assert!(
            hub.has_publisher("bp:dogcam").await,
            "推流端还在就不能丢房间"
        );

        // 再来一个观看者：仍然认得这路推流，并且要重新要一次流
        assert!(
            hub.viewer_join("bp:dogcam").await,
            "宽限内重看也要重新下发 video_start（宽限只是不停流）"
        );
    }

    /// 没有推流端的房间在最后一个观看者离开后照旧被清掉
    #[tokio::test]
    async fn room_without_publisher_is_dropped_with_the_last_viewer() {
        let hub = VideoHub::default();
        hub.viewer_join("RD-001").await;
        assert!(hub.viewer_leave("RD-001").await);
        assert!(!hub.has_publisher("RD-001").await);
    }

    /// 推流端抖动（房间被丢弃）时，最后一个观看者离开仍必须触发 `video_stop`，
    /// 否则客户端会一直重连推流 —— 现场真机上踩过：相机断一下，狗就一直编码。
    #[tokio::test]
    async fn last_viewer_still_reported_after_room_was_dropped() {
        let hub = VideoHub::default();
        hub.viewer_join("RD-001").await;
        hub.publisher_attach("RD-001", 1).await;
        hub.publisher_gone("RD-001", 1).await; // 房间没了，但观看者还连着
        assert!(hub.viewer_leave("RD-001").await, "应判定为最后一个观看者");
        assert_eq!(hub.viewers("RD-001").await, 0);
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

    /// 设备客户端的房间没有控制通道（控制走设备网关）
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

    /// 排障快照：`transport` 是「板端 relay 有没有升级」的唯一凭据。
    #[tokio::test]
    async fn debug_state_reports_the_media_transport() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 1).await;
        assert!(
            hub.debug_state().await["rooms"][0]["transport"].is_null(),
            "还没上报时应为 null（前端据此报「板端未升级」）"
        );

        hub.set_transport("bp:dogcam", Some("rtsp".to_string()))
            .await;
        assert_eq!(
            hub.debug_state().await["rooms"][0]["transport"],
            serde_json::json!("rtsp")
        );
    }

    /// 重连补发 `video_start` 的判据：现在就是「有人在看」
    /// （媒体在 mediamtx 里，平台看不见字节，所以不能再用「房间里有流头」）。
    #[tokio::test]
    async fn viewers_waiting_is_just_whether_anybody_is_watching() {
        let hub = VideoHub::default();
        hub.publisher_attach("bp:dogcam", 1).await;
        assert!(!hub.viewers_waiting("bp:dogcam").await, "没人看就不用补发");
        hub.viewer_join("bp:dogcam").await;
        assert!(hub.viewers_waiting("bp:dogcam").await);
        hub.viewer_leave("bp:dogcam").await;
        assert!(!hub.viewers_waiting("bp:dogcam").await);
    }

    /* ---------------- 多设备命名 ---------------- */

    /// 设备编号会同时进 URL / 房间名 / mediamtx path，所以只能放行安全字符；
    /// 空串 = 默认设备（旧命名，绝不能因为前端传了个空 ``device`` 就变成 `bp::dogcam`）。
    #[test]
    fn device_ids_are_restricted_to_path_safe_characters() {
        assert_eq!(normalize_device(" RD-001 "), Some("RD-001".to_string()));
        assert_eq!(
            normalize_device("gh-test_2.1"),
            Some("gh-test_2.1".to_string())
        );
        assert_eq!(normalize_device(""), None);
        assert_eq!(normalize_device("   "), None);
        // 冒号会撞房间分隔符、斜杠/点会撞 URL 与 path、中文不在白名单（上游 SN 是 ASCII）
        assert_eq!(normalize_device("a:b"), None);
        assert_eq!(normalize_device("../x"), None);
        assert_eq!(normalize_device("1号狗"), None);
        assert_eq!(normalize_device(&"a".repeat(33)), None);
    }

    /// 三个名字必须一一对应，而且旧命名（无设备号）一个字节都不变：
    /// 现场已部署的 relay 仍推 `bp_cam0` / 注册 `bp:cam0`。
    #[test]
    fn naming_keeps_legacy_shape_and_adds_the_device_dimension() {
        assert_eq!(room_of(None, "dogcam"), "bp:dogcam");
        assert_eq!(room_of(Some(""), "dogcam"), "bp:dogcam");
        assert_eq!(room_of(Some("RD-001"), "dogcam"), "bp:RD-001:dogcam");

        assert_eq!(mediamtx_path_of(None, "dogcam"), "bp_dogcam");
        assert_eq!(
            mediamtx_path_of(Some("RD-001"), "dogcam"),
            "bp_RD-001_dogcam"
        );

        assert_eq!(
            split_room("bp:dogcam"),
            Some((String::new(), "dogcam".into()))
        );
        assert_eq!(
            split_room("bp:RD-001:dogcam"),
            Some(("RD-001".to_string(), "dogcam".to_string()))
        );
        assert_eq!(split_room("RD-001"), None, "设备客户端老房间不是背包房间");
    }

    /// 两台板子的同名通道必须是两个房间：以前都拼 `bp:cam0`，后注册的那台会接管
    /// 控制通道、把前一台的 `video_start` 抢走（现场就是「点开是另一台狗的画面」）。
    #[tokio::test]
    async fn two_devices_keep_separate_rooms_viewers_and_controls() {
        let hub = VideoHub::default();
        let first = room_of(Some("RD-001"), "cam0");
        let second = room_of(Some("RD-002"), "cam0");
        assert_ne!(first, second);

        hub.publisher_attach(&first, 1).await;
        hub.publisher_attach(&second, 2).await;
        assert!(hub.has_publisher(&first).await);
        assert!(hub.has_publisher(&second).await);

        // 观看者计数分开：看 RD-001 的人不能把 RD-002 的看门狗按下去
        assert!(hub.viewer_join(&first).await);
        assert_eq!(hub.viewers(&first).await, 1);
        assert_eq!(hub.viewers(&second).await, 0);

        // 控制通道分开：`video_start` 只能落到对应的那台板子
        let (tx1, mut rx1) = tokio::sync::mpsc::channel(4);
        let (tx2, mut rx2) = tokio::sync::mpsc::channel(4);
        hub.register_control(&first, 1, tx1).await;
        hub.register_control(&second, 2, tx2).await;
        assert!(
            hub.send_control(&first, "video_start".into()).await,
            "第一台要收到"
        );
        assert_eq!(rx1.recv().await.as_deref(), Some("video_start"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), rx2.recv())
                .await
                .is_err(),
            "第二台不能收到另一台的控制消息"
        );

        assert_eq!(
            hub.control_rooms().await,
            vec![first.clone(), second.clone()],
            "广播中继档位时要能枚举到全部设备房间"
        );
    }

    /// `GET /api/backpack/devices` 的快照：设备/通道拆开、老房间归到默认设备、
    /// 不在白名单里的通道不给前端。
    #[tokio::test]
    async fn channels_expose_device_and_stream_for_the_console() {
        let hub = VideoHub::default();
        hub.publisher_attach(&room_of(None, "dogcam"), 1).await;
        hub.publisher_attach(&room_of(Some("RD-002"), "cam1"), 2)
            .await;
        hub.set_transport(&room_of(Some("RD-002"), "cam1"), Some("rtsp".into()))
            .await;
        // 不在白名单：WHEP 反代会拒绝，前端也不该列出来
        hub.publisher_attach(&room_of(Some("RD-002"), "evil"), 3)
            .await;

        let list = hub.channels().await;
        assert_eq!(list.len(), 2, "只列白名单通道: {list:?}");
        assert_eq!(list[0].device, "", "默认设备排在最前");
        assert_eq!(list[0].stream, "dogcam");
        assert!(list[0].publisher);
        assert_eq!(list[1].device, "RD-002");
        assert_eq!(list[1].stream, "cam1");
        assert_eq!(list[1].transport.as_deref(), Some("rtsp"));
    }
}
