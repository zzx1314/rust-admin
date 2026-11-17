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
//! - **webhook 地址本身就是全部凭据**，没有加签。所以它只从环境变量
//!   `ROBODOG_WECOM_WEBHOOK` 读，不进 `server.toml`、不进 SQLite、不进日志。
//! - **20 条/分钟**：超了返回 `errcode 45009`。必须本地限流。
//! - **markdown ≤ 4096 字节**（中文 3 字节/字）：超长整条被拒，不是截断。
//! - **markdown 不支持 @提及**：要 @所有人得另发一条 `text`，用 `mentioned_list`。
//! - **只能发不能收**：消息里带平台链接，处理动作仍在平台内完成。
//!
//! # 不能阻塞调用方
//!
//! 所有 `enqueue` 都是同步的 `try_send`，队列满就丢并计数（每 60s 汇总一条
//! 「另有 N 条未逐条推送」）。绝不能在 WS handler / 看门狗里 await 外网 HTTP：
//! `qyapi.weixin.qq.com` 一次超时 10s，会把心跳处理和看门狗 tick 全拖住。

use crate::app::AppState;
use crate::config::Config;
use crate::db::now;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// markdown content 上限（**字节**）。留 96 字节余量给可能的包装。
pub const MARKDOWN_LIMIT: usize = 4000;
/// text content 上限（字节）。
pub const TEXT_LIMIT: usize = 2000;
/// 本地限流：比企业微信的 20 条/分钟留 2 条余量给「测试消息」和重试。
pub const RATE_PER_MIN: u32 = 18;
/// 队列深度。看门狗 15s 一轮、设备事件突发，256 足够。
const QUEUE_CAPACITY: usize = 256;
/// 同一 `dedupe_key` 的静默窗口。
///
/// 心跳看门狗 15s 扫一轮、客户端 5s 报一次进度，同一件事会被"发现"很多次。
const DEDUPE_WINDOW: Duration = Duration::from_secs(300);

/// 告警级别。顺序即严重度：`Info < Warning < Critical`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warning,
    Critical,
}

impl Level {
    /// 从库里/配置里的中文级别解析，未知值按「警告」处理（宁可多推不可漏推）。
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "严重" | "critical" | "error" => Self::Critical,
            "提示" | "info" => Self::Info,
            _ => Self::Warning,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Critical => "严重",
            Self::Warning => "警告",
            Self::Info => "提示",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Critical => "🔴",
            Self::Warning => "🟠",
            Self::Info => "🔵",
        }
    }

    /// 企业微信 markdown 只认这三个颜色。
    fn color(self) -> &'static str {
        match self {
            Self::Critical => "warning",
            Self::Warning => "comment",
            Self::Info => "info",
        }
    }
}

/// 推送的业务来源（决定取哪两个设置开关）。
#[derive(Clone, Copy)]
enum Channel {
    Alert,
    Task,
}

impl Channel {
    /// 两个开关都必须为 `true`：一个是运营在页面上的总开关，一个是本渠道的开关。
    fn settings_keys(self) -> [&'static str; 2] {
        match self {
            Self::Alert => ["alert_push", "wecom_alert_push"],
            Self::Task => ["task_timeout_notice", "wecom_task_abnormal"],
        }
    }

    fn ref_type(self) -> &'static str {
        match self {
            Self::Alert => "alert",
            Self::Task => "task",
        }
    }
}

/// 一条待推送的消息。只存结构化字段，渲染放在消费端做（便于单测）。
#[derive(Clone, Debug)]
pub struct Notice {
    pub level: Level,
    pub ref_type: &'static str,
    pub ref_id: Option<i64>,
    /// 去重键：告警用 `alert_code`，任务用 `task:{task_code}`。
    pub dedupe_key: String,
    pub title: String,
    /// 字段名 → 值，渲染成 `> 名：值` 的引用行。
    pub fields: Vec<(&'static str, String)>,
    pub body: Option<String>,
    pub action: Option<String>,
    pub link: Option<String>,
    /// 严重级别是否 @所有人（由配置 `wecom_mention_all` 决定，构造时写入）。
    pub mention_all: bool,
    /// 要 @ 的具体责任人手机号（企业微信 `mentioned_mobile_list`）。
    ///
    /// 群里只有手机号能被 @：责任人库没有电话时就只能靠 markdown 卡片里那一行
    /// 「当班责任人」让人自己对号。与 `mention_all` 互斥使用（先 @所有人，否则 @具体人）。
    pub mentions: Vec<String>,
}

impl Notice {
    /// 渲染成企业微信 markdown。
    ///
    /// 只用企业微信支持的语法：`##` 标题、`>` 引用、`**加粗**`、`[文字](链接)`、
    /// `<font color>`。**不用 `-` 列表**（群机器人 markdown 不支持）。
    pub fn markdown(&self) -> String {
        let mut out = String::with_capacity(512);
        out.push_str(&format!(
            "## {} <font color=\"{}\">{}</font> · {}\n",
            self.level.icon(),
            self.level.color(),
            self.level.label(),
            self.title
        ));
        for (name, value) in &self.fields {
            if value.trim().is_empty() {
                continue;
            }
            out.push_str(&format!("> {name}：{value}\n"));
        }
        if let Some(body) = self.body.as_deref().filter(|body| !body.trim().is_empty()) {
            out.push_str(&format!("\n**内容**：{}\n", body.trim()));
        }
        if let Some(action) = self
            .action
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            out.push_str(&format!("\n**处置建议**：{}\n", action.trim()));
        }
        if let Some(link) = &self.link {
            out.push_str(&format!("\n[查看详情 →]({link})"));
        }
        truncate_bytes(out.trim_end(), MARKDOWN_LIMIT)
    }

    /// `@所有人` / `@当班责任人` 用的一条短文本。
    ///
    /// 单独发而不是合并进 markdown：群机器人 markdown 不支持提及，
    /// 而 `mentioned_list` / `mentioned_mobile_list` 只有 `text` 类型才认。
    ///
    /// **只当提醒，不复述卡片**：markdown 卡片紧跟在后面，把标题、字段和正文
    /// 再拼一遍会让群里出现两条几乎一样的「设备异常告警」，现场会误以为是
    /// 重复告警（2026-11-17 反馈：一条 markdown 卡片 + 一条一模一样的 text）。
    /// 所以这里只保留级别 + 标题 + 前两个非空字段（设备/园区）。
    pub fn mention_text(&self) -> String {
        let mut line = format!("【{}】{}", self.level.label(), self.title);
        for (name, value) in self
            .fields
            .iter()
            .filter(|(_, value)| !value.trim().is_empty())
            .take(2)
        {
            line.push_str(&format!("　{name}：{}", value.trim()));
        }
        truncate_bytes(&line, TEXT_LIMIT)
    }
}

/// 按**字节**截断，且不切断多字节字符。
///
/// 企业微信的限制是字节不是字符：中文 3 字节/字，4096 字节只够一千多字。
/// 超长是整条被拒（`errcode 40058`），所以必须自己截。
pub fn truncate_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    const SUFFIX: &str = "…（已截断）";
    if max_bytes <= SUFFIX.len() {
        let mut end = max_bytes;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        return text[..end].to_string();
    }
    let budget = max_bytes - SUFFIX.len();
    let mut end = budget;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(max_bytes);
    out.push_str(&text[..end]);
    out.push_str(SUFFIX);
    out
}

/// 去重窗口：同一键在 `window` 内只放行一次。
struct Dedupe {
    seen: HashMap<String, Instant>,
    window: Duration,
}

impl Dedupe {
    fn new(window: Duration) -> Self {
        Self {
            seen: HashMap::new(),
            window,
        }
    }

    fn accept(&mut self, key: &str, at: Instant) -> bool {
        // 顺带清理过期项：告警量级下这张表始终只有几十个键
        self.seen
            .retain(|_, seen_at| at.saturating_duration_since(*seen_at) < self.window);
        if self.seen.contains_key(key) {
            return false;
        }
        self.seen.insert(key.to_string(), at);
        true
    }
}

/// 固定窗口限流。单消费端独占，不需要加锁。
struct RateGate {
    capacity: u32,
    window: Duration,
    used: u32,
    started: Instant,
}

impl RateGate {
    fn new(capacity: u32, window: Duration) -> Self {
        Self {
            capacity,
            window,
            used: 0,
            started: Instant::now(),
        }
    }

    fn allow(&mut self, at: Instant) -> bool {
        if at.saturating_duration_since(self.started) >= self.window {
            self.started = at;
            self.used = 0;
        }
        if self.used < self.capacity {
            self.used += 1;
            true
        } else {
            false
        }
    }

    /// 距离下一个窗口还要等多久。
    fn wait_time(&self, at: Instant) -> Duration {
        self.window
            .saturating_sub(at.saturating_duration_since(self.started))
    }
}

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
    shared: Arc<Shared>,
}

impl Notifier {
    /// 装配句柄 + 消费端。未配置 webhook 时返回 `None` 消费端，所有 `enqueue` 静默丢弃。
    pub fn channel(config: &Config) -> (Self, Option<mpsc::Receiver<Notice>>) {
        let min_level = Level::parse(&config.wecom_min_level);
        if !config.wecom_ready() {
            return (
                Self {
                    tx: None,
                    min_level,
                    mention_all: false,
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
            shared: Arc::new(Shared::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// 入队。**同步、不阻塞**：永不 await。
    ///
    /// 返回 `true` 表示已进入队列（不代表已发出）。
    pub fn enqueue(&self, mut notice: Notice) -> bool {
        let Some(tx) = &self.tx else {
            return false;
        };
        if notice.level < self.min_level {
            debug!(
                key = %notice.dedupe_key,
                level = notice.level.label(),
                "wecom: 低于推送级别，跳过"
            );
            return false;
        }
        notice.mention_all = notice.mention_all && self.mention_all;
        {
            let mut dedupe = self.shared.dedupe();
            if !dedupe.accept(&notice.dedupe_key, Instant::now()) {
                debug!(key = %notice.dedupe_key, "wecom: 去重窗口内，跳过");
                return false;
            }
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
pub fn spawn(state: AppState, rx: mpsc::Receiver<Notice>) {
    tokio::spawn(async move {
        run_consumer(state, rx).await;
    });
}

async fn run_consumer(state: AppState, mut rx: mpsc::Receiver<Notice>) {
    let Some(url) = state.config.wecom_webhook.clone() else {
        return;
    };
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
        min_level = state.notifier.min_level.label(),
        "企业微信推送通道已启动"
    );

    loop {
        tokio::select! {
            maybe = rx.recv() => {
                let Some(notice) = maybe else { break };
                // @所有人 / @具体责任人都要多发一条 text，企业微信按「消息条数」限流，
                // 所以占两个额度
                let cost = if notice.mention_all || !notice.mentions.is_empty() {
                    2
                } else {
                    1
                };
                for _ in 0..cost {
                    while !gate.allow(Instant::now()) {
                        let wait = gate.wait_time(Instant::now());
                        debug!("wecom: 触发本地限流，等待 {wait:?}");
                        tokio::time::sleep(wait).await;
                    }
                }
                deliver(&state, &client, &url, &notice, &mut gate).await;
            }
            _ = ticker.tick() => {
                flush_overflow(&state).await;
            }
        }
    }
}

/// 队列满丢掉的条数汇总成一条，避免"静默丢消息"。
async fn flush_overflow(state: &AppState) {
    let dropped = state.notifier.shared.dropped.swap(0, Ordering::Relaxed);
    if dropped == 0 {
        return;
    }
    let notice = Notice {
        level: Level::Warning,
        ref_type: "summary",
        ref_id: None,
        dedupe_key: format!("summary:{}", now()),
        title: "告警推送积压".to_string(),
        fields: vec![("说明", "以下消息因队列积压未逐条推送".to_string())],
        body: Some(format!("过去 1 分钟内有 {dropped} 条告警未逐条推送")),
        action: Some("请到告警中心查看完整列表".to_string()),
        link: alerts_link(state),
        mention_all: false,
        mentions: Vec::new(),
    };
    let Some(url) = state.config.wecom_webhook.clone() else {
        return;
    };
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    else {
        return;
    };
    let mut gate = RateGate::new(RATE_PER_MIN, Duration::from_secs(60));
    deliver(state, &client, &url, &notice, &mut gate).await;
}

/// 发送一条（含重试与落库）。
async fn deliver(
    state: &AppState,
    client: &reqwest::Client,
    url: &str,
    notice: &Notice,
    gate: &mut RateGate,
) {
    let payload = notice.markdown();

    if notice.mention_all {
        // @所有人先发：群消息顺序即发送顺序，提醒在前、详情在后
        let body = json!({
            "msgtype": "text",
            "text": {
                "content": truncate_bytes(&notice.mention_text(), TEXT_LIMIT),
                "mentioned_list": ["@all"],
            }
        });
        match post(client, url, &body).await {
            Ok(()) => {}
            Err(failure) => warn!("wecom: @所有人消息发送失败: {}", failure.message),
        }
    } else if !notice.mentions.is_empty() {
        // @当班责任人：markdown 不支持提及，只能另发一条 text 带手机号。
        // 手机号没在企微通讯录里时企微不报错、只不置红，所以这条失败也不影响正文
        let body = json!({
            "msgtype": "text",
            "text": {
                "content": truncate_bytes(&notice.mention_text(), TEXT_LIMIT),
                "mentioned_mobile_list": notice.mentions,
            }
        });
        match post(client, url, &body).await {
            Ok(()) => {}
            Err(failure) => warn!("wecom: @责任人消息发送失败: {}", failure.message),
        }
    }

    let body = json!({ "msgtype": "markdown", "markdown": { "content": payload } });
    let mut attempts = 0_i64;
    let mut last_error = String::new();
    for round in 0..3 {
        attempts += 1;
        match post(client, url, &body).await {
            Ok(()) => {
                record(state, notice, &payload, "sent", attempts, None).await;
                debug!(key = %notice.dedupe_key, "wecom: 推送成功");
                return;
            }
            Err(failure) => {
                last_error = failure.message.clone();
                match failure.kind {
                    // 服务端限流：等到下一个窗口再试一次，不再重试
                    FailureKind::RateLimited => {
                        let wait = gate.wait_time(Instant::now());
                        warn!("wecom: 触发服务端限流，{wait:?} 后重试");
                        tokio::time::sleep(wait).await;
                    }
                    // 地址/参数错误重试没有意义（93000 = key 失效）
                    FailureKind::Rejected => break,
                    FailureKind::Transport => {
                        if round < 2 {
                            tokio::time::sleep(Duration::from_secs(1 + round as u64 * 2)).await;
                        }
                    }
                }
            }
        }
    }
    warn!(key = %notice.dedupe_key, error = %last_error, "wecom: 推送失败");
    record(
        state,
        notice,
        &payload,
        "failed",
        attempts,
        Some(&last_error),
    )
    .await;
}

enum FailureKind {
    /// 45009：每分钟条数超限
    RateLimited,
    /// errcode != 0 的其他情况：key 失效、内容超长等，重试无用
    Rejected,
    /// 网络 / HTTP 层失败
    Transport,
}

struct SendFailure {
    kind: FailureKind,
    message: String,
}

async fn post(client: &reqwest::Client, url: &str, body: &Value) -> Result<(), SendFailure> {
    let response = client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|error| SendFailure {
            kind: FailureKind::Transport,
            message: format!("请求失败: {error}"),
        })?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(SendFailure {
            kind: FailureKind::Transport,
            message: format!("HTTP {status}: {}", truncate_bytes(&text, 200)),
        });
    }
    // 企业微信 HTTP 200 但 errcode != 0
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let errcode = value.get("errcode").and_then(Value::as_i64).unwrap_or(-1);
    if errcode == 0 {
        return Ok(());
    }
    let errmsg = value
        .get("errmsg")
        .and_then(Value::as_str)
        .unwrap_or("未知错误");
    Err(SendFailure {
        kind: if errcode == 45009 {
            FailureKind::RateLimited
        } else {
            FailureKind::Rejected
        },
        message: format!("errcode {errcode}: {errmsg}"),
    })
}

async fn record(
    state: &AppState,
    notice: &Notice,
    payload: &str,
    status: &str,
    attempts: i64,
    error: Option<&str>,
) {
    let timestamp = now();
    let result = sqlx::query(
        "INSERT INTO notifications
            (channel, ref_type, ref_id, level, dedupe_key, payload, status, attempts,
             last_error, created_at, sent_at)
         VALUES ('wecom', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(notice.ref_type)
    .bind(notice.ref_id)
    .bind(notice.level.label())
    .bind(&notice.dedupe_key)
    .bind(payload)
    .bind(status)
    .bind(attempts)
    .bind(error)
    .bind(&timestamp)
    .bind(if status == "sent" {
        Some(timestamp.clone())
    } else {
        None
    })
    .execute(&state.db)
    .await;
    if let Err(error) = result {
        warn!("wecom: 写推送记录失败: {error}");
    }
}

fn alerts_link(state: &AppState) -> Option<String> {
    state
        .config
        .public_url
        .as_deref()
        .map(|base| format!("{base}/alerts"))
}

/// 只 @ 看起来像手机号的号码。
///
/// 责任人库里的电话可能是脱敏值（演示数据是 `138****1203`），把它塞进
/// `mentioned_mobile_list` 会让整条 text 被企微按参数错误拒掉 —— 一条 @ 都发不出去。
/// 所以先做一次形状校验：失败就只保留 markdown 卡片里的「当班责任人」那一行。
fn mentionable(phone: &str) -> Option<String> {
    let phone = phone.trim();
    let ok = phone.len() >= 5
        && !phone.contains('*')
        && phone
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch == '+' || ch == '-');
    ok.then(|| phone.to_string())
}

fn work_orders_link(state: &AppState, order_id: Option<i64>) -> Option<String> {
    state
        .config
        .public_url
        .as_deref()
        .map(|base| match order_id {
            Some(id) => format!("{base}/workorders?focus={id}"),
            None => format!("{base}/workorders"),
        })
}

fn tasks_link(state: &AppState, task_id: Option<i64>) -> Option<String> {
    state
        .config
        .public_url
        .as_deref()
        .map(|base| match task_id {
            Some(id) => format!("{base}/tasks?focus={id}"),
            None => format!("{base}/tasks"),
        })
}

/// 两个开关都为真才推。
///
/// 缺行视为开启：新增开关项在旧库上还没播种时，不应该把告警静默掉。
async fn channel_enabled(state: &AppState, channel: Channel) -> bool {
    let [outer, inner] = channel.settings_keys();
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT setting_key, setting_value FROM settings WHERE setting_key IN (?1, ?2)",
    )
    .bind(outer)
    .bind(inner)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let read = |key: &str| {
        rows.iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, value)| value.as_deref())
            .map(|value| value == "true")
            .unwrap_or(true)
    };
    read(outer) && read(inner)
}

/// `alert_created` 回查用：告警 + 设备 + 园区 + 区域。
#[derive(Debug, sqlx::FromRow)]
struct AlertLookup {
    id: i64,
    alert_code: String,
    level: String,
    content: String,
    occurred_at: String,
    device_code: Option<String>,
    park_name: Option<String>,
    zone_name: Option<String>,
}

/// `task_abnormal` 回查用：任务 + 设备 + 园区。
#[derive(Debug, sqlx::FromRow)]
struct TaskLookup {
    id: i64,
    task_code: String,
    task_name: String,
    progress: i64,
    park_id: Option<i64>,
    device_code: Option<String>,
    park_name: Option<String>,
}

/// 告警落库后调用：找责任人 → 自动建工单 → 推送企微。
///
/// 顺序不能颠倒：
///
/// 1. **派单 + 建工单**（[`crate::alert_flow::on_alert`]）放在最前面，且**不看推送开关**。
///    现场很可能根本没配 webhook，但「告警要有人管」不能依赖群消息通道。
/// 2. 推送才是旁路，失败只打日志。
///
/// 消息里因此能直接带「当班责任人 / 工单号」，并把责任人手机号塞进
/// `mentioned_mobile_list`（markdown 不支持 @，只能另发一条 text）。
pub async fn alert_created(state: &AppState, alert_code: &str) {
    let assignment = crate::alert_flow::on_alert(state, alert_code).await;

    // 同一设备的同一故障还在办：群里已经通知过一次，这条只是在「跟单」。
    // 现场实测：机器人每 ~90s 报一次 `12608 RS 与 lidar 时间不同步`，
    // 90 秒一条群消息没人受得了 —— 告警中心里仍然看得到，只是不再推。
    if assignment
        .as_ref()
        .is_some_and(|assignment| assignment.repeated)
    {
        debug!(alert = %alert_code, "wecom: 同类告警已有在办工单，跳过推送");
        return;
    }

    if !state.notifier.is_enabled() {
        return;
    }
    if !channel_enabled(state, Channel::Alert).await {
        return;
    }
    let row: Option<AlertLookup> = sqlx::query_as(
        "SELECT a.id, a.alert_code, a.level, a.content, a.occurred_at,
                    d.device_id AS device_code, p.park_name, z.zone_name
               FROM alerts a
               LEFT JOIN devices d ON d.id = a.device_id
               LEFT JOIN parks p ON p.id = COALESCE(a.park_id, d.park_id)
               LEFT JOIN zones z ON z.id = d.zone_id
              WHERE a.alert_code = ?1",
    )
    .bind(alert_code)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let Some(row) = row else {
        debug!("wecom: 告警 {alert_code} 回查不到，跳过推送");
        return;
    };

    let mut fields: Vec<(&'static str, String)> = Vec::with_capacity(9);
    fields.push((
        "设备",
        row.device_code
            .clone()
            .unwrap_or_else(|| "未绑定设备".to_string()),
    ));
    fields.push((
        "园区",
        row.park_name
            .clone()
            .unwrap_or_else(|| "未绑定园区".to_string()),
    ));
    if let Some(zone) = &row.zone_name {
        fields.push(("区域", zone.clone()));
    }
    if let Some(assignment) = &assignment {
        if let Some(task_name) = &assignment.task_name {
            fields.push(("关联计划", task_name.clone()));
        }
        fields.push(("当班责任人", assignment.summary()));
        if let Some(order) = &assignment.work_order_code {
            fields.push(("工单", format!("{order}（已自动派单）")));
        }
    }
    fields.push(("时间", row.occurred_at));
    fields.push(("告警码", row.alert_code.clone()));

    // 有工单就链到工单（责任人点进去就能回填处置记录），否则退回告警中心
    let link = assignment
        .as_ref()
        .and_then(|assignment| assignment.work_order_id)
        .and_then(|order_id| work_orders_link(state, Some(order_id)))
        .or_else(|| alerts_link(state));

    let notice = Notice {
        level: Level::parse(&row.level),
        ref_type: Channel::Alert.ref_type(),
        ref_id: Some(row.id),
        dedupe_key: format!("alert:{}", row.alert_code),
        title: if assignment
            .as_ref()
            .is_some_and(|assignment| assignment.work_order_code.is_some())
        {
            "设备异常告警（已自动派单）".to_string()
        } else {
            "设备异常告警".to_string()
        },
        fields,
        body: Some(row.content),
        action: assignment
            .as_ref()
            .map(crate::alert_flow::Assignment::dispatch_sentence),
        link,
        mention_all: false,
        mentions: assignment
            .as_ref()
            .and_then(|assignment| assignment.phone.as_deref())
            .and_then(mentionable)
            .map(|phone| vec![phone])
            .unwrap_or_default(),
    };
    state.notifier.enqueue(notice);
}

/// 任务被判「异常」时调用。看门狗与客户端上报终态两处都走这里，靠去重键合并。
pub async fn task_abnormal(state: &AppState, task_code: &str, level: Level, reason: &str) {
    if !state.notifier.is_enabled() {
        return;
    }
    if !channel_enabled(state, Channel::Task).await {
        return;
    }
    let row: Option<TaskLookup> = sqlx::query_as(
        "SELECT t.id, t.task_code, t.task_name, t.progress, t.park_id,
                d.device_id AS device_code, p.park_name
           FROM tasks t
           LEFT JOIN devices d ON d.id = t.device_id
           LEFT JOIN parks p ON p.id = t.park_id
          WHERE t.task_code = ?1",
    )
    .bind(task_code)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let Some(row) = row else {
        debug!("wecom: 任务 {task_code} 回查不到，跳过推送");
        return;
    };

    // 计划层的责任人：任务异常同样要 @ 到人，否则群里只是「又一条报错」
    let assignment =
        crate::alert_flow::responsible_for_task(state, row.park_id, Some(row.id)).await;

    let mut fields: Vec<(&'static str, String)> = Vec::with_capacity(5);
    fields.push(("任务", format!("{}（{}）", row.task_name, row.task_code)));
    fields.push((
        "设备",
        row.device_code
            .clone()
            .unwrap_or_else(|| "未绑定设备".to_string()),
    ));
    fields.push((
        "园区",
        row.park_name
            .clone()
            .unwrap_or_else(|| "未绑定园区".to_string()),
    ));
    fields.push(("进度", format!("{}%", row.progress)));
    if let Some(assignment) = &assignment {
        fields.push(("当班责任人", assignment.summary()));
    }

    let notice = Notice {
        level,
        ref_type: Channel::Task.ref_type(),
        ref_id: Some(row.id),
        dedupe_key: format!("task:{}", row.task_code),
        title: "任务执行异常".to_string(),
        fields,
        body: Some(reason.to_string()),
        action: Some(match &assignment {
            Some(assignment) => format!(
                "{}；请先检查设备在线状态与本体定位/导航状态，确认后在平台重新下发任务",
                assignment.duty_sentence()
            ),
            None => "检查设备在线状态与本体定位/导航状态，确认后在平台重新下发任务".to_string(),
        }),
        link: tasks_link(state, Some(row.id)),
        mention_all: level == Level::Critical,
        mentions: assignment
            .as_ref()
            .and_then(|assignment| assignment.phone.as_deref())
            .and_then(mentionable)
            .map(|phone| vec![phone])
            .unwrap_or_default(),
    };
    state.notifier.enqueue(notice);
}

/// 设置页「发送测试消息」：**同步**发一条并返回企业微信的原始结果。
///
/// 走同步是因为运维需要立刻看到 `errcode`（配错 key 时群里的静默失败最难查）。
pub async fn send_test(state: &AppState) -> Result<String, String> {
    let Some(url) = state.config.wecom_webhook.clone() else {
        return Err(
            "未配置 ROBODOG_WECOM_WEBHOOK：企业微信群机器人地址只能从环境变量读取".to_string(),
        );
    };
    if !state.config.wecom_enabled {
        return Err("推送通道已在配置中关闭（wecom.enabled = false）".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("构建 HTTP 客户端失败: {error}"))?;
    let notice = Notice {
        level: Level::Info,
        ref_type: "test",
        ref_id: None,
        dedupe_key: format!("test:{}", now()),
        title: "推送通道测试".to_string(),
        fields: vec![
            ("平台", state.config.public_url.clone().unwrap_or_default()),
            ("时间", now()),
        ],
        body: Some("这是一条手动触发的测试消息，收到即表示企业微信告警推送可用".to_string()),
        action: None,
        link: None,
        mention_all: false,
        mentions: Vec::new(),
    };
    let payload = notice.markdown();
    let body = json!({ "msgtype": "markdown", "markdown": { "content": payload } });
    match post(&client, &url, &body).await {
        Ok(()) => {
            record(state, &notice, &payload, "sent", 1, None).await;
            info!("wecom: 测试消息发送成功");
            Ok("测试消息已发送，请检查企业微信群".to_string())
        }
        Err(failure) => {
            record(
                state,
                &notice,
                &payload,
                "failed",
                1,
                Some(&failure.message),
            )
            .await;
            Err(failure.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(level: Level) -> Notice {
        Notice {
            level,
            ref_type: "alert",
            ref_id: Some(1),
            dedupe_key: "alert:AL-1".to_string(),
            title: "设备异常告警".to_string(),
            fields: vec![
                ("设备", "DOG-003".to_string()),
                ("园区", "张江智造园区".to_string()),
                ("时间", "2026-05-21 14:32:07".to_string()),
                ("告警码", "AL-WS-2026052114".to_string()),
            ],
            body: Some("导航指令非法（13440），航点 3 无法到达".to_string()),
            action: None,
            link: Some("http://10.0.0.8:3100/alerts".to_string()),
            mention_all: false,
            mentions: Vec::new(),
        }
    }

    #[test]
    fn mentions_skip_masked_phones() {
        // 演示数据里的脱敏号码不能进 mentioned_mobile_list
        assert_eq!(mentionable("138****1203"), None);
        assert_eq!(mentionable("13800001203"), Some("13800001203".to_string()));
        assert_eq!(
            mentionable("+8613800001203"),
            Some("+8613800001203".to_string())
        );
        assert_eq!(mentionable(""), None);
        assert_eq!(mentionable("1234"), None, "太短的不当作号码");
    }

    #[test]
    fn level_parsing_and_ordering() {
        assert_eq!(Level::parse("严重"), Level::Critical);
        assert_eq!(Level::parse("警告"), Level::Warning);
        assert_eq!(Level::parse("提示"), Level::Info);
        assert_eq!(Level::parse("不认识"), Level::Warning);
        assert!(Level::Critical > Level::Warning && Level::Warning > Level::Info);
    }

    #[test]
    fn markdown_uses_only_supported_syntax() {
        let text = notice(Level::Critical).markdown();
        assert!(text.starts_with("## 🔴 <font color=\"warning\">严重</font> · 设备异常告警"));
        assert!(text.contains("> 设备：DOG-003"));
        assert!(text.contains("**内容**：导航指令非法"));
        assert!(text.contains("[查看详情 →](http://10.0.0.8:3100/alerts)"));
        // 群机器人 markdown 不支持 `-` 列表，出现就是渲染成字面量
        assert!(!text.lines().any(|line| line.starts_with("- ")));
    }

    #[test]
    fn markdown_skips_empty_fields() {
        let mut item = notice(Level::Warning);
        item.fields.push(("园区", "   ".to_string()));
        let text = item.markdown();
        assert!(!text.contains("园区：\n"), "空字段不应渲染成 `> 园区：`");
    }

    #[test]
    fn truncate_respects_byte_limit_and_char_boundary() {
        // 中文 3 字节/字：6000 字节必须被压到 4000 以内
        let text = "告警".repeat(2000);
        let clipped = truncate_bytes(&text, MARKDOWN_LIMIT);
        assert!(clipped.len() <= MARKDOWN_LIMIT);
        assert!(clipped.ends_with("（已截断）"));
        // 截断点必须落在字符边界上：留下的部分必须是原文的前缀
        let suffix_len = "…（已截断）".len();
        let body = &clipped[..clipped.len() - suffix_len];
        assert!(text.starts_with(body), "截断后不应出现半个汉字");
        assert!(!body.is_empty());

        let short = truncate_bytes("短内容", MARKDOWN_LIMIT);
        assert_eq!(short, "短内容");

        // 上限比后缀还小时也得返回合法 UTF-8
        let tiny = truncate_bytes("告警示", "…".len());
        assert!(tiny.len() <= "…".len());
    }

    #[test]
    fn long_alert_is_clipped_under_wecom_limit() {
        let mut item = notice(Level::Critical);
        item.body = Some("导航指令非法".repeat(2000));
        let text = item.markdown();
        assert!(
            text.len() <= MARKDOWN_LIMIT,
            "超过 4096 字节会被企微整条拒绝"
        );
    }

    #[test]
    fn dedupe_suppresses_repeats_inside_window() {
        let mut dedupe = Dedupe::new(Duration::from_secs(300));
        let start = Instant::now();
        assert!(dedupe.accept("alert:AL-1", start));
        assert!(!dedupe.accept("alert:AL-1", start + Duration::from_secs(10)));
        // 窗口外重新放行
        assert!(dedupe.accept("alert:AL-1", start + Duration::from_secs(400)));
        // 不同键互不影响
        assert!(dedupe.accept("alert:AL-2", start));
    }

    #[test]
    fn rate_gate_allows_capacity_then_waits_for_window() {
        let mut gate = RateGate::new(2, Duration::from_secs(60));
        let start = Instant::now();
        assert!(gate.allow(start));
        assert!(gate.allow(start + Duration::from_secs(1)));
        assert!(!gate.allow(start + Duration::from_secs(2)));
        // Instant 内部精度不足 1ns，用区间比较
        let wait = gate.wait_time(start + Duration::from_secs(20));
        assert!(
            wait <= Duration::from_secs(40) && wait >= Duration::from_secs(39),
            "窗口内应等待约 40s，实际 {wait:?}"
        );
        assert!(gate.allow(start + Duration::from_secs(61)));
    }

    #[test]
    fn rate_gate_capacity_stays_under_wecom_limit() {
        // 企业微信硬限 20 条/分钟，本地留余量；markdown ≤ 4096 字节、text ≤ 2048 字节
        const { assert!(RATE_PER_MIN < 20) };
        const { assert!(MARKDOWN_LIMIT <= 4096) };
        const { assert!(TEXT_LIMIT <= 2048) };
    }

    #[test]
    fn mention_text_is_single_line_and_short() {
        let mut item = notice(Level::Critical);
        item.mention_all = true;
        let text = item.mention_text();
        assert!(!text.contains('\n'));
        assert!(text.starts_with("【严重】设备异常告警"));
        assert!(text.len() <= TEXT_LIMIT);
    }

    #[test]
    fn mention_text_does_not_repeat_the_card() {
        // @责任人时那条 text 紧跟在 markdown 卡片前面；如果它也把正文和
        // 全部字段拼进来，群里就是两条一模一样的告警（2026-11-17 反馈）
        let item = notice(Level::Warning);
        let text = item.mention_text();
        assert!(
            !text.contains("导航指令非法"),
            "正文只在 markdown 卡片里出现一次: {text}"
        );
        assert!(!text.contains("告警码"), "只保留前两个字段: {text}");
        assert!(text.contains("设备：DOG-003"));
        assert!(text.contains("园区：张江智造园区"));
        // 提醒行应该比卡片短得多
        assert!(text.len() < item.markdown().len());
    }

    #[test]
    fn disabled_notifier_drops_everything() {
        let config = Config::from_env();
        let (notifier, rx) = Notifier::channel(&config);
        // 默认没有 webhook → 未启用、无消费端
        assert!(!notifier.is_enabled());
        assert!(rx.is_none());
        assert!(!notifier.enqueue(notice(Level::Critical)));
    }
}
