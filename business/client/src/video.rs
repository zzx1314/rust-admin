//! 实时视频回传：RTSP → FLV → 业务平台
//!
//! 智航 Pro 协议没有视频接口，本体只对外提供 RTSP（如
//! `rtsp://user:***@192.168.234.1:8554/test`）；浏览器不能直连 RTSP，
//! 而能访问本体网段的是本客户端 —— 所以由这里把流接回来：
//!
//! ```text
//! 本体 RTSP ──ffmpeg（转封装 H.264/FLV）──► 本模块 ──ws binary──► 业务平台 ──ws──► 浏览器
//! ```
//!
//! 触发方式是**按需拉流**：控制台有人打开实时监控时，业务平台在既有设备连接上下发
//! `video_start`（带设备管理里填的 `video_url`），最后一个观看者离开时下发
//! `video_stop`。没人看的时候本模块不占 CPU / 带宽。
//!
//! 编码参数在 `client.toml` 的 `[video]` 段（`ffmpeg` / `fps` / `bitrate` / `width`）。
//! 拉流地址不落配置：它属于设备档案，运营在设备管理里改一次即可。

use robodog_device_client::config::Config;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

/// 读 ffmpeg stdout 的分片大小：一个 FLV tag 常见几 KB，8KB 足够低延迟
const CHUNK_BYTES: usize = 8 * 1024;
/// 断流重连间隔（本体没出流 / 网络抖动的退避）
const RETRY_DELAY: Duration = Duration::from_secs(3);
/// 停止时等 ffmpeg 退出的上限
const STOP_GRACE: Duration = Duration::from_secs(5);

/// 推流句柄：可跨任务 clone，内部只有一个监管任务
#[derive(Clone)]
pub struct VideoRelay {
    commands: mpsc::Sender<Command2>,
}

/// 内部命令（与 `std::process::Command` 无关，避免重名混淆）
enum Command2 {
    Start(String),
    Stop,
}

impl VideoRelay {
    /// 启动监管任务。真正拉流要等业务平台下发 `video_start`。
    pub fn spawn(config: Arc<Config>) -> Self {
        let (tx, mut rx) = mpsc::channel::<Command2>(8);
        tokio::spawn(async move {
            // 当前在跑的推流任务（句柄 + 停止信号）
            let mut active: Option<(tokio::task::JoinHandle<()>, oneshot::Sender<()>)> = None;

            while let Some(command) = rx.recv().await {
                match command {
                    Command2::Start(url) => {
                        if !config.video_enabled {
                            warn!("收到 video_start，但 video.enabled = false，忽略");
                            continue;
                        }
                        shutdown(&mut active).await;
                        let (stop_tx, stop_rx) = oneshot::channel();
                        let config = config.clone();
                        let handle = tokio::spawn(async move {
                            run_stream(config, url, stop_rx).await;
                        });
                        active = Some((handle, stop_tx));
                    }
                    Command2::Stop => {
                        if active.is_some() {
                            info!("业务平台结束实时视频，停止拉流");
                        }
                        shutdown(&mut active).await;
                    }
                }
            }

            // 客户端进程退出（VideoRelay 被 drop）：兜底停掉 ffmpeg
            shutdown(&mut active).await;
        });
        Self { commands: tx }
    }

    /// 业务平台请求开始拉流（地址来自设备档案的 `video_url`）
    pub async fn start(&self, url: String) {
        let _ = self.commands.send(Command2::Start(url)).await;
    }

    /// 业务平台请求停止拉流
    pub async fn stop(&self) {
        let _ = self.commands.send(Command2::Stop).await;
    }
}

/// 停掉在跑的推流任务（先发停止信号，超时直接 abort）
async fn shutdown(active: &mut Option<(tokio::task::JoinHandle<()>, oneshot::Sender<()>)>) {
    let Some((mut handle, stop)) = active.take() else {
        return;
    };
    let _ = stop.send(());
    if tokio::time::timeout(STOP_GRACE, &mut handle).await.is_err() {
        warn!("拉流任务未在 {STOP_GRACE:?} 内退出，强制中止");
        handle.abort();
    }
}

/// 推流主循环：连平台 → 起 ffmpeg → 把 stdout 分片发出去；断了就退避重连
async fn run_stream(config: Arc<Config>, url: String, mut stop: oneshot::Receiver<()>) {
    let endpoint = format!("{}/ws/device/video", config.server_url.trim_end_matches('/'));
    info!("开始实时视频回传: {} → {endpoint}", redact_url(&url));

    loop {
        tokio::select! {
            _ = &mut stop => {
                info!("实时视频已停止");
                return;
            }
            _ = pump_once(&config, &endpoint, &url) => {}
        }

        tokio::select! {
            _ = &mut stop => {
                info!("实时视频已停止");
                return;
            }
            _ = tokio::time::sleep(RETRY_DELAY) => {}
        }
    }
}

/// 一次完整的推流会话；返回即代表这次断了（上层决定是否重连）
async fn pump_once(config: &Config, endpoint: &str, url: &str) {
    let (socket, _) = match connect_async(endpoint).await {
        Ok(pair) => pair,
        Err(error) => {
            warn!("连接视频中继通道失败: {error}");
            return;
        }
    };
    let (mut sink, mut stream) = socket.split();

    // 首帧文本：注册推流归属（服务端据此把二进制帧路由到对应设备的房间）
    let hello = json!({
        "msgType": "video_register",
        "ts": chrono::Utc::now().to_rfc3339(),
        "payload": { "deviceId": config.device_id },
    });
    if sink.send(Message::Text(hello.to_string())).await.is_err() {
        warn!("视频中继通道握手失败");
        return;
    }

    let mut child = match spawn_ffmpeg(config, url) {
        Ok(child) => child,
        Err(error) => {
            error!(
                "启动 ffmpeg 失败（{}）: {error}；请确认已安装 ffmpeg 或配置 [video].ffmpeg",
                config.video_ffmpeg
            );
            return;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        error!("ffmpeg stdout 未接管");
        let _ = child.kill().await;
        return;
    };

    info!("ffmpeg 已启动，开始回传 FLV");
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    let mut sent = 0_u64;
    loop {
        tokio::select! {
            read = stdout.read(&mut buffer) => match read {
                Ok(0) => {
                    warn!("ffmpeg 结束（已传 {sent} 字节），{RETRY_DELAY:?} 后重试");
                    break;
                }
                Ok(n) => {
                    sent += n as u64;
                    let frame = buffer[..n].to_vec();
                    if sink.send(Message::Binary(frame)).await.is_err() {
                        warn!("视频中继连接已断开");
                        break;
                    }
                }
                Err(error) => {
                    warn!("读取 ffmpeg 输出失败: {error}");
                    break;
                }
            },
            incoming = stream.next() => match incoming {
                Some(Ok(Message::Close(_))) | None => {
                    info!("业务平台关闭了视频中继通道");
                    break;
                }
                Some(Err(error)) => {
                    warn!("视频中继通道异常: {error}");
                    break;
                }
                // 服务端不会往这条连接发数据，其余消息忽略
                Some(Ok(_)) => {}
            },
        }
    }

    // 无论如何都收回 ffmpeg，避免留下孤儿进程
    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// 组装 ffmpeg 参数（纯函数，便于单测校验「参数顺序」这类踩坑点）
///
/// 顺序要求：所有输入参数（`-rtsp_transport`）在 `-i` 之前；
/// 所有输出参数（含 `-vf`）在输出 URL `pipe:1` 之前 —— ffmpeg 的参数是按位置归属的。
fn ffmpeg_args(config: &Config, url: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "warning".into(),
        "-nostdin".into(),
    ];
    // RTSP 才需要传输方式：现场走 TCP 更稳（UDP 丢包会花屏）；
    // 填 http/m3u8 之类地址时不能带这个私有选项，否则 ffmpeg 直接报错。
    if url.starts_with("rtsp://") {
        args.push("-rtsp_transport".into());
        args.push(config.video_transport.clone());
    }
    args.push("-i".into());
    args.push(url.to_string());

    for value in [
        "-an",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-tune",
        "zerolatency",
        // baseline profile + yuv420p：浏览器 MSE 兼容性最好
        "-profile:v",
        "baseline",
        "-pix_fmt",
        "yuv420p",
        "-g",
    ] {
        args.push(value.to_string());
    }
    // 2 秒一个关键帧：决定起播与丢帧恢复的最长等待
    args.push((config.video_fps * 2).to_string());
    for value in [
        "-sc_threshold",
        "0",
        "-r",
    ] {
        args.push(value.to_string());
    }
    args.push(config.video_fps.to_string());
    for value in ["-b:v", "-maxrate"] {
        args.push(value.to_string());
        args.push(config.video_bitrate.clone());
    }
    args.push("-bufsize".into());
    args.push(config.video_bitrate.clone());

    // 缩放必须在输出 URL 之前
    if config.video_width > 0 {
        args.push("-vf".into());
        args.push(format!("scale={}:-2", config.video_width));
    }
    args.push("-f".into());
    args.push("flv".into());
    args.push("pipe:1".into());
    args
}

/// 启动 ffmpeg：RTSP 输入 → H.264 + FLV 输出到 stdout
///
/// 参数取向是**低延迟优先**：`ultrafast` + `zerolatency` + 2 秒一个关键帧，
/// 关键帧间隔决定了播放器起播与丢帧恢复的最长等待。
fn spawn_ffmpeg(config: &Config, url: &str) -> std::io::Result<tokio::process::Child> {
    let mut command = Command::new(&config.video_ffmpeg);
    command.args(ffmpeg_args(config, url));
    // -an 已去掉音轨；stdin 关闭、stdout 走管道给 WS、stderr 单独消费
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn()?;
    // stderr 必须持续消费：不读的话管道写满会让 ffmpeg 卡住
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                warn!("ffmpeg: {line}");
            }
        });
    }
    Ok(child)
}

/// 日志里隐去 RTSP 凭据（`rtsp://user:pw@host` → `rtsp://user:***@host`）
pub fn redact_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest = &url[scheme_end + 3..];
    let Some(at) = rest.rfind('@') else {
        return url.to_string();
    };
    let authority = &rest[..at];
    let host = &rest[at + 1..];
    let user = authority.split(':').next().unwrap_or("");
    format!("{}://{user}:***@{host}", &url[..scheme_end])
}

#[cfg(test)]
mod tests {
    use super::{ffmpeg_args, redact_url};
    use robodog_device_client::config::{CliArgs, Config, EnvConfig, FileConfig};

    fn test_config(with_scale: bool) -> Config {
        // 用默认值构造：不读文件、不读环境，避免用例受现场配置影响
        let mut file = FileConfig::default();
        if with_scale {
            file.video.width = Some(1280);
        }
        Config::resolve(&file, &EnvConfig::default(), &CliArgs::default(), Some("TEST-SN"))
            .expect("默认配置应可解析")
    }

    #[test]
    fn ffmpeg_output_options_come_before_the_pipe() {
        let args = ffmpeg_args(&test_config(false), "rtsp://user:pw@192.168.234.1:8554/test");
        // 输入参数必须在 -i 之前
        let transport = args.iter().position(|arg| arg == "-rtsp_transport").unwrap();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        assert!(transport < input, "RTSP 传输方式要放在 -i 之前");
        // 输出参数必须在输出 URL 之前，且输出固定是 FLV → pipe:1
        assert_eq!(&args[args.len() - 3..], ["-f", "flv", "pipe:1"]);
        assert!(!args.contains(&"-vf".to_string()), "未配置宽度时不该缩放");
    }

    #[test]
    fn scale_is_inserted_before_output_url() {
        let args = ffmpeg_args(&test_config(true), "rtsp://192.168.234.1:8554/test");
        let scale = args.iter().position(|arg| arg == "-vf").unwrap();
        let format = args.iter().position(|arg| arg == "-f").unwrap();
        assert!(scale < format, "缩放参数放在输出 URL 之后会被 ffmpeg 忽略");
        assert_eq!(args[scale + 1], "scale=1280:-2");
    }

    #[test]
    fn non_rtsp_source_has_no_rtsp_option() {
        let args = ffmpeg_args(&test_config(false), "http://192.168.41.9/live.m3u8");
        assert!(
            !args.contains(&"-rtsp_transport".to_string()),
            "非 RTSP 地址带 -rtsp_transport 会让 ffmpeg 报错"
        );
    }

    #[test]
    fn rtsp_credentials_are_masked_in_logs() {
        // 现场地址的密码是 URL 编码的（Abcd%232027%21 = Abcd#2027!）
        assert_eq!(
            redact_url("rtsp://user:Abcd%232027%21@192.168.234.1:8554/test"),
            "rtsp://user:***@192.168.234.1:8554/test"
        );
        // 没有凭据的地址原样返回
        assert_eq!(
            redact_url("rtsp://192.168.234.1:8554/test"),
            "rtsp://192.168.234.1:8554/test"
        );
        // 非 URL 也原样返回，不 panic
        assert_eq!(redact_url("192.168.234.1"), "192.168.234.1");
    }
}
