//! 实时视频中继集成测试。
//!
//! WebSocket 必须走真实 TCP，所以这里不像 `api_smoke.rs` 那样用 `oneshot` 直接打
//! Router，而是监听 127.0.0.1 的随机端口，用真实的 ws:// 客户端复现现场链路：
//!
//! ```text
//! 设备客户端（本测试扮演）──/ws/device/video──► 平台 ──/api/devices/{id}/video/ws──► 浏览器（本测试扮演）
//!         ▲                                          │
//!         └──── /ws/device 收到 video_start/video_stop ┘
//! ```
//!
//! 断言三件事：
//! 1. 观看者连上后，设备端收到 `video_start`，且携带设备管理里配置的 RTSP 地址；
//! 2. 推上来的二进制帧原样转发给观看者（浏览器侧 flv.js 就靠这个字节流解码）；
//! 3. 最后一个观看者断开后，设备端收到 `video_stop`（没人看就停流）。

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

/// 设备管理里配置的现场 RTSP 地址（密码是 URL 编码的 `Abcd#2027!`）
const RTSP_URL: &str = "rtsp://user:Abcd%232027%21@192.168.234.1:8554/test";

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
    });

    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");

    let state = AppState {
        db: pool,
        config,
        device_connections: gateway::new_device_connections(),
        video_hub: Default::default(),
    };
    (state, temp)
}

/// 起真实监听端口，返回端口与 Router（登录用 `oneshot` 即可）
async fn spawn_server(state: AppState) -> (u16, axum::Router) {
    let router = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定随机端口失败");
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

/// 把演示设备改成「在线 + 已配 RTSP 地址」
async fn prepare_device(state: &AppState) -> (i64, String) {
    let row: (i64, String) = sqlx::query_as("SELECT id, device_id FROM devices ORDER BY id LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("演示数据应至少有一台设备");
    sqlx::query("UPDATE devices SET status = '在线待命', video_url = ?1, video_type = 'rtsp' WHERE id = ?2")
        .bind(RTSP_URL)
        .bind(row.0)
        .execute(&state.db)
        .await
        .expect("写入视频地址失败");
    row
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

/// 等下一条指定类型的文本消息，超时即失败（避免用例挂死）
///
/// 设备通道上还有 `ping` 之类的保活消息（心跳定时器首个 tick 是立即触发的），
/// 所以这里按 msgType 过滤而不是取「第一条」。
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
async fn rtsp_stream_is_relayed_to_browser_and_stopped_when_nobody_watches() {
    let (state, _db) = test_state().await;
    let (device_pk, device_code) = prepare_device(&state).await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;

    // 1) 设备端接入主链路（只有连着，平台才能下发 video_start）
    let (mut device_ws, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device"))
        .await
        .expect("设备接入失败");
    device_ws
        .send(Message::Text(
            json!({
                "msgType": "register",
                "ts": "2026-01-01T00:00:00Z",
                "payload": { "deviceId": device_code },
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let ack = next_msg(&mut device_ws, "register_ack").await;
    assert_eq!(ack["payload"]["success"], true, "设备注册应成功: {ack}");

    // 2) 客户端视频推流通道注册
    let (mut publisher, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device/video"))
        .await
        .expect("推流通道连接失败");
    publisher
        .send(Message::Text(
            json!({
                "msgType": "video_register",
                "ts": "2026-01-01T00:00:00Z",
                "payload": { "deviceId": device_code },
            })
            .to_string(),
        ))
        .await
        .unwrap();

    // 3) 浏览器订阅：平台应立刻要求设备端拉流，地址就是设备档案里的那个
    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/devices/{device_pk}/video/ws?token={token}"
    ))
    .await
    .expect("观看通道连接失败");

    let start = next_msg(&mut device_ws, "video_start").await;
    assert_eq!(start["payload"]["url"], RTSP_URL, "应下发设备档案里的地址");

    // 4) 推流：真 FLV 头 + 首关键帧后，字节原样转发给观看者
    let (init, frame) = flv_stream();
    publisher
        .send(Message::Binary(init.clone()))
        .await
        .unwrap();
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

    // 5) 中途加入的观看者：必须补到流头，否则浏览器黑屏
    let (mut late_viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/devices/{device_pk}/video/ws?token={token}"
    ))
    .await
    .expect("第二个观看者连接失败");
    assert_eq!(
        next_binary(&mut late_viewer, "中途加入补流头").await,
        init,
        "中途加入的观看者应收到补发的流头"
    );

    // 中途加入不该重复触发 video_start（设备端只在第一个观看者进来时收到）
    publisher
        .send(Message::Binary(frame.clone()))
        .await
        .unwrap();
    assert_eq!(next_binary(&mut late_viewer, "中途加入收帧").await, frame);

    // 6) 最后一个观看者离开 → 设备端收到 video_stop
    late_viewer.close(None).await.unwrap();
    viewer.close(None).await.unwrap();
    let stop = next_msg(&mut device_ws, "video_stop").await;
    assert_eq!(stop["msgType"], "video_stop", "没人看时应停止拉流");
}

/// 用真实 ffmpeg 生成的 FLV 验证流头解析（手写字节可能和 ffmpeg 的实际布局有出入）
///
/// 本机没装 ffmpeg 时自动跳过：现场机器狗侧才需要它，CI 不应因此变红。
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
    let (device_pk, device_code) = prepare_device(&state).await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;

    // 客户端推流通道
    let (mut publisher, _) = connect_async(format!("ws://127.0.0.1:{port}/ws/device/video"))
        .await
        .expect("推流通道连接失败");
    publisher
        .send(Message::Text(
            json!({
                "msgType": "video_register",
                "ts": "2026-01-01T00:00:00Z",
                "payload": { "deviceId": device_code },
            })
            .to_string(),
        ))
        .await
        .unwrap();

    // 浏览器观看
    let (mut viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/devices/{device_pk}/video/ws?token={token}"
    ))
    .await
    .expect("观看通道连接失败");

    // 用合成画面代替 RTSP，编码参数与客户端 src/video.rs 保持一致
    let mut ffmpeg = tokio::process::Command::new("ffmpeg")
        .args([
            "-hide_banner", "-loglevel", "error", "-nostdin",
            "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=10",
            "-an", "-c:v", "libx264", "-preset", "ultrafast", "-tune", "zerolatency",
            "-profile:v", "baseline", "-pix_fmt", "yuv420p", "-g", "10",
            "-sc_threshold", "0", "-r", "10", "-b:v", "300k",
            "-f", "flv", "pipe:1",
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
        "ws://127.0.0.1:{port}/api/devices/{device_pk}/video/ws?token={token}"
    ))
    .await
    .expect("中途观看者连接失败");
    let init = next_binary(&mut late_viewer, "真实流补头").await;
    assert!(init.starts_with(b"FLV"), "补发的流头应以 FLV 魔数开头");
    assert!(init.len() > 100, "补发的流头应包含序列头与关键帧，实际 {} 字节", init.len());

    pump.abort();
}

#[tokio::test]
async fn viewer_without_video_url_is_rejected_before_upgrade() {
    let (state, _db) = test_state().await;
    let (device_pk, _device_code) = prepare_device(&state).await;
    // 清空地址：升级前就该报错，而不是让浏览器干等
    sqlx::query("UPDATE devices SET video_url = NULL WHERE id = ?1")
        .bind(device_pk)
        .execute(&state.db)
        .await
        .unwrap();
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;

    let error = connect_async(format!(
        "ws://127.0.0.1:{port}/api/devices/{device_pk}/video/ws?token={token}"
    ))
    .await
    .expect_err("未配置视频地址应拒绝升级");
    let text = error.to_string();
    assert!(
        text.contains("400") || text.contains("Bad Request"),
        "期望 400 拒绝，实际: {text}"
    );
}

#[tokio::test]
async fn viewer_requires_token() {
    let (state, _db) = test_state().await;
    let (device_pk, _device_code) = prepare_device(&state).await;
    let (port, _router) = spawn_server(state.clone()).await;

    let error = connect_async(format!(
        "ws://127.0.0.1:{port}/api/devices/{device_pk}/video/ws"
    ))
    .await
    .expect_err("没有令牌应被拒绝");
    let text = error.to_string();
    assert!(
        text.contains("401") || text.contains("Unauthorized"),
        "期望 401 拒绝，实际: {text}"
    );
}
