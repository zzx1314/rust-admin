//! 智航 Pro 本体链路：建链、断线原地重连、链路句柄与一次性建图。

use crate::ai_patrol::AiPatrol;
use crate::lights::PointLights;
use crate::maps::upload_mapping_to_zentai;
use crate::mapupload::UploadOutcome;
use crate::photo::Photographer;
use crate::pump::spawn_zhpro_pump;
use crate::record::Recorder;
use crate::report::report_progress;
use crate::speak::Speaker;
use crate::state::ActiveTask;
use crate::state::RobotState;
use crate::state::Shared;
use crate::wire::DeviceEventPayload;
use crate::wire::WsMessage;
use crate::wire::send;
use crate::zhpro::client::ZhProClient;
use crate::zhpro::mapping::MappingSession;
use crate::zhpro::route::RouteRunner;
use crate::zhpro::teleop;
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use zhpro::mapping::MapManager;
use zhpro::status::StatusFrame;
use zhpro::status::TaskMonitor;
use zhpro::teleop::SharedTeleop;

use super::*;
use zhpro::protocol::*;

/// 本体断连后的**进程内**重连参数
///
/// 现场「本体重启」要 40~60s 才重新监听 10010，所以窗口给到 2 分钟；超过窗口
/// 就不再自己试，退出进程交给 systemd（`Restart=always`）作最后兜底。
pub(crate) const BODY_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) const BODY_RECONNECT_WINDOW: Duration = Duration::from_secs(120);

pub(crate) const BODY_RECONNECT_MIN_BACKOFF: Duration = Duration::from_secs(1);

pub(crate) const BODY_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(10);

/// 链路「刚挂上就被踢」的抖动保护窗口
///
/// 重连成功但与本体只活了不到这个时间（网关还留着旧会话、重复 client 竞争）时，
/// 下一轮重连先等够窗口再动手，避免无间隔连打网关。
pub(crate) const BODY_RECONNECT_SETTLE: Duration = Duration::from_secs(5);

pub(crate) fn next_body_backoff(current: Duration) -> Duration {
    (current * 2).min(BODY_RECONNECT_MAX_BACKOFF)
}

/// 连接智航本体。
///
/// - `robot.host` 为空（`--robot sim`）：显式模拟模式，返回 `None`；
/// - 配了本体：最多重试 `BODY_ATTEMPTS` 次（现场开机竞态：客户端比本体网络先起来），
///   仍失败则返回错误让进程退出 —— systemd `Restart=always` 会带退避重启，
///   绝不带着「模拟数据」冒充真机上报。
pub(crate) async fn connect_body(
    config: &Config,
) -> Result<Option<Arc<ZhProClient>>, Box<dyn std::error::Error + Send + Sync>> {
    const BODY_ATTEMPTS: u32 = 12;
    const BODY_RETRY_SECS: u64 = 5;

    let Some(raw_host) = config.robot.as_deref() else {
        warn!(
            "未接入本体（robot.host 为空或为 sim），以模拟模式运行（任务只回传进度，不驱动本体）"
        );
        return Ok(None);
    };

    // 联调用假本体：进程内起一个 mock 网关，再把地址换成它。
    // 后面走的仍是真机的双通道协议，所以 5000/5001/5009 全部是真实报文。
    let host = if mock_body::is_mock_alias(raw_host) {
        match mock_body::start_from_env().await {
            Ok(addr) => {
                info!("已启动模拟本体（mock）：{addr}（联调模式，非真机）");
                addr
            }
            Err(e) => return Err(format!("启动模拟本体失败: {e}").into()),
        }
    } else {
        raw_host.to_string()
    };
    let host = host.as_str();

    for attempt in 1..=BODY_ATTEMPTS {
        match ZhProClient::connect(host).await {
            Ok(client) => {
                let session = client.session().await;
                info!(
                    "本体连接就绪: client_id={}, 型号={}",
                    session.client_id, session.device_type
                );
                return Ok(Some(Arc::new(client)));
            }
            Err(error) if attempt == BODY_ATTEMPTS => {
                return Err(format!(
                    "连接智航本体 {host} 失败（已重试 {BODY_ATTEMPTS} 次，每次间隔 {BODY_RETRY_SECS}s）: {error}"
                )
                .into());
            }
            Err(error) => {
                warn!(
                    "连接智航本体失败（第 {attempt}/{BODY_ATTEMPTS} 次）: {error}，{BODY_RETRY_SECS}s 后重试"
                );
                tokio::time::sleep(Duration::from_secs(BODY_RETRY_SECS)).await;
            }
        }
    }
    unreachable!("循环必然在最后一次尝试时返回")
}

/// 只跑一次建图（无需业务服务），用于地图标定与联调
pub(crate) async fn run_mapping_once(config: &Config, zhpro: Arc<ZhProClient>) {
    let session = MappingSession::new(zhpro.clone());
    let kind = match config.map_kind.as_str() {
        "indoor" => zhpro::protocol::MappingType::Indoor,
        _ => zhpro::protocol::MappingType::Outdoor,
    };
    match session.start(&config.map_name, kind).await {
        Ok(task_id) => info!("建图已开始: task_id={task_id}，走完后按 Ctrl+C 停止并保存"),
        Err(e) => {
            error!("建图任务创建失败: {e}");
            return;
        }
    }

    let mut rx = zhpro.subscribe();
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        tokio::select! {
            _ = &mut ctrl_c => {
                info!("收到中断信号，停止建图并保存");
                match session.stop(Duration::from_secs(30)).await {
                    Some(saved) if saved.success => {
                        info!("地图保存成功: {}", saved.message);
                        if let Ok(maps) = MapManager::new(zhpro.clone()).list().await {
                            for map in maps {
                                info!(
                                    "地图: {} 路网={} 回充点={}",
                                    map.map_id, map.route_exist, map.charge_point_exist
                                );
                            }
                        }
                        // 手工建图同样支持回传中台
                        match upload_mapping_to_zentai(config, &saved).await {
                            UploadOutcome::Skipped(reason) => {
                                info!("未回传中台: {reason}");
                            }
                            UploadOutcome::Uploaded { map_id } => {
                                info!("已入库中台: {map_id}");
                            }
                            UploadOutcome::Failed { reason } => {
                                error!("回传中台失败: {reason}");
                            }
                        }
                    }
                    Some(saved) => error!("地图保存失败: code={} {}", saved.code, saved.message),
                    None => error!("未收到 4001 保存结果"),
                }
                return;
            }
            env = rx.recv() => {
                let Ok(env) = env else { continue };
                match env.head.msg_type {
                    T_MAPPING_TELEMETRY => {
                        session.on_telemetry(&env.data).await;
                        let tel = session.telemetry().await;
                        if let Some(pose) = tel.pose {
                            info!(
                                "建图位姿 ({:.2}, {:.2}) 里程 {:.1}m 剩余 {:.1}m",
                                pose.position.x, pose.position.y, tel.mileage, tel.remain_mileage
                            );
                        }
                    }
                    T_MAP_SAVED => {
                        if let Some(saved) = session.on_map_saved(&env.data).await {
                            info!("建图收尾: success={} {}", saved.success, saved.message);
                            return;
                        }
                    }
                    T_TASK_STATUS => {
                        if let Some(frame) = StatusFrame::parse(&env.data)
                            && let Some(task) = &frame.current
                            && task.task_type == "mapping"
                        {
                            info!("建图阶段: {} ({})", task.stage, task.lifecycle);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// 本体链路重建时需要重新挂到新任务上的共享句柄（都可 `Clone`）
pub(crate) struct BodyContext {
    pub(crate) config: Arc<Config>,
    pub(crate) ai: Arc<AiPatrol>,
    pub(crate) lights: Arc<PointLights>,
    pub(crate) recorder: Arc<Recorder>,
    pub(crate) speaker: Arc<Speaker>,
    pub(crate) photographer: Arc<Photographer>,
    pub(crate) out_tx: mpsc::Sender<WsMessage>,
    pub(crate) state: Shared<RobotState>,
    pub(crate) active: Shared<Option<ActiveTask>>,
    pub(crate) monitor: Arc<Mutex<TaskMonitor>>,
    pub(crate) local_runner: Shared<Option<Arc<RouteRunner>>>,
    pub(crate) local_mapping: Shared<Option<Arc<MappingSession>>>,
    pub(crate) teleop: SharedTeleop,
}

/// 本体链路的运行期句柄。
///
/// `control` / `bulkdata` 断开后**不退出进程**：在这里原地重建连接，而业务
/// WebSocket（注册、心跳、任务、事件）全程不断 —— 设备不会因为一次网线抖动
/// 就从平台上掉线重登记，恢复窗口从「systemd 重启 + 重新握手」的 5~15s 缩到 1~3s。
pub(crate) struct BodyLink {
    pub(crate) client: Option<Arc<ZhProClient>>,
    pub(crate) pump: Option<tokio::task::JoinHandle<()>>,
    pub(crate) republisher: Option<tokio::task::JoinHandle<()>>,
    /// 链路断开信号（任一通道读到 EOF 或写失败即置 `true`）
    pub(crate) down: Option<tokio::sync::watch::Receiver<bool>>,
    /// 本次链路挂上的时刻，用于识别「刚挂上就被踢」的抖动
    pub(crate) attached_at: Option<Instant>,
}

impl BodyLink {
    pub(crate) fn new(client: Option<Arc<ZhProClient>>) -> Self {
        Self {
            client,
            pump: None,
            republisher: None,
            down: None,
            attached_at: None,
        }
    }

    /// 挂上推送任务与链路监视（首次连接与每次重连各调一次）
    pub(crate) fn attach(&mut self, ctx: &BodyContext, client: Arc<ZhProClient>) {
        self.down = Some(client.body_link());
        self.pump = Some(spawn_zhpro_pump(
            client.clone(),
            ctx.out_tx.clone(),
            ctx.state.clone(),
            ctx.active.clone(),
            ctx.monitor.clone(),
            ctx.local_mapping.clone(),
            ctx.teleop.clone(),
            ctx.config.clone(),
            ctx.ai.clone(),
            ctx.lights.clone(),
            ctx.recorder.clone(),
            ctx.speaker.clone(),
            ctx.photographer.clone(),
        ));
        // 摇杆重发（50Hz）：浏览器 ~10Hz 的离散帧需要在本体侧连续化，否则看着像点动
        self.republisher = Some(teleop::spawn_republisher(
            client.clone(),
            ctx.teleop.clone(),
        ));
        self.attached_at = Some(Instant::now());
        self.client = Some(client);
    }

    /// 摘掉当前链路，返回被摘下的 client（模拟模式 / 已摘下时为 `None`）
    ///
    /// 两个后台任务必须显式 `abort`：它们各自持有旧 `Arc<ZhProClient>`，只换字段
    /// 不杀任务会留下继续往 `out_tx` 塞过期帧的僵尸读循环。
    pub(crate) fn detach(&mut self) -> Option<Arc<ZhProClient>> {
        if let Some(handle) = self.pump.take() {
            handle.abort();
        }
        if let Some(handle) = self.republisher.take() {
            handle.abort();
        }
        self.down = None;
        self.attached_at = None;
        self.client.take()
    }
}

/// 重连前的一次性清理，返回「抖动保护」需要先等的时长
///
/// 必须**快**：它跑在主 select 的关键路径上，期间心跳会暂停 —— 所以耗时的
/// 「建链 + 退避等待」全部交给后台任务 [`connect_body_with_backoff`]，
/// 由主循环在 select 里等结果。这样重连 2 分钟也不会把设备心跳打断
/// （否则服务端 `device_timeout_secs` 会先把设备判离线）。
pub(crate) async fn prepare_body_reconnect(
    ctx: &BodyContext,
    body: &mut BodyLink,
    trigger: &str,
) -> Duration {
    // 上一轮链路刚挂上就被踢（网关还留着旧会话 / 重复 client 竞争）：先等够窗口
    let settle = body
        .attached_at
        .map(|at| BODY_RECONNECT_SETTLE.saturating_sub(at.elapsed()))
        .unwrap_or_default();
    if !settle.is_zero() {
        warn!("本体链路刚建立就断开，{settle:?} 后再重连（抖动保护）");
    }

    let previous = body.detach();

    // 1) 接管态必然失效：`1204` 是会话级的，重建 control 通道就没了
    //    先补一帧停车，避免最后一帧非零摇杆留在本体侧
    let was_engaged = teleop::is_engaged(&ctx.teleop).await;
    match previous.as_ref() {
        Some(client) => teleop::reset_after_control_lost(client, &ctx.teleop).await,
        None => teleop::reset_local_only(&ctx.teleop).await,
    }
    // 2) 业务 WS 没断，服务端收不到断连回调：必须自己上报 holder 已清空，
    //    否则留下幽灵接管（页面显示接管中、摇杆被客户端门禁拒绝）。
    //    服务端只在 holder 非空时才动手，所以这里无条件发一帧是安全的。
    send(
        &ctx.out_tx,
        "teleop_state",
        json!({
            "holder": false,
            "reason": trigger,
            "wasEngaged": was_engaged,
        }),
    )
    .await;

    // 3) 运行态清理
    //
    // 断链时本体**可能还在跑那条任务**（任务在 `.1` 的 `dog_task` 里，我们断链不会让它停）。
    // 这里把它的本体任务号记进登记簿，`finish_relink` 拿到新链路后补发 `5002`；
    // 不记的话就是「平台已判异常、狗还在跑」（2026-11-27 `TK-1127-22`：17:14:03
    // 遥控器抢权断链，17:18:46 平台判超时，狗 17:22:23 还在跑）。
    // 巡检的 `nav_task_id` 只在 runner 里有（`bind_nav_task` 也会写进 active），
    // 两边都取一次，不靠单一路径。
    let runner_nav_task = match ctx.local_runner.lock().await.clone() {
        Some(runner) => runner.current_nav_task().await,
        None => None,
    };
    if let Some(task) = ctx.active.lock().await.clone() {
        if let Some(nav_task_id) = task.nav_task_id.as_deref() {
            crate::orphan::remember(nav_task_id).await;
        }
        report_progress(
            &ctx.out_tx,
            &task.task_code,
            "异常",
            0,
            Some(format!(
                "本体连接已断开（{trigger}）——客户端正在原地重连本体（不退出进程），请稍后重新下发任务"
            )),
            None,
            None,
            None,
        )
        .await;
    }
    if let Some(nav_task_id) = runner_nav_task {
        crate::orphan::remember(&nav_task_id).await;
    }
    *ctx.active.lock().await = None;
    *ctx.local_runner.lock().await = None;
    if ctx.local_mapping.lock().await.take().is_some() {
        warn!("建图会话随本体断连一起废弃（4001 保存结果不会再来）");
    }
    {
        let mut guard = ctx.state.lock().await;
        guard.current_task = None;
        // 门禁立刻回到「设备初始化中」：断连前的 1315 值不可再用于放行运动指令
        guard.body_status_at = None;
        guard.ctrl_source = None;
        guard.sdk_node_available = None;
        guard.estop_software = None;
        guard.motion_status = None;
    }

    settle
}

/// 后台退避重连：连上就返回新 client，窗口内连不上返回 `Err`
///
/// 与主循环解耦，所以期间心跳、业务指令、事件上报都照常走。
pub(crate) async fn connect_body_with_backoff(
    host: String,
    settle: Duration,
) -> Result<Arc<ZhProClient>, String> {
    if !settle.is_zero() {
        tokio::time::sleep(settle).await;
    }
    let started = Instant::now();
    let mut backoff = BODY_RECONNECT_MIN_BACKOFF;
    let mut attempt = 0_u32;
    let mut last_error = String::new();

    while started.elapsed() < BODY_RECONNECT_WINDOW {
        attempt += 1;
        match tokio::time::timeout(BODY_CONNECT_TIMEOUT, ZhProClient::connect(&host)).await {
            Ok(Ok(client)) => {
                info!(
                    "本体链路已重建（第 {attempt} 次尝试，耗时 {:?}）",
                    started.elapsed()
                );
                return Ok(Arc::new(client));
            }
            Ok(Err(error)) => last_error = error.to_string(),
            Err(_) => {
                last_error = format!("连接超时（>{}s）", BODY_CONNECT_TIMEOUT.as_secs());
            }
        }
        warn!(
            "本体重连失败（第 {attempt} 次）: {last_error}，{}s 后重试",
            backoff.as_secs()
        );
        tokio::time::sleep(backoff).await;
        backoff = next_body_backoff(backoff);
    }

    Err(format!(
        "本体在 {}s 内重连失败（{attempt} 次尝试）：{last_error}",
        BODY_RECONNECT_WINDOW.as_secs()
    ))
}

/// 启动后台重连：先摘链路 + 清运行态（快），再把耗时的建链丢给后台任务
pub(crate) async fn start_relink(
    ctx: &BodyContext,
    body: &mut BodyLink,
    relinking: &mut bool,
    relink_tx: &mpsc::Sender<Result<Arc<ZhProClient>, String>>,
    trigger: &str,
) {
    let settle = prepare_body_reconnect(ctx, body, trigger).await;
    let Some(host) = ctx.config.robot.clone() else {
        // 没有本体配置却收到断连信号（理论上不可达）：只能退回「退出交给 systemd」
        error!("未接入本体（robot.host 为空），无法原地重连");
        std::process::exit(1);
    };
    *relinking = true;
    let tx = relink_tx.clone();
    tokio::spawn(async move {
        let result = connect_body_with_backoff(host, settle).await;
        // 接收端已关闭（进程正在退出）时忽略
        let _ = tx.send(result).await;
    });
}

/// 重连成功：挂上推送任务，并把新会话的型号 / 工作模式同步给平台
pub(crate) async fn finish_relink(
    ctx: &BodyContext,
    body: &mut BodyLink,
    client: Arc<ZhProClient>,
) {
    let session = client.session().await;
    {
        // 新会话可能报回不同型号（换本体 / 刷固件）：心跳会把变化同步给中台台账
        let mut guard = ctx.state.lock().await;
        if !session.device_type.trim().is_empty() {
            guard.model = Some(session.device_type.clone());
        }
        guard.mode = "robot";
    }
    body.attach(ctx, client.clone());
    // 断链前没送出去的 5002 在这里补：本体一直在跑那条任务，而它已经不归我们跟踪了。
    // 放到后台跑，别把主循环（心跳 / 指令）堵在 5002 + 等 idle 上。
    if crate::orphan::len().await > 0 {
        let cancel_client = client.clone();
        tokio::spawn(async move {
            zhpro::route::flush_pending_cancels(&cancel_client, zhpro::route::IDLE_WAIT).await;
        });
    }
    info!(
        "本体链路已重建：client_id={}，型号={}（业务连接未中断）",
        session.client_id, session.device_type
    );
    let event = DeviceEventPayload {
        event_type: "info".to_string(),
        message: format!(
            "本体链路已重建（client_id={}），设备在线状态未受影响，需要重新「远程接管」才能遥控",
            session.client_id
        ),
        data: None,
    };
    send(
        &ctx.out_tx,
        "event",
        serde_json::to_value(event).unwrap_or_default(),
    )
    .await;
}

/// 等本体链路断开（永不返回 = 没有本体连接 / 尚未断开）
///
/// `watch::Receiver::changed()` 是取消安全的，可以反复放进 `select!` 里轮询。
pub(crate) async fn wait_body_down(link: &mut Option<tokio::sync::watch::Receiver<bool>>) {
    let Some(rx) = link.as_mut() else {
        std::future::pending::<()>().await;
        return;
    };
    if *rx.borrow_and_update() {
        return;
    }
    loop {
        if rx.changed().await.is_err() {
            // 发送端被丢弃：视为链路已不可用
            return;
        }
        if *rx.borrow_and_update() {
            return;
        }
    }
}

#[cfg(test)]
mod body_reconnect_tests {
    use super::*;

    /// 业务连接重连计数：跑得久的连接断了 → 退避归 1；刚连上就断 → 继续累加
    #[test]
    fn retry_resets_after_a_healthy_connection() {
        use crate::session::{HEALTHY_CONNECTION, next_retry};
        use std::time::Duration;
        assert_eq!(next_retry(0, Duration::from_secs(1)), 1);
        assert_eq!(next_retry(5, Duration::from_secs(2)), 6);
        assert_eq!(next_retry(9, HEALTHY_CONNECTION), 1);
        assert_eq!(next_retry(9, Duration::from_secs(600)), 1);
    }

    /// 退避序列必须是「翻倍 + 封顶」，否则重连会退化成忙等
    #[test]
    fn backoff_doubles_then_caps() {
        let mut backoff = BODY_RECONNECT_MIN_BACKOFF;
        let mut seen = vec![backoff];
        for _ in 0..6 {
            backoff = next_body_backoff(backoff);
            seen.push(backoff);
        }
        assert_eq!(
            seen,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                BODY_RECONNECT_MAX_BACKOFF,
                BODY_RECONNECT_MAX_BACKOFF,
                BODY_RECONNECT_MAX_BACKOFF,
            ]
        );
    }

    /// 120s 窗口按「已等退避 + 单次连接超时」估算能试几次：
    /// 至少要留出 3 次机会，否则本体重启（40~60s 不可用）撐不过来
    #[test]
    fn window_allows_at_least_three_attempts() {
        let mut elapsed = Duration::ZERO;
        let mut backoff = BODY_RECONNECT_MIN_BACKOFF;
        let mut attempts = 0;
        while elapsed < BODY_RECONNECT_WINDOW {
            attempts += 1;
            elapsed += BODY_CONNECT_TIMEOUT.min(Duration::from_secs(5));
            if elapsed >= BODY_RECONNECT_WINDOW {
                break;
            }
            elapsed += backoff;
            backoff = next_body_backoff(backoff);
        }
        assert!(attempts >= 3, "窗口内只够 {attempts} 次尝试");
    }
}
