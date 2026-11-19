//! 背包实时视频中继集成测试。
//!
//! WebSocket 必须走真实 TCP，所以这里不像 `api_smoke.rs` 那样用 `oneshot` 直接打
//! Router，而是监听 127.0.0.1 的随机端口，用真实的 ws:// 客户端复现现场链路：
//!
//! ```text
//! 板端 relay（本测试扮演）──/ws/device/video──► 平台 ──/api/backpack/video/ws──► 浏览器（本测试扮演）
//!         ▲                                         │
//!         └──── 推流连接上收到 video_start/video_stop ┘
//! ```
//!
//! 断言五件事：
//! 1. 观看者连上后，板端 relay 收到 `video_start`（带 `stream`），且只发一次；
//! 2. 推上来的二进制帧原样转发给观看者（浏览器侧 flv.js 就靠这个字节流解码）；
//! 3. 最后一个观看者断开后**不立刻**停流，只进看门狗的空转名单（重连 / 切页签不用等
//!    ffmpeg 重启，现场才不会看到「一闪一闪」）；
//! 4. 同一条流里重新观看（关掉抽屉再打开 / 浏览器 WS 重连）也必须拿到流头。
//! 5. 鉴权与通道白名单在升级前就拦掉（`?token=` + 固定六路 + preview）。
//!
//! 设备客户端那条「拉 `devices.video_url` → 平台中继」的旧链路已删除，
//! 设备管理 / 计划管理 / 实时视频三处画面统一走背包前置 `dogcam`。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use futures_util::{SinkExt, StreamExt};
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::config::Config;
use robodog_ops_server::{db, gateway, seed};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt;

/// 板端 relay 推的房间名（与服务端 `BACKPACK_ROOM_PREFIX` 一致）
const ROOM: &str = "bp:dogcam";

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct TempDb {
    dir: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn test_state() -> (AppState, TempDb) {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-video-test-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    let temp = TempDb { dir: dir.clone() };

    let config = Arc::new(Config {
        port: 0,
        database_path: dir.join("test.db"),
        jwt_secret: "test-secret-at-least-32-bytes-long!!".to_string(),
        jwt_ttl_hours: 1,
        web_dist: dir.join("web-dist"),
        backend_db_path: dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: dir.join("map_ingest"),
        service_token: None,
        zentai_ws_url: None,
        device_timeout_secs: 90,
        task_timeout_secs: 120,
        task_stuck_secs: 600,
        backpack_base_url: None,
        backpack_webrtc_url: None,
        backpack_browser_url: None,
        mediamtx_url: None,
        backpack_tunnel_device: None,
        point_ai: "auto".to_string(),
        ai_window_max_secs: 300,
        public_url: None,
        wecom_enabled: false,
        wecom_webhook: None,
        wecom_min_level: "警告".to_string(),
        wecom_mention_all: false,
    });

    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");

    let state = AppState {
        db: pool,
        notifier: robodog_ops_server::notify::Notifier::disabled(),
        config,
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
    };
    (state, temp)
}

/// 起真实监听端口，返回端口与 Router（登录用 `oneshot` 即可）
async fn spawn_server(state: AppState) -> (u16, axum::Router) {
    let router = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定随机端口失败");
    let port = listener.local_addr().expect("取端口失败").port();
    let serving = router.clone();
    tokio::spawn(async move {
        axum::serve(
            listener,
            serving.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .expect("测试服务运行失败");
    });
    (port, router)
}

async fn login_token(router: &axum::Router) -> String {
    let request = Request::post("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"username":"admin","password":"admin123"}"#))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    body["data"]["token"].as_str().unwrap().to_string()
}

/// 连上推流通道并注册成板端 relay 房间，等平台真的登记成功再返回
async fn publisher_for(
    port: u16,
    state: &AppState,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (mut publisher, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device/video"))
        .await
        .expect("推流通道连接失败");
    publisher
        .send(Message::Text(
            json!({
                "msgType": "video_register",
                "ts": "2026-01-01T00:00:00Z",
                "payload": { "deviceId": ROOM },
            })
            .to_string(),
        ))
        .await
        .unwrap();
    // 观看通道升级前会校验 `has_publisher`：注册是异步处理的，必须等它生效
    tokio::time::timeout(Duration::from_secs(5), async {
        while !state.video_hub.has_publisher(ROOM).await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("推流端注册未被平台登记");
    publisher
}

/// 造一段最小可用的 FLV：header + AVC 序列头 + 关键帧 + 普通帧
///
/// 服务端只在解析出「FLV 头 + SPS/PPS + 首个关键帧」后才开始向观看者下发字节，
/// 所以测试推的必须是真 FLV，否则中继会一直缓存（这正是生产环境的黑屏保护）。
fn flv_stream() -> (Vec<u8>, Vec<u8>) {
    fn tag(tag_type: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag_type];
        let size = (body.len() as u32).to_be_bytes();
        out.extend_from_slice(&size[1..]);
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]); // 时间戳 + streamId
        out.extend_from_slice(body);
        out.extend_from_slice(&(body.len() as u32 + 11).to_be_bytes());
        out
    }

    // FLV header 共 13 字节：魔数 + version + flags + DataOffset + PreviousTagSize0
    let mut init = b"FLV\x01\x05".to_vec();
    init.extend_from_slice(&[0, 0, 0, 9]);
    init.extend_from_slice(&[0, 0, 0, 0]);
    init.extend(tag(9, &[0x17, 0x00, 0x00, 0x00, 0x00])); // AVC 序列头（SPS/PPS）
    init.extend(tag(9, &[0x17, 0x01, 0x00, 0x00, 0x00])); // 关键帧 IDR

    let frame = tag(9, &[0x27, 0x01, 0x00, 0x00, 0x00]); // P 帧
    (init, frame)
}

/// 造一个带指定时间戳的视频 tag（P 帧）
///
/// 用来模拟「媒体时间只走了一点、墙钟却走了很多」：观看端落后直播时就会收到这种字节。
fn video_tag(ts_ms: u32) -> Vec<u8> {
    let body = [0x27_u8, 0x01, 0x00, 0x00, 0x00];
    let mut out = vec![9_u8];
    let size = (body.len() as u32).to_be_bytes();
    out.extend_from_slice(&size[1..]);
    // FLV 时间戳：低 24 位大端 + 扩展字节（高位）
    out.extend_from_slice(&[
        (ts_ms >> 16) as u8,
        (ts_ms >> 8) as u8,
        ts_ms as u8,
        (ts_ms >> 24) as u8,
    ]);
    out.extend_from_slice(&[0, 0, 0]); // streamId
    out.extend_from_slice(&body);
    out.extend_from_slice(&((body.len() as u32 + 11).to_be_bytes()));
    out
}

/// 等下一条指定类型的文本消息，超时即失败（避免用例挂死）
async fn next_msg(
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    msg_type: &str,
) -> Value {
    let deadline = Duration::from_secs(5);
    let read = tokio::time::timeout(deadline, async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(text))) => {
                    let value: Value = serde_json::from_str(&text).expect("消息不是合法 JSON");
                    if value["msgType"] == msg_type {
                        return value;
                    }
                }
                Some(Ok(_)) => continue,
                Some(Err(error)) => panic!("{msg_type} 连接异常: {error}"),
                None => panic!("{msg_type} 连接已关闭"),
            }
        }
    })
    .await;
    read.unwrap_or_else(|_| panic!("{msg_type} 在 {deadline:?} 内没有收到"))
}

/// 等下一帧二进制数据，超时即失败
async fn next_binary(
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    what: &str,
) -> Vec<u8> {
    let deadline = Duration::from_secs(5);
    let read = tokio::time::timeout(deadline, async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Binary(bytes))) => return bytes.to_vec(),
                Some(Ok(_)) => continue,
                Some(Err(error)) => panic!("{what} 连接异常: {error}"),
                None => panic!("{what} 连接已关闭"),
            }
        }
    })
    .await;
    read.unwrap_or_else(|_| panic!("{what} 在 {deadline:?} 内没有收到数据"))
}

#[tokio::test]
async fn backpack_stream_is_relayed_to_browser_and_stops_after_a_grace_period() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    // 1) 浏览器订阅：平台应要求板端起流，且带的就是这个通道
    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("观看通道连接失败");

    let start = next_msg(&mut publisher, "video_start").await;
    assert_eq!(start["payload"]["stream"], "dogcam", "应下发当前通道名");

    // 2) 推流：真 FLV 头 + 首关键帧后，字节原样转发给观看者
    let (init, frame) = flv_stream();
    publisher.send(Message::Binary(init.clone())).await.unwrap();
    assert_eq!(
        next_binary(&mut viewer, "首个观看者补流头").await,
        init,
        "观看端应从 FLV 魔数开始收到流"
    );

    publisher
        .send(Message::Binary(frame.clone()))
        .await
        .unwrap();
    assert_eq!(
        next_binary(&mut viewer, "首个观看者收帧").await,
        frame,
        "推上来的帧应原样转发"
    );

    // 3) 中途加入的观看者：补「流头 + **当前 GOP**」，否则浏览器 MSE 里会出现
    //    「几百秒空洞 + 缺参考帧的 P 帧」→ 现场表现为「字节还在流、画面定住」。
    let (mut late_viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("第二个观看者连接失败");
    let mut expected = init.clone();
    expected.extend_from_slice(&frame); // 当前 GOP：关键帧 + 已推过的 P 帧
    assert_eq!(
        next_binary(&mut late_viewer, "中途加入补流").await,
        expected,
        "中途加入的观看者应收到「流头 + 当前 GOP」（时间轴连续、参考帧完整）"
    );

    // 中途加入不该重复触发 video_start（推流端是常驻连接，重复起流会白换一条 FLV）
    publisher
        .send(Message::Binary(frame.clone()))
        .await
        .unwrap();
    assert_eq!(next_binary(&mut late_viewer, "中途加入收帧").await, frame);
    let duplicate_start = tokio::time::timeout(
        Duration::from_millis(500),
        next_msg(&mut publisher, "video_start"),
    )
    .await;
    assert!(
        duplicate_start.is_err(),
        "中途加入的观看者不应再触发一次 video_start"
    );

    // 4) 最后一个观看者离开：**不立刻**停流 —— 现场关掉抽屉再打开、切页签、浏览器
    //    WS 抖一下就重连，每次都重启 ffmpeg 要黑 3~7s 且换一条新 FLV，看起来就是
    //    「一闪一闪的连接中断」。改成留给看门狗（~15s）收尾。
    late_viewer.close(None).await.unwrap();
    viewer.close(None).await.unwrap();
    let early_stop = tokio::time::timeout(
        Duration::from_millis(800),
        next_msg(&mut publisher, "video_stop"),
    )
    .await;
    assert!(
        early_stop.is_err(),
        "最后一个观看者离开后不应立刻停流，要留重连 / 切页签的宽限"
    );

    // 观看者计数归零后，这路流进入看门狗的空转名单（宽限到期才下发 video_stop）。
    // 这里用 `Duration::ZERO` 模拟「宽限已过」，看门狗真实用的是 15s 宽限。
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if state.video_hub.viewers(ROOM).await == 0
                && state
                    .video_hub
                    .idle_publishers(Duration::ZERO)
                    .await
                    .contains(&ROOM.to_string())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("没人看之后应进入看门狗的空转名单");

    // 宽限期（15s）内不应该进入空转名单：浏览器刷新 / 切页签是常态
    assert!(
        !state
            .video_hub
            .idle_publishers(Duration::from_secs(15))
            .await
            .contains(&ROOM.to_string()),
        "15s 宽限内不许停流"
    );

    // 5) 宽限期内重新打开（关抽屉再打开 / 浏览器重连）：流还在跑，新观看者**立刻**
    //    拿到（流头 + 当前 GOP）字节，不必再等一次 ffmpeg 启动，也不会黑屏。
    //    到这里已经推过 init + 2 个 P 帧，所以补的就是这几段（时间轴连续）。
    let (mut reopen, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("重开的观看者连接失败");
    let mut expected = init.clone();
    expected.extend_from_slice(&frame);
    expected.extend_from_slice(&frame);
    assert_eq!(
        next_binary(&mut reopen, "重开补流").await,
        expected,
        "重开的观看者必须立刻拿到流头 + 当前 GOP（不必等 ffmpeg）"
    );
}

/// 上一轮的流头（init）还在时，**不许**因此跳过 `video_start`。
///
/// 现场 2026-11-18：最后一个人看完 → 看门狗 `video_stop` → 板端杀掉 ffmpeg，但房间里的
/// `init` 还留着。旧逻辑「有 init 且有 publisher 就不发 video_start」会让板端永远不重新
/// 起流，浏览器只能收到 40KB 旧流头 → 20s 连接超时 → 重连，表现为精确每 23s 断一次。
#[tokio::test]
async fn stale_stream_header_does_not_block_video_start() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    // 第一轮：有人看 → 起流 → 推流头
    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("观看通道连接失败");
    next_msg(&mut publisher, "video_start").await;
    let (init, _frame) = flv_stream();
    publisher.send(Message::Binary(init.clone())).await.unwrap();
    assert_eq!(next_binary(&mut viewer, "首轮流头").await, init);

    // 第一个人离开
    viewer.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.video_hub.viewers(ROOM).await != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("观看者计数应归零");

    // 模拟「流已经停了，但上一轮的 init 没被清掉」（旧逻辑就是被它骗过去的）
    state.video_hub.mark_streaming(ROOM, false).await;
    assert!(
        state.video_hub.has_publisher(ROOM).await,
        "推流连接应该还在（只是 ffmpeg 停了）"
    );

    // 新观看者进来：必须再次向板端要流，否则永远只有一段旧流头
    let (_again, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("第二个观看者连接失败");
    tokio::time::timeout(
        Duration::from_secs(5),
        next_msg(&mut publisher, "video_start"),
    )
    .await
    .expect("残留流头不应阻止 video_start（板端不会重新起流 → 每 23s 断一次）");
}

/// 推流端重连（上一条连接已经死了、平台还没判死）时必须**补发** `video_start`。
///
/// 现场时序（2026-11-19「起流黑好大一会」）：
///
/// ```text
/// T0 观看者进来 → 平台下发 video_start → 板端起 ffmpeg（还没有任何字节）
/// T1 Wi-Fi 抖一下 → 板端 relay 断开（同时杀掉 ffmpeg），平台那条连接还没被写活
/// T2 1s 后 relay 重连 → publisher_attach 沿用房间；这一轮没有字节，所以既不 reset
///    也不踢人 → 观看者手上那条 video_start 早就打在死连接上丢了
/// T3 观看者干等到前端 20s 连接超时 → 重连 → 才重新要流
/// ```
///
/// 修法：推流端重连登记控制通道时，若「有人在看 + 房间里一个字节都没有」就补发一次
/// （板端 `StreamSession.start()` 幂等，真有流时重复下发不会重启 ffmpeg）。
#[tokio::test]
async fn relay_reconnect_resends_video_start_to_waiting_viewers() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    // T0：老推流连接在线，观看者进来，它的 video_start 打在这条（马上就死的）通道上
    let _old_publisher = publisher_for(port, &state).await;
    let (_viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("观看通道连接失败");
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.video_hub.viewers(ROOM).await == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("观看者未被登记");

    // T2：板端 relay 重连（老连接还挂在平台上，房间沿用、这一轮还没有任何字节）
    let mut new_publisher = publisher_for(port, &state).await;

    // 新连接必须重新拿到 video_start，否则观看者只能黑到前端超时重连
    let start = next_msg(&mut new_publisher, "video_start").await;
    assert_eq!(
        start["payload"]["stream"], "dogcam",
        "补发的 video_start 要带通道名（房间名去掉 bp: 前缀）: {start}"
    );
    assert_eq!(
        start["payload"]["profile"]["tier"], "degrade",
        "补发时也要带上平台配置的档位: {start}"
    );
}

/// 用真实 ffmpeg 生成的 FLV 验证流头解析（手写字节可能和 ffmpeg 的实际布局有出入）
///
/// 本机没装 ffmpeg 时自动跳过：现场板端侧才需要它，CI 不应因此变红。
#[tokio::test]
async fn real_ffmpeg_flv_is_relayed_with_headers() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("跳过：本机没有 ffmpeg");
        return;
    }

    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    // 浏览器观看
    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("观看通道连接失败");
    next_msg(&mut publisher, "video_start").await;

    // 用合成画面代替 RTSP，编码参数与板端 relay 保持一致
    let mut ffmpeg = tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=10",
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-profile:v",
            "baseline",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "10",
            "-sc_threshold",
            "0",
            "-r",
            "10",
            "-b:v",
            "300k",
            "-f",
            "flv",
            "pipe:1",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("启动 ffmpeg 失败");
    let mut stdout = ffmpeg.stdout.take().expect("取 ffmpeg stdout 失败");

    // 持续把 ffmpeg 输出推给平台
    let pump = tokio::spawn(async move {
        let mut buf = vec![0_u8; 8192];
        loop {
            let read = tokio::io::AsyncReadExt::read(&mut stdout, &mut buf).await;
            match read {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if publisher
                        .send(Message::Binary(buf[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });

    let first = next_binary(&mut viewer, "真实 ffmpeg 流").await;
    assert!(
        first.starts_with(b"FLV"),
        "观看端应收到从 FLV 魔数开始的字节，实际 {:02x?}",
        &first[..first.len().min(16)]
    );

    // 中途加入的观看者也要补到流头（真实流的流头布局由 ffmpeg 决定）
    let (mut late_viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("中途观看者连接失败");
    let init = next_binary(&mut late_viewer, "真实流补头").await;
    assert!(init.starts_with(b"FLV"), "补发的流头应以 FLV 魔数开头");
    assert!(
        init.len() > 100,
        "补发的流头应包含序列头与关键帧，实际 {} 字节",
        init.len()
    );

    pump.abort();
}

#[tokio::test]
async fn viewer_without_publisher_is_rejected_before_upgrade() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;

    let error = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect_err("板端推流未连接时应拒绝升级");
    let text = error.to_string();
    assert!(
        text.contains("400") || text.contains("Bad Request"),
        "期望 400 拒绝，实际: {text}"
    );
}

#[tokio::test]
async fn unknown_backpack_stream_is_rejected() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;

    let error = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=../../etc/passwd&token={token}"
    ))
    .await
    .expect_err("白名单之外的通道应被拒绝");
    let text = error.to_string();
    assert!(
        text.contains("400") || text.contains("Bad Request"),
        "期望 400 拒绝，实际: {text}"
    );
}

#[tokio::test]
async fn viewer_requires_token() {
    let (state, _db) = test_state().await;
    let (port, _router) = spawn_server(state.clone()).await;

    let error = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam"
    ))
    .await
    .expect_err("没有令牌应被拒绝");
    let text = error.to_string();
    assert!(
        text.contains("401") || text.contains("Unauthorized"),
        "期望 401 拒绝，实际: {text}"
    );
}

/// 板端 relay 重启 ffmpeg（`video_reset`）时，**已经连着的观看端必须被断开**。
///
/// 现场事故（2026-11-18）：同一条推流连接上接着发新一轮 FLV（时间戳从 0 重来、
/// 降档后分辨率还从 720p 变 640x360），浏览器里的 flv.js 不会自己恢复 ——
/// 画面定住、字节却还在流（`pendingBytes` 一直是 0），只能等前端的假死看门狗。
/// 现在由平台主动断开：前端 250ms 起重连，重连时补到的是**新流头**。
#[tokio::test]
async fn video_reset_disconnects_attached_viewers_so_they_refetch_the_header() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("观看通道连接失败");
    let (init, frame) = flv_stream();
    publisher.send(Message::Binary(init.clone())).await.unwrap();
    assert_eq!(
        next_binary(&mut viewer, "首个观看者补流头").await,
        init,
        "观看端应从 FLV 魔数开始收到流"
    );
    publisher
        .send(Message::Binary(frame.clone()))
        .await
        .unwrap();
    assert_eq!(next_binary(&mut viewer, "首个观看者收帧").await, frame);

    // 板端 relay 重启 ffmpeg：同一条推流连接上声明「新一轮流」
    publisher
        .send(Message::Text(
            json!({ "msgType": "video_reset", "payload": { "stream": "dogcam" } }).to_string(),
        ))
        .await
        .unwrap();

    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match viewer.next().await {
                None => return true,
                Some(Ok(Message::Close(_))) => return true,
                Some(Err(_)) => return true,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .expect("video_reset 之后老观看端应被断开");
    assert!(closed, "老观看端必须重连才能拿到新流头");

    // 重连的观看端拿到的是新一轮的流头（旧头已经被 reset 清掉）
    let (mut fresh, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("重连观看通道失败");
    publisher.send(Message::Binary(init.clone())).await.unwrap();
    assert_eq!(
        next_binary(&mut fresh, "重连后补新一轮流头").await,
        init,
        "重连的观看端必须拿到新一轮流头，否则 flv.js 解析错位、画面定住"
    );
}

/// 冷启动那一条 `video_reset` 不能踢人。
///
/// 时序：观看者进来 → 平台下发 `video_start` → 板端起 ffmpeg **前**先发 `video_reset`
/// → 平台清流头。这一轮之前根本没有流（`init` / `last_frame` 都是空），观看者手上
/// 没有「坏字节」，踢掉只会让首帧白等一次重连（`measure_bitrate` 这类一次性探针
/// 会直接拿 1006 断连）。
#[tokio::test]
async fn video_reset_on_a_cold_start_does_not_kick_the_viewer() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("观看通道连接失败");
    let _start = next_msg(&mut publisher, "video_start").await;

    // 板端 relay 起 ffmpeg 前的 video_reset：这时房间里还没有任何流
    publisher
        .send(Message::Text(
            json!({ "msgType": "video_reset", "payload": { "stream": "dogcam" } }).to_string(),
        ))
        .await
        .unwrap();

    let (init, frame) = flv_stream();
    publisher.send(Message::Binary(init.clone())).await.unwrap();
    assert_eq!(
        next_binary(&mut viewer, "冷启动后首个观看者补流头").await,
        init,
        "冷启动的 video_reset 不该断开观看者，它应该直接收到新流头"
    );
    publisher
        .send(Message::Binary(frame.clone()))
        .await
        .unwrap();
    assert_eq!(next_binary(&mut viewer, "冷启动后收帧").await, frame);
}

/// 观看端落后直播超过阈值就断开，让它重连到直播边缘。
///
/// 现场背景（2026-11-18）：上行抖了 45 秒，板端 ffmpeg 恢复后以 3 倍速把攒下的画面倒
/// 出来（日志 `last viewer ... 6.9s / 2964 KB → 3533 kbps`），平台给每个观看者留的
/// 1MB 缓冲能把这些全吃下去 —— 于是观众看的是十几秒前的画面，而且**不会自己追回来**：
/// 没有任何日志，UI 上也看不出来（画面很流畅，只是比现场晚）。
/// 现在按 FLV 视频 tag 的时间戳量「墙钟流逝 - 媒体流逝」，超过 2s 就断开。
#[tokio::test]
async fn viewer_falling_behind_live_is_disconnected() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("观看通道连接失败");
    let _start = next_msg(&mut publisher, "video_start").await;

    let (init, _frame) = flv_stream();
    publisher.send(Message::Binary(init.clone())).await.unwrap();
    assert_eq!(next_binary(&mut viewer, "补流头").await, init);

    // 正常节奏：媒体时间跟着墙钟走（100ms 一片）
    publisher
        .send(Message::Binary(video_tag(100)))
        .await
        .unwrap();
    assert_eq!(next_binary(&mut viewer, "正常帧").await, video_tag(100));

    // 链路卡了 3 秒，媒体时间只往前走了 100ms → 这个观看者落后直播 ≈2.9s
    tokio::time::sleep(Duration::from_millis(3000)).await;
    publisher
        .send(Message::Binary(video_tag(200)))
        .await
        .unwrap();

    let disconnected = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match viewer.next().await {
                None => return true,
                Some(Ok(Message::Close(_))) => return true,
                Some(Err(_)) => return true,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .expect("落后直播超过阈值时平台应断开这个观看端");
    assert!(disconnected, "否则观看端会一直「慢几秒」地看下去");

    assert!(
        state.video_hub.has_publisher(ROOM).await,
        "只该断开观看端，推流房间要留着（重连后马上有画面）"
    );
    assert_eq!(state.video_hub.viewers(ROOM).await, 0);
}

/// 中继默认档位：`video_start` 要带平台配置（页面「系统设置 → 视频中继」），
/// 改设置后立刻广播 `video_config` 给在线板端 —— 正在看的流就地换档，不用等下一次起流。
///
/// 现场背景：板子出公网主要走 4G 模块，720p15/1000k 起流就被反压；默认档位是
/// 省流档 640x360@12/500k，板端再按链路自动降一档到 320x180@8/200k。
#[tokio::test]
async fn relay_profile_defaults_and_updates_reach_the_board() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    let (_viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&token={token}"
    ))
    .await
    .expect("观看通道连接失败");

    let start = next_msg(&mut publisher, "video_start").await;
    assert_eq!(start["payload"]["stream"], "dogcam");
    assert_eq!(
        start["payload"]["profile"]["tier"], "degrade",
        "4G 场景默认档位应是省流档（640x360@12/500k）: {start}"
    );
    assert_eq!(
        start["payload"]["profile"]["auto"], true,
        "默认允许板端按链路自动降档: {start}"
    );

    // 页面保存设置 → 平台广播 video_config（板端收到后就地换档，观看端不重连）
    sqlx::query(
        "UPDATE settings SET setting_value = 'degrade2' WHERE setting_key = 'video_relay_profile'",
    )
    .execute(&state.db)
    .await
    .unwrap();
    let sent = robodog_ops_server::video::broadcast_relay_profile(&state).await;
    assert_eq!(sent, 1, "在线的那条推流连接应该收到新配置");

    let config = next_msg(&mut publisher, "video_config").await;
    assert_eq!(config["payload"]["profile"]["tier"], "degrade2");
    assert_eq!(config["payload"]["profile"]["auto"], true);
}

/// 平台 WebRTC 通路：浏览器开 `media=0` 的观看通道 —— **只计数、不传字节**。
///
/// 板端把媒体用 RTSP 推给平台 mediamtx（`business/docs/backpack-video-integration.md` §7），
/// 这条 WS 上根本不会有 FLV 字节。它仍然必须：
/// 1. 触发 `video_start`（否则板端不会起 ffmpeg，按需推流就没了）；
/// 2. 计入观看者（否则看门狗 30s 后下发 `video_stop` 把流停掉）；
/// 3. 一个字节都不下发（否则等于把 1.2Mbps 白推一遍给浏览器）。
#[tokio::test]
async fn control_only_viewer_triggers_the_stream_but_receives_no_media() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&media=0&token={token}"
    ))
    .await
    .expect("观看通道连接失败");

    let start = next_msg(&mut publisher, "video_start").await;
    assert_eq!(start["payload"]["stream"], "dogcam");
    assert_eq!(
        state.video_hub.viewers(ROOM).await,
        1,
        "WHEP 模式下这条连接就是「有人在看」的唯一凭据"
    );

    // 推流端即便真有字节上来，也不能往这条连接发
    let (init, frame) = flv_stream();
    publisher.send(Message::Binary(init)).await.unwrap();
    publisher.send(Message::Binary(frame)).await.unwrap();

    let leaked = tokio::time::timeout(Duration::from_millis(600), async {
        loop {
            match viewer.next().await {
                Some(Ok(Message::Binary(bytes))) => return Some(bytes.len()),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .unwrap_or(None);
    assert_eq!(leaked, None, "只计数的那条连接不该收到媒体字节");

    // 断开就少一个观看者（宽限期内不停流）
    viewer.close(None).await.ok();
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.video_hub.viewers(ROOM).await != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("观看者断开后计数应归零");
}

/// 板端上报 `video_transport` → `video/state` 的 `transport` 给前端选播放器。
///
/// 这条字段是前端**唯一**的判据：两条通路在平台上都表现为「房间里有推流端」，
/// 靠猜会把 WHEP 会话打到一条没有字节的 FLV 上（或者反过来，永远等不到画面）。
#[tokio::test]
async fn video_transport_is_reported_to_the_console() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    let transport = |state: &AppState| {
        let state = state.clone();
        async move {
            let value = state.video_hub.debug_state().await;
            value["rooms"]
                .as_array()
                .and_then(|rooms| rooms.iter().find(|room| room["room"] == ROOM))
                .map(|room| room["transport"].clone())
                .unwrap_or(Value::Null)
        }
    };
    assert_eq!(transport(&state).await, Value::Null, "还没上报时是 null（按 FLV 走）");

    publisher
        .send(Message::Text(
            json!({
                "msgType": "video_transport",
                "payload": { "stream": "dogcam", "transport": "rtsp" },
            })
            .to_string(),
        ))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        while transport(&state).await != json!("rtsp") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("video_transport 应刷到房间状态里");

    // 诊断接口也要能看到（现场排障只看这一个接口）
    let request = Request::get("/api/backpack/video/state")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let room = body["data"]["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|room| room["room"] == ROOM)
        .expect("诊断接口里应有这一路");
    assert_eq!(room["transport"], "rtsp");
}

/// WHEP 反代的两个前置条件：本部署没开 WebRTC 出口 → 503（前端据此提示）；
/// 通道不在白名单 → 400（路径来自 URL，不能让它替我们创建任意 mediamtx path）。
#[tokio::test]
async fn whep_proxy_rejects_unknown_streams_and_unconfigured_deployments() {
    let (state, _db) = test_state().await;
    let (_port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;

    let call = |method: &str, path: &str| {
        let router = router.clone();
        let token = token.clone();
        let method = method.to_string();
        let path = path.to_string();
        async move {
            let request = Request::builder()
                .method(method.as_str())
                .uri(format!("{path}?token={token}"))
                .body(Body::empty())
                .unwrap();
            router.oneshot(request).await.unwrap().status()
        }
    };

    // 注意这里用 POST 而不是 OPTIONS：全站的 `CorsLayer` 会先吃掉**任何** OPTIONS
    // （回 200 空响应），所以 WHEP 的 OPTIONS 根本到不了反代 —— 这也意味着
    // `lib/whep.ts` 取不到 `Link: <…>; rel="ice-server"`。当前 mediamtx 有公网地址、
    // 不需要 STUN，所以没影响；将来要接 TURN 得换一种下发方式（见文档 §7.6）。
    assert_eq!(
        call("POST", "/api/backpack/whep/dogcam/whep").await,
        StatusCode::SERVICE_UNAVAILABLE,
        "未配置 ROBODOG_MEDIAMTX_URL 时应明确说「本部署没有 WebRTC 出口」"
    );
    assert_eq!(
        call("POST", "/api/backpack/whep/backdoor/whep").await,
        StatusCode::BAD_REQUEST,
        "任意 path 不能透传给 mediamtx"
    );
    assert_eq!(
        call("POST", "/api/backpack/whep/dogcam/config/get").await,
        StatusCode::BAD_REQUEST,
        "只反代 WHEP，mediamtx 的其它端点不能出去"
    );
}
