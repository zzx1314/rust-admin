//! 业务连接主循环：注册、心跳/进度/事件定时器与本体链路的 select 调度。

use crate::agent_relay::AgentRelay;
use crate::ai_patrol::AiPatrol;
use crate::body::BodyContext;
use crate::body::BodyLink;
use crate::body::finish_relink;
use crate::body::start_relink;
use crate::body::wait_body_down;
use crate::handlers::handle_server_message;
use crate::lights::PointLights;
use crate::maps::MapSyncRunning;
use crate::maps::sync_maps_from_robot;
use crate::photo::Photographer;
use crate::record::Recorder;
use crate::report::ProgressReporter;
use crate::report::progress_pct;
use crate::signal::SignalProbe;
use crate::speak::Speaker;
use crate::state::ActiveTask;
use crate::state::RobotState;
use crate::state::Shared;
use crate::state::clear_active_task;
use crate::wire::DeviceEventPayload;
use crate::wire::TaskProgressPayload;
use crate::wire::WsMessage;
use crate::wire::send;
use crate::zhpro::client::ZhProClient;
use crate::zhpro::mapping::MappingSession;
use crate::zhpro::route::RouteRunner;
use crate::zhpro::teleop;
use rand::Rng;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::time::interval;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use zhpro::status::TaskMonitor;
use zhpro::teleop::SharedTeleop;
use zhpro::teleop::TeleopState;

use super::*;
use futures_util::{SinkExt, StreamExt};

/// 下一次重连退避：翻倍并封顶（1s → 2s → 4s → 8s → 10s…）
/// 一次连接「活过这么久」就认为平台侧是好的（重启 / 发布 / 抖动），
/// 下次断开的退避从 1s 重新开始，别一路飙到 30s。
pub(crate) const HEALTHY_CONNECTION: Duration = Duration::from_secs(30);

/// 业务连接重连次数：连接稳定存活过 [`HEALTHY_CONNECTION`] 就重新从 1 开始退避。
pub(crate) fn next_retry(previous: u64, elapsed: Duration) -> u64 {
    if elapsed >= HEALTHY_CONNECTION {
        1
    } else {
        previous + 1
    }
}

pub(crate) async fn run_client(
    config: &Config,
    zhpro: Option<Arc<ZhProClient>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws_url = format!("{}/ws/device", config.server_url.trim_end_matches('/'));
    info!("正在连接业务服务: {ws_url}");
    // 平台只开 https 入口时是 `wss://`：`server.tls_ca` 钉了自签证书就把连接器交下去，
    // 没配就是 tungstenite 默认（webpki 公共根，公网 CA 证书不用配）
    let (ws_stream, _) = match tls::ws_connector() {
        Some(connector) => {
            tokio_tungstenite::connect_async_tls_with_config(
                ws_url.as_str(),
                None,
                false,
                Some(connector),
            )
            .await?
        }
        None => connect_async(&ws_url).await?,
    };
    let (mut write, mut read) = ws_stream.split();
    let (out_tx, mut out_rx) = mpsc::channel::<WsMessage>(128);

    // 板端 Agent 隧道：平台/浏览器都可能不在背包网段，页面要的模型目录、运行态、
    // AI 启停与检出框都借这条设备 WS 回到平台（见 `agent_relay` 模块）
    let relay = Arc::new(AgentRelay::new(
        config.ai_agent_url.clone(),
        config.ai_enabled,
    ));
    let relay_task = relay.enabled().then(|| {
        let relay = relay.clone();
        let out = out_tx.clone();
        tokio::spawn(async move { relay.forward_events(out).await })
    });
    // Agent 可能比客户端晚起来（板子重启顺序不固定）：没登记成功就每 15s 补一次
    let announce_task = relay.enabled().then(|| {
        let relay = relay.clone();
        let out = out_tx.clone();
        tokio::spawn(async move {
            loop {
                if relay.announce(&out).await {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(15)).await;
            }
        })
    });
    if relay.enabled() {
        info!(
            "板端 Agent 隧道已启用：{}（背包页的模型目录 / 运行态 / AI 启停可跨网段代调）",
            relay.agent()
        );
    }

    // 单写循环：所有发送集中在一处，避免并发写锁
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let text = serde_json::to_string(&msg).unwrap_or_default();
            if write.send(Message::Text(text)).await.is_err() {
                break;
            }
        }
    });

    // 1) 注册：一并上报本体型号与固件版本，服务端落库后同步中台台账
    //    型号：本体 1201 `type` 优先，取不到用 `robot.model` / `--model` 兜底
    let body_model = match zhpro.as_ref() {
        Some(client) => client.session().await.device_type.trim().to_string(),
        None => String::new(),
    };
    let model = (!body_model.is_empty())
        .then_some(body_model)
        .or_else(|| config.model.clone());
    send(
        &out_tx,
        "register",
        json!({
            "deviceId": config.device_id,
            "secret": config.device_secret,
            "model": &model,
            "firmware": config.firmware,
        }),
    )
    .await;

    let state = {
        let mut initial = RobotState::new(config);
        initial.model = model;
        initial.mode = if zhpro.is_some() { "robot" } else { "sim" };
        Arc::new(Mutex::new(initial))
    };
    let active: Shared<Option<ActiveTask>> = Arc::new(Mutex::new(None));
    let monitor = Arc::new(Mutex::new(TaskMonitor::default()));
    let local_mapping: Shared<Option<Arc<MappingSession>>> = Arc::new(Mutex::new(None));
    let teleop: SharedTeleop = Arc::new(Mutex::new(TeleopState::default()));

    // 2) 本体推送任务（5009/1315/4001 等）与摇杆重发在 [`BodyLink::attach`] 里挂上，
    //    重连时用同一套句柄重新挂 —— 所以这里不再直接 spawn。

    // 3) 定时器
    let mut heartbeat_timer = interval(Duration::from_secs(config.heartbeat_interval.max(1)));
    let mut progress_timer = interval(Duration::from_secs(5));
    progress_timer.tick().await;
    let mut event_timer = interval(Duration::from_secs(120));
    // 摇杆看门狗：250ms 检查一次，超过阈值无新帧自动停车
    let mut teleop_timer = interval(Duration::from_millis(250));
    event_timer.tick().await;

    // 地图定时增量同步：只补平台没有的图，不覆盖已有记录。
    // 0 = 关闭（也可用 --no-map-sync 显式关）。
    let mut map_sync_timer = (config.map_sync_interval_secs > 0)
        .then(|| interval(Duration::from_secs(config.map_sync_interval_secs)));
    if let Some(timer) = map_sync_timer.as_mut() {
        // interval 的第一次 tick 立即就绪：先吃掉，让第一次同步在间隔之后
        timer.tick().await;
    }
    // 同一时间只跑一次：同步要拉几十兆地图包，并发会把本体文件服务压死
    let map_sync_running = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let local_runner: Shared<Option<Arc<RouteRunner>>> = Arc::new(Mutex::new(None));
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    // 航点 AI 识别：板端 Agent 与客户端同机（127.0.0.1 一定通），
    // 业务平台在公网够不到板端，所以由客户端本地代调 Agent（见 `ai_patrol` 模块）
    let ai = Arc::new(AiPatrol::with_jobs_dir(
        config.ai_agent_url.clone(),
        config.ai_enabled,
        config.ai_jobs_dir.clone(),
    ));
    if config.ai_enabled {
        info!(
            "航点 AI 识别已启用：板端 Agent = {}（到点且有「AI 识别」动作时本地开识别）",
            ai.agent()
        );
    } else {
        info!("航点 AI 识别已关闭（--no-ai / ROBODOG_AI_ENABLED=0）");
    }

    // 航点 / 航段灯光（补光灯 / LED）：与 AI 同一个板端 Agent、同一个总开关（`--no-ai`）。
    // 到点由 5009 `detail_index` 查路线动作：航点灯到点开、离开关；航段灯到点只记账、
    // 起步（离开航点）才开，走完这一段（下一个航点到点）关；
    // 夜间判定在板端。
    let lights = Arc::new(PointLights::new(
        config.ai_agent_url.clone(),
        config.ai_enabled,
    ));
    if config.ai_enabled {
        info!(
            "灯光已启用：板端 Agent = {}（计划自动开灯 lightFill/lightLed 优先，没配才按航点 / 航段动作；夜间 lightNightOnly）",
            lights.agent()
        );
    }

    // 航点 / 航段录像：起停都在 5009 状态机上（`record` 模块），这里只建运行态。
    // 与 AI / 灯光共用同一个板端 Agent 地址，但开关独立（`--no-record`）。
    let recorder = Arc::new(Recorder::new(
        Arc::new(config.clone()),
        config.record_enabled,
    ));
    if config.record_enabled {
        info!(
            "航点 / 航段录像已启用：板端 Agent = {}（到点按路线动作起 ffmpeg，录像留在板端，等平台下发「回传录像」再传）",
            recorder.agent()
        );
    } else {
        info!("航点 / 航段录像已关闭（--no-record / ROBODOG_RECORD_ENABLED=0）");
    }

    // 航点拍照：到点抓一张（`photo` 模块），任务收尾回传平台。
    // 与 AI / 录像 / 播报共用板端 Agent，但开关独立（`--no-photo`）。
    let photographer = Arc::new(Photographer::new(
        Arc::new(config.clone()),
        config.photo_enabled,
    ));
    if config.photo_enabled {
        info!(
            "航点拍照已启用：板端 Agent = {}（到点抓 JPEG，任务终态回传平台）",
            photographer.agent()
        );
        // 上次没传完的照片（断网 / 客户端重启）：连上平台之后由这里补传
        photographer.sync_pending();
    } else {
        info!("航点拍照已关闭（--no-photo / ROBODOG_PHOTO_ENABLED=0）");
    }

    // 航点 / 航段语音播报：起停同样在 5009 状态机上（`speak` 模块），
    // 与 AI / 录像共用板端 Agent，但开关独立（`--no-speak`）。
    let speaker = Arc::new(Speaker::new(
        config.ai_agent_url.clone(),
        config.speak_enabled,
    ));
    if config.speak_enabled {
        info!(
            "航点 / 航段语音播报已启用：板端 Agent = {}（航点播报：到点播、离开该点收；航段播报：起步播、到下一个航点 / 终态收）",
            speaker.agent()
        );
    } else {
        info!("航点 / 航段语音播报已关闭（--no-speak / ROBODOG_SPEAK_ENABLED=0）");
    }

    // 网页对讲中继：平台把浏览器的音频丢上来，这里转成板端 UDP（见 `phone_relay`）。
    // 与 AI / 播报共用 `[ai] agent_url`；总开关也跟 `--no-ai` 走（都靠板端 Agent）。
    let phone_relay = Arc::new(crate::phone_relay::PhoneRelay::new(
        config.ai_agent_url.clone(),
        out_tx.clone(),
    ));
    if config.ai_enabled {
        info!(
            "网页对讲已启用：板端 Agent = {}（浏览器 ⇄ 板端音频经本进程当 UDP 对端）",
            phone_relay.agent()
        );
    } else {
        info!("网页对讲已关闭（--no-ai / ROBODOG_AI_ENABLED=0）");
    }

    // 板端联网信号：客户端与 Agent 同机，信号强弱只有板子自己知道（本体 1315 没有
    // rssi）。板端 `/api/network` 已把「WiFi 优先、没连 WiFi 退 4G CSQ」统一好，
    // 这里起一个 30s 轮询，随心跳 `signal` / `signalSource` 上报，业务平台「设备管理」展示。
    // 与 AI 共用一个开关（`--no-ai` = 不要板端 Agent 代调）。
    let signal = SignalProbe::new(config.ai_agent_url.clone());
    if config.ai_enabled {
        signal.spawn();
        info!(
            "联网信号上报已启用：板端 Agent = {}（读 /api/network，WiFi 优先、未连 WiFi 退 4G）",
            signal.agent()
        );
    } else {
        info!("联网信号上报已关闭（--no-ai / ROBODOG_AI_ENABLED=0）");
    }

    // 启动时兜一把残留：`detect_jobs` 下的计划目录本该在任务结束时删（`finish_plan`），
    // 但客户端被 kill / 断电时收尾没跑到，那些目录就永远留在板子上。这里只清「早就
    // 该走了」的（`STALE_PLAN_AGE`），不碰正在跑的和刚跑完的。
    {
        let ai = ai.clone();
        tokio::spawn(async move {
            ai.sweep_stale_plans().await;
        });
    }

    // 本体链路（可变）：断连后原地重建，业务 WebSocket 不受影响
    let body_ctx = BodyContext {
        config: Arc::new(config.clone()),
        ai: ai.clone(),
        lights: lights.clone(),
        recorder: recorder.clone(),
        speaker: speaker.clone(),
        photographer: photographer.clone(),
        out_tx: out_tx.clone(),
        state: state.clone(),
        active: active.clone(),
        monitor: monitor.clone(),
        local_runner: local_runner.clone(),
        local_mapping: local_mapping.clone(),
        teleop: teleop.clone(),
    };
    let mut body = BodyLink::new(zhpro);
    if let Some(client) = body.client.clone() {
        body.attach(&body_ctx, client);
    }
    // 手动「本体重连」指令 → 主循环（与自动重连同一条路径）
    let (body_reconnect_tx, mut body_reconnect_rx) = mpsc::channel::<()>(1);
    // 后台重连的结果回传：连上（新 client）或窗口内失败
    let (relink_tx, mut relink_rx) = mpsc::channel::<Result<Arc<ZhProClient>, String>>(1);
    let mut relinking = false;

    // 进度上报：内容没变就降到 [`PROGRESS_DEDUP_WINDOW`] 一次（服务端还会按「标题 + 描述」
    // 去重落事件）。**不能彻底停发**：服务端任务看门狗按 `tasks.updated_at` 判超时
    // （默认 120s），超时会直接把跑着的任务翻成「异常」。反过来，进度一直不动也会
    // 被「进度停滞」判据（`task_stuck_secs`）判异常，所以卡住时上报真实原因比刷
    // 「执行中」更有用。
    let mut progress_reporter = ProgressReporter::default();

    // 连接断开的原因：`None` = 主动退出（Ctrl-C）；`Some` = 平台侧断开 / 网络抖动，
    // 交给 `main` 的重连循环**原地重连**，不再让进程退出等 systemd 拉起
    // （以前一次平台重启就是「进程退出 → 5s 后 systemd 重启 → 再握手」，设备白掉线 10s+）。
    let mut close_reason: Option<String> = None;
    loop {
        // 每次迭代拷一份链路监视器：`wait_body_down` 只借用这个临时副本，
        // 分支体里才能安全地重建 `body`
        let mut probe = body.down.clone();
        tokio::select! {
            // 本体 control/bulkdata 断了：原地重连，不退出进程
            _ = wait_body_down(&mut probe) => {
                if relinking {
                    // 已在重连：结果由 `relink_rx` 分支处理，这里不能空转
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
                warn!("本体 control/bulkdata 通道已断开，开始原地重连（不退出进程）");
                start_relink(
                    &body_ctx,
                    &mut body,
                    &mut relinking,
                    &relink_tx,
                    "body-link-lost",
                )
                .await;
            }

            // 控制台「本体重连」：同一条重建路径，也不退出进程
            _ = body_reconnect_rx.recv() => {
                if relinking {
                    warn!("已在重连中，忽略重复的「本体重连」");
                    continue;
                }
                warn!("收到「本体重连」，开始原地重建本体链路");
                start_relink(&body_ctx, &mut body, &mut relinking, &relink_tx, "manual").await;
            }

            // 后台重连有结果了（心跳/指令这段时间一直照常跑）
            Some(result) = relink_rx.recv() => {
                relinking = false;
                match result {
                    Ok(client) => finish_relink(&body_ctx, &mut body, client).await,
                    Err(error) => {
                        error!("{error}；进程退出，等待 systemd 重启重建本体链路");
                        std::process::exit(1);
                    }
                }
            }

            _ = &mut ctrl_c => {
                info!("收到中断信号，退出");
                break;
            }

            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) else {
                            warn!("无法解析业务消息: {text}");
                            continue;
                        };
                        handle_server_message(
                            config,
                            ws_msg,
                            &out_tx,
                            &state,
                            &active,
                            &body.client,
                            &local_runner,
                            &local_mapping,
                            &teleop,
                            &body_reconnect_tx,
                            &ai,
                            &lights,
                            &recorder,
                            &speaker,
                            &photographer,
                            &relay,
                            &phone_relay,
                        ).await;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // 交给写循环回复 pong
                        let _ = data;
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("业务服务关闭了连接，准备原地重连");
                        close_reason = Some("业务服务关闭了连接".to_string());
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        error!("WebSocket 错误: {e}，准备原地重连");
                        close_reason = Some(e.to_string());
                        break;
                    }
                    None => {
                        info!("业务连接已断开，准备原地重连");
                        close_reason = Some("业务连接已断开".to_string());
                        break;
                    }
                }
            }

            _ = heartbeat_timer.tick() => {
                let running = active.lock().await.is_some();
                let mut guard = state.lock().await;
                // 本体没在报状态（模拟模式 / 本体掉线）才走模拟推演
                if !guard.body_status_fresh() {
                    guard.tick(running);
                }
                let mut hb = guard.heartbeat();
                drop(guard);
                // 信号来自背包板子（`signal` 探针）：读不到就不带这两个字段，页面显示 `—`
                let network = signal.signal();
                hb.signal = network.map(|item| item.value);
                hb.signal_source = network.map(|item| item.source.as_str().to_string());
                send(&out_tx, "heartbeat", serde_json::to_value(hb).unwrap_or_default()).await;
            }

            // 执行中定期回传进度（5009 推送是 500ms，这里合流成业务侧 5s 一次）
            _ = progress_timer.tick() => {
                let snapshot = active.lock().await.clone();
                let Some(task) = snapshot else { continue };
                let runner = local_runner.lock().await.clone();
                let mapping = local_mapping.lock().await.clone();
                if runner.is_none() && mapping.is_none() {
                    // 返充 / 出桩这类本体任务没有本地 runner：进度由 5009 快照驱动，
                    // 这里不重复上报，更不能拿「已暂停」去覆盖真实阶段。
                    if task.nav_task_id.is_some() {
                        continue;
                    }
                    // 既没有 runner 也没有本体任务 = 根本没有执行上下文（下发失败 / 客户端重启过），
                    // 继续上报只会每 5s 刷一条假「已暂停」，直接清掉本地占位。
                    warn!("任务 {} 本地没有执行上下文，停止进度上报", task.task_code);
                    clear_active_task(&active, &state).await;
                    continue;
                }
                // 航线任务的导航任务 ID 只在 RouteRunner 里，必须从这里取：
                // 服务端靠 `robot_task_id` 判断「暂停后能不能 resume」，缺了会被
                // 当成从未下发的任务重跑整条航线。
                let runner_nav_task = match &runner {
                    Some(runner) => runner.current_nav_task().await,
                    None => None,
                };
                let mut payload = TaskProgressPayload {
                    task_code: task.task_code.clone(),
                    status: "执行中".to_string(),
                    progress: 0,
                    point_index: None,
                    waypoint_index: task.point_index,
                    message: None,
                    stage: task.stage.clone(),
                    robot_task_id: runner_nav_task.or_else(|| task.nav_task_id.clone()),
                    total_points: None,
                };
                // 智航导航进度按航点折算；模拟模式按时间推演
                if let Some(runner) = runner {
                    let (done, total) = runner.progress().await;
                    payload.progress = progress_pct(done, total);
                    // `pointIndex` 是「已完成航点数」（1 基，前端显示「进度 n/N」）；
                    // 平台拿它查路线航点会查到下一个点，所以「站在哪个点」另有
                    // `waypointIndex`（上面已按 5009 的 detail_index 填好）。
                    payload.point_index = Some(done as i64);
                    payload.total_points = Some(total as i64);
                    payload.message = Some(format!("已完成 {done}/{total} 个航点"));
                    // 暂停时不能还报「执行中」，否则平台上的「已暂停」会被刷回去
                    if runner.is_paused() {
                        payload.status = "已暂停".to_string();
                        payload.message = Some("任务已暂停".to_string());
                    }
                }
                // 内容未变时降频，不再往事件流里刷重复进度（以前 5s 一条，暂停一小时几千条）；
                // 每次真正发出去都会把整份 payload 打到日志，方便对着平台上的读数排查
                progress_reporter.report(&out_tx, &payload, "定时").await;
            }

            _ = teleop_timer.tick() => {
                if let Some(ref client) = body.client {
                    teleop::tick_watchdog(client, &teleop).await;
                }
            }

            // 地图定时增量同步：只在完全空闲时跑（无任务 / 无建图 / 非重连中 /
            // 本体状态新鲜），并且抛到后台任务 —— 下载整包动辄几十秒，
            // 卡在主循环里会让心跳与 1315 处理停摆，反而把设备刷成离线。
            _ = async {
                match map_sync_timer.as_mut() {
                    Some(timer) => {
                        timer.tick().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            } => {
                if relinking {
                    debug!("本体重连中，跳过本轮地图增量同步");
                    continue;
                }
                let Some(client) = body.client.clone() else {
                    continue;
                };
                if active.lock().await.is_some() || local_mapping.lock().await.is_some() {
                    debug!("设备正在执行任务/建图，跳过本轮地图增量同步");
                    continue;
                }
                if !state.lock().await.body_status_fresh() {
                    debug!("本体状态不新鲜，跳过本轮地图增量同步");
                    continue;
                }
                let running = map_sync_running.clone();
                if running.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    debug!("上一轮地图同步还没结束，跳过本轮");
                    continue;
                }
                // 本轮结束（含 panic）一定复位，否则一次意外就永久停掉轮询
                let guard = MapSyncRunning(running.clone());
                let cfg = body_ctx.config.clone();
                let out = out_tx.clone();
                tokio::spawn(async move {
                    let _guard = guard;
                    let result = sync_maps_from_robot(
                        &cfg,
                        &client,
                        &out,
                        None,
                        // 轮询永远只补缺，绝不覆盖平台已有地图
                        false,
                        // 没变化就不发事件，有新增/失败才发
                        false,
                        // 后台轮询不上报进度（没有中台 requestId）
                        None,
                    )
                    .await;
                    match result {
                        Ok(done) if done.report.failed > 0 => {
                            warn!("定时地图同步有失败项: {}", done.summary)
                        }
                        Ok(done) => info!("定时地图同步结束: {}", done.summary),
                        Err(error) => warn!("定时地图同步失败: {error}"),
                    }
                });
            }

            _ = event_timer.tick() => {
                let guard = state.lock().await;
                if guard.body_status_fresh() && rand::thread_rng().gen_bool(0.3) {
                    let event = DeviceEventPayload {
                        event_type: "info".to_string(),
                        message: format!("机体温度 {:.1}℃", guard.temperature),
                        data: Some(json!({ "temperature": guard.temperature })),
                    };
                    drop(guard);
                    send(&out_tx, "event", serde_json::to_value(event).unwrap_or_default()).await;
                }
            }
        }
    }

    writer.abort();
    if let Some(task) = relay_task {
        task.abort();
    }
    if let Some(task) = announce_task {
        task.abort();
    }
    match close_reason {
        Some(reason) => Err(reason.into()),
        None => Ok(()),
    }
}
