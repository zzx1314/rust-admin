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

use futures_util::{SinkExt, StreamExt};
use robodog_device_client::config::Config;
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
/// 连续多少次「一个字节都没推出去」就停止重试
const MAX_CONSECUTIVE_FAILURES: u32 = 8;

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
            // 当前在回传的拉流地址：用来去重重复的 `video_start`
            let mut current_url: Option<String> = None;

            while let Some(command) = rx.recv().await {
                match command {
                    Command2::Start(url) => {
                        if !config.video_enabled {
                            warn!("收到 video_start，但 video.enabled = false，忽略");
                            continue;
                        }
                        // 平台每次「第一个观看者进来」都会重发 video_start：浏览器 WS 抖动
                        // 重连、抽屉关掉再打开、切页签都算一次第一个观看者。同一地址重复
                        // 起流会把 ffmpeg 重启一遍（黑 3~7s + 换新 FLV 头），现场看到的就是
                        // 「画面一闪、连接中断」。已经在推同一地址就什么都不做。
                        let running = active
                            .as_ref()
                            .map(|(handle, _)| !handle.is_finished())
                            .unwrap_or(false);
                        if running && current_url.as_deref() == Some(url.as_str()) {
                            info!("已在回传 {}，忽略重复的 video_start", redact_url(&url));
                            continue;
                        }
                        shutdown(&mut active).await;
                        current_url = Some(url.clone());
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
                        current_url = None;
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
    let endpoint = format!(
        "{}/ws/device/video",
        config.server_url.trim_end_matches('/')
    );
    info!("开始实时视频回传: {} → {endpoint}", redact_url(&url));

    // 连续失败熔断：一次字节都没推出去说明 ffmpeg / 相机侧有问题，
    // 无脑重试只会反复抢占 RTSP 会话（相机通常只允许 1~2 路），什么也修不好。
    let mut failures = 0_u32;

    loop {
        let pushed = tokio::select! {
            _ = &mut stop => {
                info!("实时视频已停止");
                return;
            }
            pushed = pump_once(&config, &endpoint, &url) => pushed,
        };
        if pushed {
            failures = 0;
        } else {
            failures += 1;
            if failures >= MAX_CONSECUTIVE_FAILURES {
                error!(
                    "实时视频连续 {failures} 次没有推出任何数据，停止重试（检查 ffmpeg 与相机 RTSP）"
                );
                return;
            }
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

/// 一次完整的推流会话；返回本次是否真的推过字节（上层据此判断是否熔断）
async fn pump_once(config: &Config, endpoint: &str, url: &str) -> bool {
    let (socket, _) = match connect_async(endpoint).await {
        Ok(pair) => pair,
        Err(error) => {
            warn!("连接视频中继通道失败: {error}");
            return false;
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
        return false;
    }

    let Some(encoder) = detect_encoder(config).await else {
        return false;
    };
    let mut child = match spawn_ffmpeg(config, url, &encoder) {
        Ok(child) => child,
        Err(error) => {
            error!(
                "启动 ffmpeg 失败（{}）: {error}；请确认已安装 ffmpeg 或配置 [video].ffmpeg",
                config.video_ffmpeg
            );
            return false;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        error!("ffmpeg stdout 未接管");
        let _ = child.kill().await;
        return false;
    };

    info!("ffmpeg 已启动，开始回传 FLV");
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    let mut sent = 0_u64;
    // 保活：现场是 4G / 弱网 wifi，NAT 与代理会回收「只上行、长时间无回包」的
    // TCP 连接，服务端也有 60s 静默判死。每 20s 一个 WS ping，两边都顶得住。
    let mut keepalive = tokio::time::interval(Duration::from_secs(20));
    keepalive.tick().await;
    loop {
        tokio::select! {
            _ = keepalive.tick() => {
                if sink.send(Message::Ping(Vec::new())).await.is_err() {
                    warn!("视频中继保活发送失败，连接已断开");
                    break;
                }
            }
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
    sent > 0
}

/// 组装 ffmpeg 参数（纯函数，便于单测校验「参数顺序」这类踩坑点）
///
/// 顺序要求：所有输入参数（`-rtsp_transport`）在 `-i` 之前；
/// 所有输出参数（含 `-vf`）在输出 URL `pipe:1` 之前 —— ffmpeg 的参数是按位置归属的。
fn ffmpeg_args(config: &Config, url: &str, encoder: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "warning".into(),
        "-nostdin".into(),
    ];
    if url.starts_with("rtsp://") {
        // 用**到达时间**当 PTS，而不是相机给的时间戳。
        //
        // 现场相机的 SDP 标 60fps、PTS 也按 1/60 递增，但实际只投递 ~25 帧/秒：
        // 直接沿用它的时间轴，媒体时间只走 0.42x 实时，浏览器 MSE 会反复饿死
        // （播放器在「连接中 / LIVE」之间来回跳、画面变慢放）。
        // 改成墙钟时间戳后时间轴与真实时间 1:1，播放才稳。
        args.push("-use_wallclock_as_timestamps".into());
        args.push("1".into());
        // 源包里 PTS 缺失 / 不连续时补一个：`-r` 恒定帧率输出要靠它，
        // 否则 MSE 的时间轴会跳（板端 relay 同样是这么配的）
        args.push("-fflags".into());
        args.push("+genpts".into());
        // 拉流侧：现场 RTSP 走 TCP 更稳（UDP 丢包会花屏）。
        // 非 RTSP 地址不能带这些私有选项，否则 ffmpeg 直接报错。
        args.push("-rtsp_transport".into());
        args.push(config.video_transport.clone());
    }
    args.push("-i".into());
    args.push(url.to_string());

    args.push("-an".into());
    args.push("-c:v".into());
    args.push(encoder.to_string());

    // 编码器专属参数：软件 x264 有 preset/tune/profile，硬件编码器没有这些选项，
    // 带上会直接「Unrecognized option 'preset'」启动失败（瑞芯微 ffmpeg 就是这样）。
    if encoder == "libx264" {
        for value in [
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-profile:v",
            "baseline",
        ] {
            args.push(value.to_string());
        }
    }
    // yuv420p + 固定 GOP：浏览器 MSE 兼容性最好，GOP 决定起播与丢帧恢复
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push("-g".into());
    // 2 秒一个关键帧：决定起播与丢帧恢复的最长等待
    args.push((config.video_fps * 2).to_string());
    if encoder == "libx264" {
        args.push("-sc_threshold".into());
        args.push("0".into());
    }
    args.push("-r".into());
    args.push(config.video_fps.to_string());
    for value in ["-b:v", "-maxrate"] {
        args.push(value.to_string());
        args.push(config.video_bitrate.clone());
    }
    args.push("-bufsize".into());
    args.push(config.video_bitrate.clone());

    // 输出必须**逐包立刻吐出**：ffmpeg 写管道默认带缓冲（攒满一整个 IO 块才 flush），
    // 转码出来的小帧会被攒成「一阵一阵」的突发数据。浏览器 MSE 收到突发数据时
    // 先饿死再猛灌，flv.js 在 playing / buffering 之间反复横跳 —— 现场就是「一闪一闪」。
    // 板端 relay（`deploy/backpack-relay/relay_push.py`）一直带这三个参数，所以它稳。
    for value in ["-flush_packets", "1", "-muxdelay", "0", "-muxpreload", "0"] {
        args.push(value.into());
    }

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

/// 依次尝试的编码器（`[video].encoder = "auto"` 时）
///
/// 现场机器狗（瑞芯微 RK 系列）的 ffmpeg 是定制版：**没有 libx264**，
/// 只有 `h264_rkmpp`（硬件）/ `h264_v4l2m2m`（V4L2 硬件）两个 H.264 编码器。
const AUTO_ENCODERS: [&str; 3] = ["libx264", "h264_rkmpp", "h264_v4l2m2m"];

/// 解析 `ffmpeg -encoders` 输出，挑出可用的 H.264 编码器
fn pick_encoder(configured: &str, available: &str) -> Option<String> {
    if configured != "auto" {
        return Some(configured.to_string());
    }
    AUTO_ENCODERS
        .iter()
        .find(|name| {
            available
                .lines()
                .any(|line| line.split_whitespace().any(|word| word == **name))
        })
        .map(|name| name.to_string())
}

/// 探测本机 ffmpeg 支持的编码器（结果缓存，进程内只跑一次）
async fn detect_encoder(config: &Config) -> Option<String> {
    if config.video_encoder != "auto" {
        return Some(config.video_encoder.clone());
    }
    static CACHE: tokio::sync::OnceCell<Option<String>> = tokio::sync::OnceCell::const_new();
    CACHE
        .get_or_init(|| async {
            let output = Command::new(&config.video_ffmpeg)
                .arg("-hide_banner")
                .arg("-encoders")
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .await;
            match output {
                Ok(output) => {
                    let listing = String::from_utf8_lossy(&output.stdout).to_string();
                    match pick_encoder("auto", &listing) {
                        Some(encoder) => {
                            info!("实时视频编码器: {encoder}");
                            Some(encoder)
                        }
                        None => {
                            error!(
                                "本机 ffmpeg 没有可用的 H.264 编码器（试过 {}），无法回传视频；\
                                 请安装带 libx264 的 ffmpeg，或在 [video].encoder 指定编码器",
                                AUTO_ENCODERS.join(" / ")
                            );
                            None
                        }
                    }
                }
                Err(error) => {
                    error!("执行 {} -encoders 失败: {error}", config.video_ffmpeg);
                    None
                }
            }
        })
        .await
        .clone()
}

/// 启动 ffmpeg：RTSP 输入 → H.264 + FLV 输出到 stdout
///
/// 参数取向是**低延迟优先**：`ultrafast` + `zerolatency` + 2 秒一个关键帧，
/// 关键帧间隔决定了播放器起播与丢帧恢复的最长等待。
fn spawn_ffmpeg(
    config: &Config,
    url: &str,
    encoder: &str,
) -> std::io::Result<tokio::process::Child> {
    let mut command = Command::new(&config.video_ffmpeg);
    command.args(ffmpeg_args(config, url, encoder));
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
    use super::{ffmpeg_args, pick_encoder, redact_url};
    use robodog_device_client::config::{CliArgs, Config, EnvConfig, FileConfig};

    fn test_config(with_scale: bool) -> Config {
        // 用默认值构造：不读文件、不读环境，避免用例受现场配置影响
        let mut file = FileConfig::default();
        if with_scale {
            file.video.width = Some(1280);
        }
        Config::resolve(
            &file,
            &EnvConfig::default(),
            &CliArgs::default(),
            Some("TEST-SN"),
        )
        .expect("默认配置应可解析")
    }

    #[test]
    fn ffmpeg_output_options_come_before_the_pipe() {
        let args = ffmpeg_args(
            &test_config(false),
            "rtsp://user:pw@192.168.234.1:8554/test",
            "libx264",
        );
        // 输入参数必须在 -i 之前
        let transport = args
            .iter()
            .position(|arg| arg == "-rtsp_transport")
            .unwrap();
        let wallclock = args
            .iter()
            .position(|arg| arg == "-use_wallclock_as_timestamps")
            .expect("RTSP 输入必须用到达时间当 PTS");
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        assert!(transport < input, "RTSP 传输方式要放在 -i 之前");
        assert_eq!(args[wallclock + 1], "1");
        assert!(wallclock < input, "墙钟时间戳要放在 -i 之前");
        // 输出参数必须在输出 URL 之前，且输出固定是 FLV → pipe:1
        assert_eq!(&args[args.len() - 3..], ["-f", "flv", "pipe:1"]);
        assert!(!args.contains(&"-vf".to_string()), "未配置宽度时不该缩放");
    }

    #[test]
    fn scale_is_inserted_before_output_url() {
        let args = ffmpeg_args(
            &test_config(true),
            "rtsp://192.168.234.1:8554/test",
            "libx264",
        );
        let scale = args.iter().position(|arg| arg == "-vf").unwrap();
        let format = args.iter().position(|arg| arg == "-f").unwrap();
        assert!(scale < format, "缩放参数放在输出 URL 之后会被 ffmpeg 忽略");
        assert_eq!(args[scale + 1], "scale=1280:-2");
    }

    /// 现场「一闪一闪 + 连接中断」的根因之一：ffmpeg 写管道是带缓冲的，
    /// 不加 `-flush_packets` 时小帧被攒成突发数据，浏览器 MSE 反复饿死。
    /// 板端 relay（`deploy/backpack-relay/relay_push.py`）一直带这几个参数，所以它稳。
    #[test]
    fn output_is_flushed_per_packet_for_low_latency() {
        let args = ffmpeg_args(
            &test_config(false),
            "rtsp://192.168.234.1:8554/test",
            "h264_rkmpp",
        );
        let flush = args
            .iter()
            .position(|arg| arg == "-flush_packets")
            .expect("必须逐包 flush，否则浏览器 MSE 会一顿一顿");
        assert_eq!(args[flush + 1], "1");
        for expected in ["-muxdelay", "-muxpreload"] {
            assert!(args.contains(&expected.to_string()), "缺少 {expected}");
        }
        // 这三个都是输出参数，必须在 `-f flv pipe:1` 之前
        let format = args.iter().position(|arg| arg == "-f").unwrap();
        assert!(flush < format, "-flush_packets 放在输出 URL 之后会被忽略");
        // RTSP 输入要补 PTS，-r 恒定帧率输出的时间轴才连续
        let fflags = args.iter().position(|arg| arg == "-fflags").unwrap();
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        assert_eq!(args[fflags + 1], "+genpts");
        assert!(fflags < input, "输入侧的 -fflags 要放在 -i 之前");
    }

    #[test]
    fn non_rtsp_source_has_no_rtsp_option() {
        let args = ffmpeg_args(
            &test_config(false),
            "http://192.168.41.9/live.m3u8",
            "libx264",
        );
        assert!(
            !args.contains(&"-rtsp_transport".to_string()),
            "非 RTSP 地址带 -rtsp_transport 会让 ffmpeg 报错"
        );
        assert!(
            !args.contains(&"-use_wallclock_as_timestamps".to_string()),
            "非 RTSP 地址不需要墙钟时间戳"
        );
    }

    /// 现场机器狗的 ffmpeg 是瑞芯微定制版：识别 `preset` 这类 x264 专属参数会直接
    /// 启动失败（"Unrecognized option 'preset'"），硬件编码器分支必须干净。
    #[test]
    fn hardware_encoder_args_drop_x264_only_options() {
        let args = ffmpeg_args(
            &test_config(true),
            "rtsp://192.168.234.1:8554/test",
            "h264_rkmpp",
        );
        for forbidden in ["-preset", "-tune", "-profile:v", "-sc_threshold"] {
            assert!(
                !args.contains(&forbidden.to_string()),
                "硬件编码器不该带 {forbidden}"
            );
        }
        assert!(args.contains(&"h264_rkmpp".to_string()));
        // 码率 / GOP / 像素格式这些通用参数仍然要有
        for expected in ["-b:v", "-g", "yuv420p", "scale=1280:-2", "flv"] {
            assert!(args.contains(&expected.to_string()), "缺少 {expected}");
        }
    }

    #[test]
    fn auto_encoder_prefers_hardware_when_x264_missing() {
        let rkmpp = " V..... h264_rkmpp  Rockchip MPP H264 encoder (codec h264)\n";
        assert_eq!(pick_encoder("auto", rkmpp).as_deref(), Some("h264_rkmpp"));

        let both = " V..... libx264  libx264 H.264 (codec h264)\n V..... h264_rkmpp\n";
        assert_eq!(pick_encoder("auto", both).as_deref(), Some("libx264"));

        assert_eq!(pick_encoder("auto", " V..... mpeg4  MPEG-4\n"), None);
        // 显式配置时不做探测，照用
        assert_eq!(
            pick_encoder("h264_v4l2m2m", "").as_deref(),
            Some("h264_v4l2m2m")
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
