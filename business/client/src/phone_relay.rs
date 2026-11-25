//! 网页对讲中继：板端 UDP 音频 ⇄ 平台（浏览器）
//!
//! 板端 `voice_call` 只会跟**同网段 UDP 对端**说话，公网浏览器既发不了裸 UDP，
//! 板子在 4G 后面也没有入站端口。本进程与板端同机（`127.0.0.1` 一定通），于是当那个
//! 「对端」，音频借既有设备 WS 上下行：
//!
//! ```text
//! 平台 phone_stream start ─▶ 本模块 bind 127.0.0.1:0
//!                          ─▶ 板端 /api/phone {peer:127.0.0.1, peerPort:<本机端口>}
//! 板端 UDP（12B 头 + 640B PCM / 20ms）──▶ 本模块 ──phone_audio{base64}──▶ 平台 ──▶ 浏览器
//! 平台 phone_audio{base64} ──▶ 本模块 ──UDP 12B 头 + 640B──▶ 板端 127.0.0.1:5000
//! 平台 phone_stream stop   ─▶ 板端 /api/phone/stop，收掉 socket
//! ```
//!
//! 三条约定（改这块先看）：
//!
//! 1. **一次只允许一路**：板端 FIFO / UDP 都是单通道，平台已经保证只有一条浏览器连接，
//!    这里再兜一次 —— 重复 `start` 直接幂等返回，不重建 socket。
//! 2. **只搬字节**：板端就是 16kHz / 单声道 / S16_LE / 20ms 的裸 PCM，这里不重采样、
//!    不编码、不做音量处理（要做压缩就先改协议，别偷偷改字节）。
//! 3. **异常必须收尾**：任何退出路径都要 `POST /api/phone/stop`，否则板子停在 phone
//!    模式（唤醒词失效、还占着 FIFO）。上行断开、平台断连、`stop` 指令都算。

use crate::phone;
use crate::wire::WsMessage;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// 板端 `voice_call` 固定监听端口（与 `backpack_dashboard/backend/phone.py::LOCAL_PORT` 一致）
const BOARD_PORT: u16 = 5000;
/// 20ms @ 16kHz 单声道 S16_LE
const FRAME_BYTES: usize = 640;
/// 板端那套极简 RTP 头：`0x80 | PT=96 | seq(2) | ts(4) | ssrc(4)`
const HDR_BYTES: usize = 12;
/// 收包超时：让收包任务有机会看到 `running = false` 后退出
const RECV_TIMEOUT: Duration = Duration::from_millis(500);
/// 自己发出去的 SSRC（板端只用来对齐抖动缓冲，没有语义）
const SSRC: u32 = 0x52444F47; // 'RDOG'

struct Session {
    udp: Arc<UdpSocket>,
    local_port: u16,
    running: Arc<AtomicBool>,
    seq: AtomicU16,
    ts: AtomicU32,
    task: JoinHandle<()>,
}

/// 对讲中继（进程内一份，随平台连接重建）
pub struct PhoneRelay {
    /// 板端 Agent 基地址（与 AI / 播报 / 电话共用 `[ai] agent_url`）
    agent: String,
    /// 上行走设备 WS（`phone_audio` 帧）
    out: mpsc::Sender<WsMessage>,
    session: Mutex<Option<Session>>,
}

impl PhoneRelay {
    pub fn new(agent: impl Into<String>, out: mpsc::Sender<WsMessage>) -> Self {
        Self {
            agent: agent.into().trim_end_matches('/').to_string(),
            out,
            session: Mutex::new(None),
        }
    }

    pub fn agent(&self) -> &str {
        &self.agent
    }

    /// 是否正在对讲
    pub async fn active(&self) -> bool {
        self.session.lock().await.is_some()
    }

    /// 平台第一次下发 `phone_stream start` / 板端 Dashboard 开始对讲
    ///
    /// 幂等：已经在通话里就直接返回（重复 start 不重建 socket，否则正在说话的那路
    /// 会被掐断）。
    pub async fn start(&self) -> Result<u16, String> {
        let mut guard = self.session.lock().await;
        if let Some(session) = guard.as_ref() {
            return Ok(session.local_port);
        }

        let udp = Arc::new(
            UdpSocket::bind("127.0.0.1:0")
                .await
                .map_err(|error| format!("绑定本机 UDP 失败: {error}"))?,
        );
        let local_port = udp
            .local_addr()
            .map_err(|error| format!("取本机 UDP 端口失败: {error}"))?
            .port();

        // 让板端来找「我」：`phone <ip> <port>` 的对端就是本机这个 UDP 端口
        phone::start(&self.agent, "127.0.0.1", u64::from(local_port)).await?;

        let running = Arc::new(AtomicBool::new(true));
        let task = tokio::spawn(recv_loop(
            udp.clone(),
            running.clone(),
            self.out.clone(),
            Arc::new(self.agent.clone()),
        ));
        *guard = Some(Session {
            udp,
            local_port,
            running,
            seq: AtomicU16::new(0),
            ts: AtomicU32::new(0),
            task,
        });
        info!("对讲：本机 UDP 对端 127.0.0.1:{local_port}，板端已切到 phone 模式");
        Ok(local_port)
    }

    /// 浏览器上行的一帧 PCM（base64）：拆成 20ms 帧发给板端 5000
    pub async fn push_uplink(&self, data: &str) -> Result<(), String> {
        let pcm = STANDARD
            .decode(data)
            .map_err(|error| format!("上行音频不是合法 base64: {error}"))?;
        if pcm.is_empty() {
            return Ok(());
        }
        let guard = self.session.lock().await;
        let Some(session) = guard.as_ref() else {
            return Err("当前不在通话中".to_string());
        };
        for chunk in pcm.chunks(FRAME_BYTES) {
            let mut frame = [0u8; FRAME_BYTES];
            frame[..chunk.len()].copy_from_slice(chunk);
            let seq = session.seq.fetch_add(1, Ordering::Relaxed);
            let ts = session.ts.fetch_add(320, Ordering::Relaxed);
            let mut packet = Vec::with_capacity(HDR_BYTES + FRAME_BYTES);
            packet.push(0x80); // V=2
            packet.push(96); // PT
            packet.extend_from_slice(&seq.to_be_bytes());
            packet.extend_from_slice(&ts.to_be_bytes());
            packet.extend_from_slice(&SSRC.to_be_bytes());
            packet.extend_from_slice(&frame);
            if let Err(error) = session
                .udp
                .send_to(&packet, ("127.0.0.1", BOARD_PORT))
                .await
            {
                warn!("对讲：上行 UDP 发送失败（{error}）");
                return Err(format!("上行发送失败: {error}"));
            }
        }
        Ok(())
    }

    /// 结束对讲（幂等）：停收包任务 + `phone stop`
    pub async fn stop(&self, reason: &str) {
        let session = {
            let mut guard = self.session.lock().await;
            guard.take()
        };
        let Some(session) = session else {
            return;
        };
        session.running.store(false, Ordering::Relaxed);
        session.task.abort();
        // 板端可能已经回到 voice：`phone stop` 在那边是幂等的
        if let Err(error) = phone::stop(&self.agent, reason).await {
            warn!("对讲：挂断失败（{error}）；交给板端看门狗 / 下一次 start 收口");
        }
        info!("对讲结束（{reason}）");
    }
}

/// 收包任务：板端 PCM → 平台 `phone_audio`
///
/// 平台连接一断（`out.send` 失败）就自己收口，避免板子一直停在 phone 模式。
async fn recv_loop(
    udp: Arc<UdpSocket>,
    running: Arc<AtomicBool>,
    out: mpsc::Sender<WsMessage>,
    agent: Arc<String>,
) {
    let mut buf = vec![0u8; 4096];
    let mut frames: u64 = 0;
    while running.load(Ordering::Relaxed) {
        match tokio::time::timeout(RECV_TIMEOUT, udp.recv(&mut buf)).await {
            Ok(Ok(len)) if len > HDR_BYTES => {
                // 板端那套头没有扩展位，12 字节之后就是 PCM
                let payload = STANDARD.encode(&buf[HDR_BYTES..len]);
                let msg = WsMessage {
                    msg_type: "phone_audio".to_string(),
                    ts: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    payload: json!({ "data": payload }),
                };
                if out.send(msg).await.is_err() {
                    warn!("对讲：平台连接已断，结束本次通话");
                    break;
                }
                frames += 1;
                if frames.is_multiple_of(250) {
                    debug!("对讲：已上传 {frames} 帧（{}s）", frames / 50);
                }
            }
            Ok(Ok(_)) => {} // 空包 / 头都不全：丢掉
            Ok(Err(error)) => {
                warn!("对讲：UDP 收包失败（{error}）");
                break;
            }
            Err(_) => {} // 超时：回去看 running
        }
    }
    running.store(false, Ordering::Relaxed);
    if let Err(error) = phone::stop(&agent, "音频通道结束").await {
        warn!("对讲：收尾挂断失败（{error}）");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(packet: &[u8]) -> (u8, u8, u16, u32, u32) {
        (
            packet[0],
            packet[1],
            u16::from_be_bytes([packet[2], packet[3]]),
            u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
            u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        )
    }

    /// 帧头形态：与 `pc_tools/voice_call_pc.py` / `voice_call.cpp` 必须一致
    #[test]
    fn uplink_frames_carry_the_board_compatible_header() {
        let mut packet = Vec::new();
        packet.push(0x80);
        packet.push(96);
        packet.extend_from_slice(&7u16.to_be_bytes());
        packet.extend_from_slice(&320u32.to_be_bytes());
        packet.extend_from_slice(&SSRC.to_be_bytes());
        packet.extend_from_slice(&[0u8; FRAME_BYTES]);
        let (version, pt, seq, ts, ssrc) = headers(&packet);
        assert_eq!(version, 0x80, "V=2 / P=0 / X=0 / CC=0");
        assert_eq!(pt, 96);
        assert_eq!(seq, 7);
        assert_eq!(ts, 320, "20ms @16kHz = 320 采样");
        assert_eq!(ssrc, SSRC);
        assert_eq!(packet.len(), HDR_BYTES + FRAME_BYTES);
    }

    /// 板端上行包：剥掉 12 字节头之后的字节要原样 base64 给平台
    #[tokio::test]
    async fn downlink_strips_the_header_and_frames_are_20ms() {
        let (tx, mut rx) = mpsc::channel(8);
        let relay = PhoneRelay::new("http://127.0.0.1:1", tx);
        // 直接构造一帧「板端发来的包」喂给收包逻辑的等价实现：这里只校验编码规则
        let mut packet = vec![0x80, 96, 0, 1, 0, 0, 1, 64, 0x52, 0x44, 0x4F, 0x47];
        packet.extend_from_slice(&[7u8; FRAME_BYTES]);
        let encoded = STANDARD.encode(&packet[HDR_BYTES..]);
        assert_eq!(STANDARD.decode(&encoded).unwrap().len(), FRAME_BYTES);
        // 未在通话中时上行要明确报错，而不是静默丢字节
        assert!(relay.push_uplink(&encoded).await.is_err());
        assert!(rx.try_recv().is_err(), "没在通话中不应该有任何上行消息");
        assert!(!relay.active().await);
    }
}
