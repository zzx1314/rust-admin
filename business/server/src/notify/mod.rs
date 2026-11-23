//! 企业微信群机器人推送：告警 / 任务异常 → 群消息。
//!
//! # 为什么单独一个模块
//!
//! 告警写入点散在 5 处（心跳看门狗、设备事件、控制权抢占、AI 识别、手动新建），
//! 以前每处自己拼 SQL、自己生成 `alert_code`。推送如果跟着各写一遍，很快就没人
//! 说得清「哪条告警会进群」。这里只提供一个出口：
//!
//! - 告警落库后调 [`alert_created`]，用 `alert_code` 回查设备/园区，统一格式化；
//! - 任务被判异常时调 [`task_abnormal`]。
//!
//! # 硬约束（来自企业微信，不是选择）
//!
//! - **webhook 地址本身就是全部凭据**，没有加签。它**优先**从平台设置页
//!   （`settings.wecom_webhook`）读，未配置时回退环境变量 `ROBODOG_WECOM_WEBHOOK`；
//!   环境变量/配置文件仍然是部署级入口。接口只回显掩码，日志只打 key 末几位。
//!   平台配置改完**立即生效**（句柄在 `Notifier` 里，不在启动时固化）。
//! - **20 条/分钟**：超了返回 `errcode 45009`。必须本地限流。
//! - **markdown ≤ 4096 字节**（中文 3 字节/字）：超长整条被拒，不是截断。
//! - **markdown 不支持 @提及**：要 @所有人得另发一条 `text`，用 `mentioned_list`。
//! - **markdown 也不支持显示图片**（官方语法子集里没有 `![]()`）：要发证据图只能走
//!   `msgtype=image`，带**原图字节的 base64 + 原图 md5**，原图 ≤ 2MB、仅 JPG/PNG。
//!   见 [`NoticeImage`] / [`evidence_image`]。
//! - **只能发不能收**：消息里带平台链接，处理动作仍在平台内完成。
//!
//! # 不能阻塞调用方
//!
//! 所有 `enqueue` 都是同步的 `try_send`，队列满就丢并计数（每 60s 汇总一条
//! 「另有 N 条未逐条推送」）。绝不能在 WS handler / 看门狗里 await 外网 HTTP：
//! `qyapi.weixin.qq.com` 一次超时 10s，会把心跳处理和看门狗 tick 全拖住。

use crate::config::Config;
use crate::gateway::DeviceConnections;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

mod deliver;
mod model;
mod policy;
#[cfg(test)]
mod tests;
mod throttle;
mod webhook;

// 对外 API（`crate::notify::*`）：只列真正要给别的模块/测试用的东西。
pub use model::{
    EvidenceCleanup, Level, Notice, NoticeImage, evidence_cleanup, evidence_image, truncate_bytes,
};
pub use policy::{
    CountMatched, alert_created, alert_created_with_image, alert_evidence, count_matched,
    send_test, task_abnormal,
};
pub use webhook::{WebhookSource, key_tail, mask_webhook, stored_webhook, validate_webhook};

// crate 内共享：模块之间互相调用用，不对外。
//
// 这里刻意用 glob：`deliver` 里的东西是一整套发送管线，逐个列名只会在每次加函数时
// 漏一个编译错误（`pub(crate)` 已经不对外泄露了）。
#[allow(clippy::wildcard_imports)]
pub(crate) use deliver::*;
pub(crate) use model::{Channel, without_evidence_path};
pub(crate) use policy::alerts_link;
pub(crate) use throttle::{Dedupe, RateGate};
pub(crate) use webhook::WebhookTarget;

/// 推送层需要的最小上下文。
///
/// 以前这里每个 `pub async fn` 都收 `&AppState`，等于让「传输层」反向依赖整个应用
/// （视频房间、指令回执、调度……），于是 `app -> notify -> app` 成了一个环：
/// 想给推送写单测、或者想在别的进程里复用推送通道，都必须先造一个完整 `AppState`。
///
/// 现在只收推送真正用得上的四样东西，由 `AppState::notify()` 装配：
///
/// - `db`：回查告警 / 工单 / 任务 / 设置（文案与「该不该推」都在库里）；
/// - `config`：只读 `public_url`（消息里的「查看详情」）与 `wecom_enabled`；
/// - `notifier`：入队句柄（webhook 可热更新、队列满丢包计数都在它里面）；
/// - `device_connections`：证据图进群后要沿**设备 WS** 下发 `ai_cleanup`。
#[derive(Clone)]
pub struct NotifyCtx {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    pub notifier: Notifier,
    pub device_connections: DeviceConnections,
}
/// markdown content 上限（**字节**）。留 96 字节余量给可能的包装。
pub const MARKDOWN_LIMIT: usize = 4000;
/// text content 上限（字节）。
pub const TEXT_LIMIT: usize = 2000;
/// 本地限流：比企业微信的 20 条/分钟留 2 条余量给「测试消息」和重试。
pub const RATE_PER_MIN: u32 = 18;
/// 队列深度。看门狗 15s 一轮、设备事件突发，256 足够。
const QUEUE_CAPACITY: usize = 256;

/// 图片消息的体积上限（**原图字节**，base64 编码前）。企业微信硬限制 2MB。
pub const IMAGE_MAX_BYTES: usize = 2 * 1024 * 1024;
/// 平台设置页保存 webhook 的 settings 键（`value_type = 'secret'`）。
///
/// 刻意**不出现在** `GET /api/settings` 的列表里：地址即凭据，通用设置接口
/// 一律只回显掩码，真正的读写走 `/api/notifications/webhook`。
pub const WEBHOOK_SETTING_KEY: &str = "wecom_webhook";

/// 同一 `dedupe_key` 的静默窗口。
///
/// 心跳看门狗 15s 扫一轮、客户端 5s 报一次进度，同一件事会被"发现"很多次。
const DEDUPE_WINDOW: Duration = Duration::from_secs(300);

/// AI 命中（`ai:` 结构化键）的推送去重窗口：**1 分钟**。
///
/// 一轮巡检就是一次结论，跨轮次（几分钟一轮、取消后重跑）必须各推一条；
/// 但窗口抖动 / 客户端重开识别时，同一航点同一类别可能几秒内重复上报，
/// 那几条要合并。所以按「结构化键 + 60s」，而不是跟设备故障一样按工单 5 分钟。
const AI_HIT_DEDUPE_WINDOW: Duration = Duration::from_secs(60);
struct Shared {
    dedupe: Mutex<Dedupe>,
    /// 队列满被丢掉的条数，60s 汇总一次。
    dropped: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Self {
            dedupe: Mutex::new(Dedupe::new(DEDUPE_WINDOW)),
            dropped: AtomicU64::new(0),
        }
    }

    fn dedupe(&self) -> std::sync::MutexGuard<'_, Dedupe> {
        // 去重表只是性能优化，锁中毒也不该让告警链路 panic
        self.dedupe
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}
/// 推送句柄（`AppState` 上挂一份，可自由 clone）。
#[derive(Clone)]
pub struct Notifier {
    tx: Option<mpsc::Sender<Notice>>,
    min_level: Level,
    mention_all: bool,
    /// 可热更新的目标地址（设置页改完立即生效）
    webhook: Arc<Mutex<WebhookTarget>>,
    shared: Arc<Shared>,
}

impl Notifier {
    /// 装配句柄 + 消费端。
    ///
    /// `platform_webhook` 是启动时从设置表读到的地址（平台里填的那个）；
    /// 它和 `config.wecom_webhook`（环境变量 / 配置文件）同时存在时**平台优先**。
    ///
    /// 注意：即使当下一个地址都没有，只要部署级开关开着，消费端照样启动 ——
    /// 否则现场在页面上填完地址还得重启进程才能生效。
    pub fn channel(
        config: &Config,
        platform_webhook: Option<String>,
    ) -> (Self, Option<mpsc::Receiver<Notice>>) {
        let min_level = Level::parse(&config.wecom_min_level);
        let webhook = Arc::new(Mutex::new(WebhookTarget::resolve(
            platform_webhook,
            config.wecom_webhook.clone(),
        )));
        if !config.wecom_enabled {
            // 部署级总开关关掉：连消费端都不启动，页面上的配置也不该偷偷发消息
            return (
                Self {
                    tx: None,
                    min_level,
                    mention_all: false,
                    webhook,
                    shared: Arc::new(Shared::new()),
                },
                None,
            );
        }
        let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
        (
            Self {
                tx: Some(tx),
                min_level,
                mention_all: config.wecom_mention_all,
                webhook,
                shared: Arc::new(Shared::new()),
            },
            Some(rx),
        )
    }

    /// 永不发送的句柄：未配置 webhook、单元/集成测试装配用。
    pub fn disabled() -> Self {
        Self {
            tx: None,
            min_level: Level::Warning,
            mention_all: false,
            webhook: Arc::new(Mutex::new(WebhookTarget::default())),
            shared: Arc::new(Shared::new()),
        }
    }

    /// 当前生效的 webhook 地址。
    pub fn webhook(&self) -> Option<String> {
        self.webhook_target().url
    }

    /// 当前生效的 webhook + 来源（设置页展示「这是环境变量还是平台配的」）。
    pub fn webhook_snapshot(&self) -> (Option<String>, Option<WebhookSource>) {
        let target = self.webhook_target();
        (target.url, target.source)
    }

    /// 热更新目标地址。设置页保存 / 清除后调用，不需要重启进程。
    pub fn set_webhook(&self, url: Option<String>, source: Option<WebhookSource>) {
        let mut slot = self
            .webhook
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *slot = WebhookTarget { url, source };
    }

    fn webhook_target(&self) -> WebhookTarget {
        self.webhook
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// 通道是否可用：部署开关开着、有地址、有消费端。
    pub fn is_enabled(&self) -> bool {
        self.tx.is_some() && self.webhook().is_some()
    }

    /// 入队。**同步、不阻塞**：永不 await。
    ///
    /// 返回 `true` 表示已进入队列（不代表已发出）。
    pub fn enqueue(&self, mut notice: Notice) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        // 地址可能刚在页面上被清空：这一条直接丢，别压进队列等一个不存在的目标
        if self.webhook().is_none() {
            debug!(key = %notice.dedupe_key, "wecom: 未配置 webhook，跳过");
            return false;
        }
        if notice.level < self.min_level && !notice.bypass_min_level {
            debug!(
                key = %notice.dedupe_key,
                level = notice.level.label(),
                "wecom: 低于推送级别，跳过"
            );
            return false;
        }
        notice.mention_all = notice.mention_all && self.mention_all;
        // 去重键 `alert-repeat:<工单>` 会复用（同一问题连刷合并），所以被挡时
        // **必须留一行生产可见的日志**：2026-11-19 现场「重跑后 AI 没上报」
        // 就是在这里被静默合并掉的（只有 debug，`notifications` 也不落行）。
        {
            let mut dedupe = self.shared.dedupe();
            let window = notice.dedupe_window;
            let verdict = match window {
                Some(window) => dedupe.accept_within(&notice.dedupe_key, Instant::now(), window),
                None => dedupe.accept(&notice.dedupe_key, Instant::now()),
            };
            if let Err(remaining) = verdict {
                let remaining = remaining.as_secs();
                info!(
                    key = %notice.dedupe_key,
                    remaining_secs = remaining,
                    "wecom: 去重窗口内，跳过本条推送（还有 {remaining}s 到期；告警/工单本身已入库）"
                );
                return false;
            }
        }
        // 证据图是队列里最贵的东西（单条最多 2MB base64）。队列积压时**先丢图保卡片**：
        // 正文才是告警信息本身，图只是辅证，不能因为一张图把队列存储吃到爆炸。
        if notice.image.is_some() && tx.capacity() < QUEUE_CAPACITY / 2 {
            debug!(key = %notice.dedupe_key, "wecom: 队列积压，本条只发卡片不带证据图");
            notice.image = None;
        }
        match tx.try_send(notice) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                warn!("wecom: 推送队列已满，本条丢弃（60s 内汇总提示）");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}
/// 启动消费端（`main.rs` 装配）。
pub fn spawn(ctx: NotifyCtx, rx: mpsc::Receiver<Notice>) {
    tokio::spawn(async move {
        run_consumer(ctx, rx).await;
    });
}

async fn run_consumer(ctx: NotifyCtx, mut rx: mpsc::Receiver<Notice>) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            warn!("wecom: 构建 HTTP 客户端失败，推送通道关闭: {error}");
            return;
        }
    };
    let mut gate = RateGate::new(RATE_PER_MIN, Duration::from_secs(60));
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    info!(
        min_level = ctx.notifier.min_level.label(),
        "企业微信推送通道已启动"
    );

    loop {
        tokio::select! {
            maybe = rx.recv() => {
                let Some(notice) = maybe else { break };
                // 每条都取一次：设置页改地址后不重启即生效，清空后立刻停发
                let Some(url) = ctx.notifier.webhook() else {
                    debug!(key = %notice.dedupe_key, "wecom: 未配置 webhook，丢弃");
                    continue;
                };
                // 额度在**真正发送的那一刻**才可取（`RateGate::acquire`），
                // 不再在这里预扣：预扣会让被拒绝（errcode≠0）/ 因去重没发出去的消息
                // 白占额度，而降级汇总又另建了一把 gate，两边各算各的。
                deliver(&ctx, &client, &url, &notice, &mut gate).await;
                continue;
            }
            _ = ticker.tick() => {
                flush_overflow(&ctx, &client, &mut gate).await;
            }
        }
    }
}
