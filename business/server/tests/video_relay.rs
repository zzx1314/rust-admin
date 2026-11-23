//! 背包实时视频**控制面**的集成测试。
//!
//! WebSocket 必须走真实 TCP，所以这里不像 `api_smoke.rs` 那样用 `oneshot` 直接打
//! Router，而是监听 127.0.0.1 的随机端口，用真实的 ws:// 客户端复现现场链路：
//!
//! ```text
//! 板端 relay（本测试扮演）──/ws/device/video──► 平台 ◄──/api/backpack/video/ws── 浏览器（本测试扮演）
//!         ▲                        │
//!         └── video_start/video_stop ┘       媒体：板端 RTSP → 平台 mediamtx → 浏览器 WHEP
//! ```
//!
//! 断言这些事：
//! 1. 观看者连上后，板端 relay 收到 `video_start`（带 `stream` + 平台档位），且只发一次；
//! 2. 观看通道**一个媒体字节都不转发**（媒体是浏览器 ⇄ mediamtx 的 WebRTC）；
//! 3. 最后一个观看者断开后**不立刻**停流，只进看门狗的空转名单（重连 / 切页签不用等
//!    ffmpeg 重启，现场才不会看到「一闪一闪」）；
//! 4. 推流端重连时要**补发** `video_start`（上一条连接上那条打丢了）；
//! 5. 鉴权与通道白名单在升级前就拦掉（`?token=` + 固定七路 + preview）。
//!
//! 2026-11-19 起媒体走平台 mediamtx 的 WebRTC 出口（`business/docs/backpack-video-integration.md`
//! §7），FLV over WS 那一整套（补流头 / 当前 GOP / 积压断开）已删除，对应用例一并删掉。

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
        ai_patrol: Default::default(),
        caches: Default::default(),
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

/// 连观看通道（`media=0`：只计数、不传媒体）
async fn viewer_for(
    port: u16,
    token: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let (viewer, _) = connect_async(format!(
        "ws://127.0.0.1:{port}/api/backpack/video/ws?stream=dogcam&media=0&token={token}"
    ))
    .await
    .expect("观看通道连接失败");
    viewer
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

/// 等 `duration` 内**没有**收到媒体字节（观看通道只做计数）
async fn expect_no_media(
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    duration: Duration,
) {
    let leaked = tokio::time::timeout(duration, async {
        loop {
            match stream.next().await {
                Some(Ok(Message::Binary(bytes))) => return Some(bytes.len()),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return None,
                Some(Ok(_)) => continue,
            }
        }
    })
    .await
    .unwrap_or(None);
    assert_eq!(
        leaked, None,
        "观看通道不该收到任何媒体字节（媒体走 WebRTC）"
    );
}

#[tokio::test]
async fn viewer_triggers_the_stream_and_leaves_a_grace_period_before_stop() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    // 1) 浏览器订阅：平台应要求板端起流，且带的就是这个通道
    let mut viewer = viewer_for(port, &token).await;
    let start = next_msg(&mut publisher, "video_start").await;
    assert_eq!(start["payload"]["stream"], "dogcam", "应下发当前通道名");
    assert_eq!(
        start["payload"]["profile"]["tier"], "degrade",
        "起流要带上平台配置的档位: {start}"
    );

    // 2) 第二个观看者：不该再触发一次 video_start
    //    （推流端是常驻连接，重复起流会白重启一次 ffmpeg）
    let mut late = viewer_for(port, &token).await;
    let duplicate = tokio::time::timeout(
        Duration::from_millis(500),
        next_msg(&mut publisher, "video_start"),
    )
    .await;
    assert!(duplicate.is_err(), "第二个观看者不应再触发一次 video_start");

    // 3) 老板端还会往推流连接上推 FLV 字节：平台丢弃，绝不转给观看端
    publisher
        .send(Message::Binary(b"FLV\x01\x05not-for-you".to_vec()))
        .await
        .unwrap();
    expect_no_media(&mut late, Duration::from_millis(600)).await;

    // 4) 最后一个观看者离开：**不立刻**停流 —— 现场关掉抽屉再打开、切页签、浏览器
    //    WS 抖一下就重连，每次都重启 ffmpeg 要黑 3~7s，看起来就是「一闪一闪的连接中断」。
    //    改成留给看门狗（~15s）收尾。
    late.close(None).await.unwrap();
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
    // 这里用 `Duration::ZERO` 模拟「宽限已过」，看门狗真实用的是 30s 宽限。
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

    // 宽限期（30s）内不应该进入空转名单：浏览器刷新 / 切页签是常态
    assert!(
        !state
            .video_hub
            .idle_publishers(Duration::from_secs(30))
            .await
            .contains(&ROOM.to_string()),
        "30s 宽限内不许停流"
    );

    // 5) 宽限期内重新打开（关抽屉再打开 / 浏览器重连）：**要重新要一次流**。
    //    平台看不见 mediamtx 里到底有没有字节（媒体不经过本进程），所以不能靠「推流端
    //    还在线」去省这次 `video_start`；板端 `start()` 幂等，重复下发不会重启 ffmpeg。
    let _reopen = viewer_for(port, &token).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        next_msg(&mut publisher, "video_start"),
    )
    .await
    .expect("重开的观看者要重新要一次流（不能因为「看起来还在推」就省掉）");
}

/// 上一轮"看起来还在推"时，**不许**因此跳过 `video_start`。
///
/// 现场 2026-11-18：最后一个人看完 → 看门狗 `video_stop` → 板端杀掉 ffmpeg，但平台侧
/// 只看得到「推流连接还在」。旧逻辑「房间里有流 + publisher 在线就不发 video_start」
/// 会让板端永远不重新起流，浏览器干等到超时 → 重连，表现为精确每 23s 断一次。
#[tokio::test]
async fn stale_stream_does_not_block_video_start() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    // 第一轮：有人看 → 起流 → 看门狗停流（板端 ffmpeg 被杀，推流连接还在）
    let mut viewer = viewer_for(port, &token).await;
    next_msg(&mut publisher, "video_start").await;
    viewer.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.video_hub.viewers(ROOM).await != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("观看者计数应归零");
    state.video_hub.mark_streaming(ROOM, false).await;
    assert!(
        state.video_hub.has_publisher(ROOM).await,
        "推流连接应该还在（只是 ffmpeg 停了）"
    );

    // 新观看者进来：必须再次向板端要流，否则永远是黑的
    let _again = viewer_for(port, &token).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        next_msg(&mut publisher, "video_start"),
    )
    .await
    .expect("残留状态不应阻止 video_start（板端不会重新起流 → 每 23s 断一次）");
}

/// 推流端重连（上一条连接已经死了、平台还没判死）时必须**补发** `video_start`。
///
/// 现场时序（2026-11-19「起流黑好大一会」）：
///
/// ```text
/// T0 观看者进来 → 平台下发 video_start → 板端起 ffmpeg
/// T1 Wi-Fi 抖一下 → 板端 relay 断开（同时杀掉 ffmpeg），平台那条连接还没被写活
/// T2 1s 后 relay 重连 → publisher_attach 沿用房间
///    → 观看者手上那条 video_start 早就打在死连接上丢了，板端根本没起流
/// T3 观看者干等到前端超时 → 重连 → 才重新要流
/// ```
///
/// 修法：推流端重连登记控制通道时，只要「有人在看」就补发一次
/// （板端 `StreamSession.start()` 幂等，真有流时重复下发不会重启 ffmpeg）。
#[tokio::test]
async fn relay_reconnect_resends_video_start_to_waiting_viewers() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    // T0：老推流连接在线，观看者进来，它的 video_start 打在这条（马上就死的）通道上
    let _old_publisher = publisher_for(port, &state).await;
    let _viewer = viewer_for(port, &token).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.video_hub.viewers(ROOM).await == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("观看者未被登记");

    // T2：板端 relay 重连（老连接还挂在平台上，房间沿用）
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

/// 板端起流前的 `video_reset` 只是「新一轮流开始了」的日志，**不能**把观看者踢下线。
///
/// 媒体在 mediamtx 里，重连 ffmpeg 之后 path 会被销毁重建，浏览器那边由 `lib/whep.ts`
/// 自己重协商；平台再去断观看通道，只会让「有人在看」的计数瞬间掉成 0，看门狗跟着
/// 下发 `video_stop`，把刚被叫起来的流又停掉。
#[tokio::test]
async fn video_reset_keeps_the_viewer_attached() {
    let (state, _db) = test_state().await;
    let (port, router) = spawn_server(state.clone()).await;
    let token = login_token(&router).await;
    let mut publisher = publisher_for(port, &state).await;

    let mut viewer = viewer_for(port, &token).await;
    next_msg(&mut publisher, "video_start").await;

    // 板端 relay 重启 ffmpeg：同一条推流连接上声明「新一轮流」
    publisher
        .send(Message::Text(
            json!({ "msgType": "video_reset", "payload": { "stream": "dogcam" } }).to_string(),
        ))
        .await
        .unwrap();

    // 观看通道保持连着、计数还是 1，也没有媒体字节冒出来
    expect_no_media(&mut viewer, Duration::from_millis(500)).await;
    assert_eq!(
        state.video_hub.viewers(ROOM).await,
        1,
        "video_reset 不该动观看者计数"
    );
    assert!(
        state.video_hub.has_publisher(ROOM).await,
        "房间要留着（推流端还在）"
    );
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

    let _viewer = viewer_for(port, &token).await;

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

/// 板端上报 `video_transport` → `video/state` 的 `transport` 给前端。
///
/// 现在只可能是 `rtsp`；这个字段是「那台板子的 relay 有没有升级」的唯一凭据 ——
/// 老版本报 `ws`，前端据此直接把话说清楚（而不是让人对着「起流中」干等）。
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
    assert_eq!(
        transport(&state).await,
        Value::Null,
        "还没上报时是 null（前端据此提示板端未升级）"
    );

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
    assert_eq!(room["publisher"], true);
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
