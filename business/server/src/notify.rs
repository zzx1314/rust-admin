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
use crate::db::now;
use crate::gateway::DeviceConnections;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

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
    /// 覆盖默认去重窗口（`DEDUPE_WINDOW`）：`None` = 5 分钟。
    ///
    /// AI 命中用它压到 1 分钟（`AI_HIT_DEDUPE_WINDOW`）：既要「每轮巡检都进群」，
    /// 又不能让窗口抖动时同一件事几秒一条。
    pub dedupe_window: Option<Duration>,
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
    /// 证据图：渲染成 markdown 之后**另发一条** `msgtype=image`。
    ///
    /// 只有拿得到图片字节的告警才有（板端 AI 识别的**检出**与**数量核对不一致**，
    /// 两类都靠设备客户端从板端存证目录读图）；markdown 里挂不了图，见 [`evidence_image`]。
    pub image: Option<NoticeImage>,
    /// 只发图、不发 markdown 卡片（用于「卡片早就发了，命中帧晚好几秒才落盘」的补发）。
    ///
    /// 企业微信本来就把图当**独立一条**消息发（`msgtype=image`），所以补发只是把
    /// 第二条推后几秒，群里看到的效果与「卡片 + 图」一样；不发卡片是因为重发一遍
    /// 会让群里当成两条告警。
    pub image_only: bool,
    /// 推完这条（含 `msgtype=image`）之后要清掉的板端存证目录。
    ///
    /// 只有**真的带了证据图**的 AI 告警才有（见 [`EvidenceCleanup`]）：图进了群，
    /// 板端那一轮存证（画框 JPG + `result.json`）就没有第二次用途了。
    /// `None`、或者图片那一条发送失败，都不删（删了不可逆，留着还能人工补推）。
    pub cleanup: Option<EvidenceCleanup>,
    /// 忽略 `wecom_min_level`（部署级「只推 ≥ 某级别」的过滤）。
    ///
    /// 只给**航点级别显式打开**的通知用（`notifyWhenCountOk`）：运维在航点上勾了
    /// 「数量一致也通知」就是明确要这条回执，再被全局的 `min_level=警告` 静默掉
    /// 会表现为「配了却什么都不发」。告警（`alert:` / `task:`）一律 `false`。
    pub bypass_min_level: bool,
}

/// 「证据图已经进群」之后要删掉的板端存证目录（`detect_jobs/<计划>/pt<航点>/`）。
///
/// 板端每命中一次就往 `<存证根>/<计划>/pt<航点>/` 写一张画框 JPG 和一份
/// `result.json`（见 `backpack_dashboard/backend/evidence.py`）；巡检一天几十轮，
/// 板子存储很快被图片吃满。这些图的唯一去处是群里那条 `msgtype=image`
/// （平台读不到板端磁盘，没有第二次用途），所以**推送成功后**让现场客户端把这一轮
/// 的存证删掉。
///
/// 为什么不在这里直接删：路径在**板端**磁盘上，只有与 Agent 同机的设备客户端能删，
/// 所以下发一条 `ai_cleanup` 走已有的设备 WS（见 [`dispatch_evidence_cleanup`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceCleanup {
    /// 存证写在**哪台**设备的盘上：指令要在那台设备的 WS 上下发
    pub device_code: String,
    /// 计划 id（= 业务任务号），板端拿它建目录
    pub plan_id: String,
    /// 航点下标（0 基，与 `routeData.points` 一致）
    pub point_index: i64,
}

/// 从 AI 告警的结构化去重键（`ai:<设备>:<任务>:<航点下标>:<what>`）反推要清理的目录。
///
/// 为什么从 `dedup_key` 反推、而不是给 `alert_created*` 加参数：告警写入点有 5 处，
/// 推送侧只按 `alert_code` 回查（见 [`alert_created`]）。多加一个参数等于让每个调用点
/// 都知道「板端存证怎么放」——那是 `ai_patrol` 与设备客户端之间的事。键里已经带了
/// 设备 / 任务 / 航点，正好够用。
///
/// **设备以键里那个为准**（报警的那个客户端），告警行的 `device_id` 只当兜底：
/// 键里的设备就是开窗、读盘、上报的那个客户端，存证也在它的盘上；
/// 告警行记的是**任务**绑的设备，两者在数据不全时可能不是同一行（集成测试里就遇到过，
/// 指向另一台设备的话 `ai_cleanup` 会发给一个不在线的号，文件永远删不掉）。
///
/// 非 AI 告警（`fault:` / `hb:` / `ctrl:` / `alert:`）返回 `None`：那些告警没有板端存证。
pub fn evidence_cleanup(
    dedup_key: Option<&str>,
    device_code: Option<&str>,
) -> Option<EvidenceCleanup> {
    let key = dedup_key?.trim();
    let mut parts = key.strip_prefix("ai:")?.split(':');
    let keyed_device = parts.next()?.trim();
    let plan_id = parts.next()?.trim();
    let point_index = parts.next()?.trim().parse::<i64>().ok()?;
    if plan_id.is_empty() || point_index < 0 {
        return None;
    }
    let device_code = if keyed_device.is_empty() {
        device_code
            .map(str::trim)
            .filter(|code| !code.is_empty())
            .unwrap_or_default()
    } else {
        keyed_device
    };
    if device_code.is_empty() {
        return None;
    }
    Some(EvidenceCleanup {
        device_code: device_code.to_string(),
        plan_id: plan_id.to_string(),
        point_index,
    })
}

/// 一条待发图片消息（企业微信 `msgtype=image`）。
///
/// 企业微信要的是「原图字节的 base64」+「**base64 之前**字节的 md5」，两个都得给；
/// 不能直接用文件路径：群机器人不能让企微去拉我们的内网地址。
#[derive(Debug, Clone)]
pub struct NoticeImage {
    /// 原图字节的 base64（无换行，由 [`evidence_image`] 规范化）
    pub base64: String,
    /// 原图字节的 md5（小写十六进制）
    pub md5: String,
    /// 原图字节数（只用于日志/排查）
    pub bytes: usize,
}

/// 把客户端/板端给的 base64 证据图变成可发的 [`NoticeImage`]。
///
/// 校验都放在这里，因为企业微信对图片的要求是硬约束，错一个字段整条消息被拒：
///
/// - 必须是合法 base64，**不能带换行**（重新编码一遍，顺手吃掉空格/换行）；
/// - 原图 ≤ [`IMAGE_MAX_BYTES`]（2MB），且必须是 **JPEG / PNG**（按文件头判，不信扩展名）；
/// - md5 算的是**base64 编码前**的字节，算错企业微信回参数错误。
pub fn evidence_image(base64_input: &str) -> Result<NoticeImage, String> {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;

    let cleaned: String = base64_input
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if cleaned.is_empty() {
        return Err("证据图为空".to_string());
    }
    let bytes = BASE64
        .decode(cleaned.as_bytes())
        .map_err(|error| format!("证据图不是合法 base64: {error}"))?;
    if bytes.len() > IMAGE_MAX_BYTES {
        return Err(format!(
            "证据图 {} 字节，超过企业微信 2MB 上限",
            bytes.len()
        ));
    }
    if !is_jpeg_or_png(&bytes) {
        return Err("证据图不是 JPEG/PNG（企业微信只收这两种）".to_string());
    }
    use md5::{Digest as _, Md5};
    let digest = Md5::digest(&bytes);
    let md5 = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(NoticeImage {
        base64: BASE64.encode(&bytes),
        md5,
        bytes: bytes.len(),
    })
}

/// 文件头判定：JPEG `FF D8 FF` / PNG `89 50 4E 47`
fn is_jpeg_or_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47])
}

/// 去掉卡片正文里的板端证据路径（`（证据：/root/…/dogcam_1.jpg）`）。
///
/// 群里**不需要**这串路径：图已经作为 `msgtype=image` 紧跟在卡片后面，而且推完就让
/// 客户端删掉了（[`dispatch_evidence_cleanup`]），留着只会把卡片撑长、还容易被当成
/// 「点得开的链接」。
///
/// **只影响企业微信**：`alerts.content` / 任务时间线照旧留完整路径，告警中心、工单详情
/// 还能回看（`record_hit` / `record_count_alert` / `record_count_ok` 拼的正文不变）。
fn without_evidence_path(text: &str) -> String {
    const MARKERS: [&str; 2] = ["（证据：", "（证据:"];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let found = MARKERS
            .iter()
            .filter_map(|marker| rest.find(marker).map(|at| (at, *marker)))
            .min_by_key(|(at, _)| *at);
        let Some((start, marker)) = found else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + marker.len()..];
        match after.find('）') {
            Some(end) => rest = &after[end + '）'.len_utf8()..],
            // 只有开头没有结尾：剩下的整段丢掉，宁可少一句话也不要把半截路径发进群
            None => break,
        }
    }
    out.trim_end().to_string()
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

/// 去重结果：放行返回 `Ok(())`，被挡返回 `Err(还要等多久)`。
///
/// 把「剩余窗口」返回给调用方，是为了能打成一行**生产可见**的 info：
/// 以前去重只打 debug，现场看到的只有「告警没进群」，无从分辨是没触发还是被合并。
type DedupeResult = Result<(), Duration>;

impl Dedupe {
    fn new(window: Duration) -> Self {
        Self {
            seen: HashMap::new(),
            window,
        }
    }

    fn accept(&mut self, key: &str, at: Instant) -> DedupeResult {
        self.accept_within(key, at, self.window)
    }

    /// 同上，但允许调用方指定窗口（AI 命中用 1 分钟，见 [`AI_HIT_DEDUPE_WINDOW`]）。
    ///
    /// 清理按「两者较大值」做：同一个 `seen` 表里同时存在两种窗口的键，
    /// 而它们的键前缀互斥（`alert:` / `alert-repeat:` / `task:` / `ai:` / …），
    /// 所以各键按自己那一档判定、互不影响。
    fn accept_within(&mut self, key: &str, at: Instant, window: Duration) -> DedupeResult {
        let keep = self.window.max(window);
        // 顺带清理过期项：告警量级下这张表始终只有几十个键
        self.seen
            .retain(|_, seen_at| at.saturating_duration_since(*seen_at) < keep);
        if let Some(seen_at) = self.seen.get(key) {
            let age = at.saturating_duration_since(*seen_at);
            if age < window {
                return Err(window - age);
            }
        }
        // 放行就刷新时间戳（同一键换了更短的窗口时，别按老时间继续算）
        self.seen.insert(key.to_string(), at);
        Ok(())
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

    /// 取一个额度；额度用尽则等到下一个窗口。
    ///
    /// **在发送前调用**（每条消息一取）：企业微信按消息条数限流，
    /// 所以「@所有人 / @责任人」的 text、markdown 卡片、证据图各占一个额度。
    async fn acquire(&mut self) {
        loop {
            let at = Instant::now();
            if self.allow(at) {
                return;
            }
            let wait = self.wait_time(at);
            debug!("wecom: 触发本地限流，等待 {wait:?}");
            tokio::time::sleep(wait).await;
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

/// webhook 从哪来的：平台设置页优先，其次部署时的环境变量/配置文件。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebhookSource {
    /// 平台设置页填的（`settings.wecom_webhook`），改完立即生效
    Platform,
    /// 环境变量 `ROBODOG_WECOM_WEBHOOK`（或 `server.toml` 的 `[wecom] webhook`）
    Env,
}

impl WebhookSource {
    /// 给接口 / 页面用的稳定标识（`db` / `env`）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Platform => "db",
            Self::Env => "env",
        }
    }
}

/// 当前生效的 webhook 及其来源。
///
/// 放在 `Notifier` 里而不是 `Config` 里，是为了**不重启进程**就能改地址：
/// 设置页保存后直接改这个槽，客户端下次发送就用了新地址。
#[derive(Clone, Debug, Default)]
struct WebhookTarget {
    url: Option<String>,
    source: Option<WebhookSource>,
}

impl WebhookTarget {
    /// 平台配置优先，空则回退环境变量的地址。
    fn resolve(platform: Option<String>, env: Option<String>) -> Self {
        let platform = platform.map(|url| url.trim().to_string());
        match platform.filter(|url| !url.is_empty()) {
            Some(url) => Self {
                url: Some(url),
                source: Some(WebhookSource::Platform),
            },
            None => match env.map(|url| url.trim().to_string()) {
                Some(url) if !url.is_empty() => Self {
                    url: Some(url),
                    source: Some(WebhookSource::Env),
                },
                _ => Self::default(),
            },
        }
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

/// 从设置表读平台侧配置的 webhook（空值 / 缺行视为未配置）。
///
/// 启动时读一次交给 [`Notifier::channel`]，之后由设置页通过
/// `PUT /api/notifications/webhook` 热更新，不需要重启进程。
pub async fn stored_webhook(pool: &sqlx::SqlitePool) -> Option<String> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT setting_value FROM settings WHERE setting_key = ?1")
            .bind(WEBHOOK_SETTING_KEY)
            .fetch_optional(pool)
            .await
            .ok()?;
    row.and_then(|(value,)| value)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 校验页面上填的 webhook 地址。
///
/// 不做「必须 qyapi.weixin.qq.com」的硬限制：私有化 / 代理网关的域名不同，
/// 硬卡域名只会让现场以为功能坏了。但**必须有 key=**：没有 key 的地址
/// 发出去只会拿到 `93000`，不如在保存时就报错。
pub fn validate_webhook(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("webhook 地址不能为空（要清空请用「清除」按钮）".to_string());
    }
    if url.len() > 512 {
        return Err("webhook 地址过长（> 512 字节），请检查是不是复制错了".to_string());
    }
    if url.chars().any(char::is_whitespace) {
        return Err("webhook 地址里不能有空格 / 换行，请重新复制一遍".to_string());
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://") || lower.starts_with("http://")) {
        return Err("webhook 地址必须以 https:// 开头（企业微信给的完整地址）".to_string());
    }
    if !url.contains("key=") {
        return Err(
            "webhook 地址缺少 key= 参数：请在企业微信群里「添加群机器人」后完整复制地址"
                .to_string(),
        );
    }
    Ok(url.to_string())
}

/// webhook 只回显 key 的前 8 位：足够核对「填的是哪一个」，又不至于泄漏。
pub fn mask_webhook(url: &str) -> String {
    match url.split_once("key=") {
        Some((prefix, key)) => {
            let head: String = key.chars().take(8).collect();
            format!("{prefix}key={head}…")
        }
        None => "（格式异常：缺少 key= 参数）".to_string(),
    }
}

/// 日志里用的 key 末 6 位；够核对是哪一个机器人，又不把凭据写进 journal。
pub fn key_tail(url: &str) -> String {
    url.chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
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

/// 队列满丢掉的条数汇总成一条，避免"静默丢消息"。
///
/// 走**同一把** `gate`：汇总也是一条企业微信消息，另建限流器会让它绕开额度。
async fn flush_overflow(ctx: &NotifyCtx, client: &reqwest::Client, gate: &mut RateGate) {
    let dropped = ctx.notifier.shared.dropped.swap(0, Ordering::Relaxed);
    if dropped == 0 {
        return;
    }
    let notice = Notice {
        level: Level::Warning,
        ref_type: "summary",
        ref_id: None,
        dedupe_key: format!("summary:{}", now()),
        dedupe_window: None,
        title: "告警推送积压".to_string(),
        fields: vec![("说明", "以下消息因队列积压未逐条推送".to_string())],
        body: Some(format!("过去 1 分钟内有 {dropped} 条告警未逐条推送")),
        action: Some("请到告警中心查看完整列表".to_string()),
        link: alerts_link(ctx),
        mention_all: false,
        mentions: Vec::new(),
        image: None,
        image_only: false,
        cleanup: None,
        bypass_min_level: false,
    };
    let Some(url) = ctx.notifier.webhook() else {
        return;
    };
    deliver(ctx, client, &url, &notice, gate).await;
}

/// 发送一条（含重试与落库）。
async fn deliver(
    ctx: &NotifyCtx,
    client: &reqwest::Client,
    url: &str,
    notice: &Notice,
    gate: &mut RateGate,
) {
    // 只补发图（卡片早就发过了）：发完就返回，不要重发卡片也不要写 notifications
    if notice.image_only {
        if let Some(image) = &notice.image
            && send_image(client, url, gate, image).await
        {
            // 图真的进群了，板端这一轮存证可以删了
            dispatch_evidence_cleanup(ctx, notice).await;
        }
        return;
    }
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
        gate.acquire().await;
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
        gate.acquire().await;
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
        gate.acquire().await;
        match post(client, url, &body).await {
            Ok(()) => {
                record(ctx, notice, &payload, "sent", attempts, None).await;
                debug!(key = %notice.dedupe_key, "wecom: 推送成功");
                // 图片排在正文之后：群里先看到卡片再看图（发送顺序 = 消息顺序）。
                // 失败只打日志——正文已经送到了，不能因一张图把告警判失败。
                if let Some(image) = &notice.image
                    && send_image(client, url, gate, image).await
                {
                    // 企业微信确认收到图之后才删板端存证：删了不可逆
                    dispatch_evidence_cleanup(ctx, notice).await;
                }
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
    record(ctx, notice, &payload, "failed", attempts, Some(&last_error)).await;
}

enum FailureKind {
    /// 45009：每分钟条数超限
    RateLimited,
    /// errcode != 0 的其他情况：key 失效、内容超长等，重试无用
    Rejected,
    /// 网络 / HTTP 层失败
    Transport,
}

/// 发一条 `msgtype=image`（证据图）。
///
/// 企业微信要求：`base64` = **原图字节**的 base64（不能带换行），`md5` = base64 **之前**
/// 字节的 md5，原图 ≤ 2MB 且只有 JPG/PNG。这些校验在 [`evidence_image`] 里已经做完，
/// 到这里就是黑盒发送。
/// 返回**企业微信是否真的收了**：调用方据此决定要不要删板端存证（删了不可逆）。
async fn send_image(
    client: &reqwest::Client,
    url: &str,
    gate: &mut RateGate,
    image: &NoticeImage,
) -> bool {
    let body = json!({
        "msgtype": "image",
        "image": { "base64": image.base64, "md5": image.md5 }
    });
    gate.acquire().await;
    match post(client, url, &body).await {
        Ok(()) => {
            debug!(bytes = image.bytes, "wecom: 证据图推送成功");
            true
        }
        Err(failure) => {
            warn!(
                bytes = image.bytes,
                "wecom: 证据图推送失败（正文已发送）: {}", failure.message
            );
            false
        }
    }
}

/// 证据图进群之后，让板端把这一轮的存证删掉（`detect_jobs/<计划>/pt<航点>/`）。
///
/// 走**已有的设备 WS**（和 `ai_start` / `ai_stop` 同一条链路）：存证在板端磁盘上，
/// 平台读不到也删不到，只有与 Agent 同机的设备客户端能动手。
///
/// 设备不在线（后台重连 / 已下线）就放弃：`send_message_to_device` 返回 false，
/// 文件留在板子上，等于退回老行为——丢的是磁盘，不是告警。
async fn dispatch_evidence_cleanup(ctx: &NotifyCtx, notice: &Notice) {
    let Some(target) = &notice.cleanup else {
        return;
    };
    let sent = crate::gateway::send_message_to_device(
        &ctx.device_connections,
        &target.device_code,
        "ai_cleanup",
        json!({
            "planId": target.plan_id,
            "pointIndex": target.point_index,
            "reason": "证据图已推送企业微信",
        }),
    )
    .await;
    if sent {
        info!(
            device = %target.device_code,
            plan = %target.plan_id,
            point = target.point_index,
            "wecom: 证据图已进群，通知客户端清理板端存证"
        );
    } else {
        debug!(
            device = %target.device_code,
            plan = %target.plan_id,
            point = target.point_index,
            "wecom: 证据图已进群，但设备不在线，板端存证留在原地"
        );
    }
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
    ctx: &NotifyCtx,
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
    .execute(&ctx.db)
    .await;
    if let Err(error) = result {
        warn!("wecom: 写推送记录失败: {error}");
    }
}

fn alerts_link(ctx: &NotifyCtx) -> Option<String> {
    ctx.config
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

fn work_orders_link(ctx: &NotifyCtx, order_id: Option<i64>) -> Option<String> {
    ctx.config.public_url.as_deref().map(|base| match order_id {
        Some(id) => format!("{base}/workorders?focus={id}"),
        None => format!("{base}/workorders"),
    })
}

fn tasks_link(ctx: &NotifyCtx, task_id: Option<i64>) -> Option<String> {
    ctx.config.public_url.as_deref().map(|base| match task_id {
        Some(id) => format!("{base}/tasks?focus={id}"),
        None => format!("{base}/tasks"),
    })
}

/// 两个开关都为真才推。
///
/// 缺行视为开启：新增开关项在旧库上还没播种时，不应该把告警静默掉。
async fn channel_enabled(ctx: &NotifyCtx, channel: Channel) -> bool {
    let [outer, inner] = channel.settings_keys();
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT setting_key, setting_value FROM settings WHERE setting_key IN (?1, ?2)",
    )
    .bind(outer)
    .bind(inner)
    .fetch_all(&ctx.db)
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

/// 跟单告警（同一问题已经挂在在办工单上）是否照样推企业微信。
///
/// 缺行视为**开启**：一次任务执行就是一次告警，默认要让群里看到；
/// 只有明确关掉（`wecom_alert_repeat_push = false`）才退回
/// 「24 小时内同一问题只推第一条」的老行为。
async fn repeat_push_enabled(ctx: &NotifyCtx) -> bool {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT setting_value FROM settings WHERE setting_key = 'wecom_alert_repeat_push'",
    )
    .fetch_optional(&ctx.db)
    .await
    .unwrap_or(None);
    match row {
        Some((Some(value),)) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "off"
        ),
        _ => true,
    }
}

/// 告警证据图（`msgtype=image`）是否推到企业微信。
///
/// 缺行视为**开启**：站点拿到图才是真需要的（「AI 说数量不对，到底几个？」）；
/// 嫌图多费流量/刷屏时在设置页关掉，卡片照旧发。
async fn image_push_enabled(ctx: &NotifyCtx) -> bool {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT setting_value FROM settings WHERE setting_key = 'wecom_alert_image'",
    )
    .fetch_optional(&ctx.db)
    .await
    .unwrap_or(None);
    match row {
        Some((Some(value),)) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "off"
        ),
        _ => true,
    }
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
    /// 结构化去重键（`ai:` / `fault:` / `hb:` / `ctrl:`）：
    /// 用它区分「AI 命中（每一轮都是新信息）」和「设备故障连刷」两种推送节奏。
    dedup_key: Option<String>,
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
/// 第一道门是 [`crate::alert_flow::should_dispatch`]：**只有执行计划期间**的告警
/// （告警绑定的设备当时有 `执行中` 的任务）才会走到下面两步。掉线 / 控制权抢占 /
/// 手动新建这类只落库，告警中心能看到，但不建单也不推群（`alert_only_in_task`
/// 关掉时退回老口径：所有告警都建单推群）。
///
/// 剩下的顺序不能颠倒：
///
/// 1. **派单 + 建工单**（[`crate::alert_flow::on_alert`]）放在最前面，且**不看推送开关**。
///    现场很可能根本没配 webhook，但「告警要有人管」不能依赖群消息通道。
/// 2. 推送才是旁路，失败只打日志。
///
/// 消息里因此能直接带「当班责任人 / 工单号」，并把责任人手机号塞进
/// `mentioned_mobile_list`（markdown 不支持 @，只能另发一条 text）。
///
/// 带证据图的告警（AI 识别检出 / 数量核对不一致）用 [`alert_created_with_image`]，
/// 多推一条 `msgtype=image`；其余来源没有图片字节，走这个无图版本。
pub async fn alert_created(ctx: &NotifyCtx, alert_code: &str) {
    alert_created_with_image(ctx, alert_code, None).await;
}

/// 同 [`alert_created`]，额外把证据图作为**第二条**消息发出去。
///
/// 图片只在 `wecom_alert_image` 开着（缺行 = 开）时附上；图片发送失败**不影响**
/// 正文那条：卡片已经进群了，不能因为一张图把整条告警判失败。
pub async fn alert_created_with_image(
    ctx: &NotifyCtx,
    alert_code: &str,
    image: Option<NoticeImage>,
) {
    if !crate::alert_flow::should_dispatch(&ctx.db, alert_code).await {
        debug!(
            alert = %alert_code,
            "告警不在任务执行期间：跳过自动建单与企业微信推送（告警本身照常落库）"
        );
        return;
    }
    let assignment = crate::alert_flow::on_alert(&ctx.db, alert_code).await;

    // 「跟单」= 同一件事已经挂在在办工单上（见 `alert_flow::find_open_order`）。
    //
    // 默认**照样推**：一次任务执行就是一次告警，现场要的是「每轮巡检都在群里出现
    // 一条」，而不是「24 小时内同一航点只出现一次」。2026-11-17 之后的老口径是
    // 静默，只有数量核对的检出数变了才补推，导致同一条周期计划第二天再报同一处
    // 隐患时群里什么都没有，现场以为「巡检没跑 / 没告警」。
    //
    // 嫌吵的部署可以在设置页关掉 `wecom_alert_repeat_push`（缺行视为开启）。
    // 另外跟单消息共用 `alert-repeat:<工单>` 这个去重键，5 分钟窗口内的连刷
    // 仍然会合并成一条（机器人每 90s 报一次 12608 时不会把群刷爆）。
    //
    // ⚠️ **AI 命中不吃这个 5 分钟窗口**（`ai:` 键，见下面的 `ai_hit`）：
    // 一轮巡检就是一次结论，同一航点上一轮命中、下一轮又命中是**新信息**，
    // 跟设备故障 90s 连刷不是一回事。2026-11-19 现场踩过：取消任务后 4 分钟
    // 内重跑，AL-879/880/881 三条 AI 告警全被上一轮的同工单推送（246/243/242s
    // 前）合并掉了，群里一条都没有，看着就是「重跑后 AI 没上报」。
    let follow_up = assignment
        .as_ref()
        .and_then(|assignment| assignment.follow_up);
    let repeat = assignment
        .as_ref()
        .is_some_and(|assignment| assignment.repeated)
        && follow_up.is_none();

    if !ctx.notifier.is_enabled() {
        return;
    }
    if !channel_enabled(ctx, Channel::Alert).await {
        return;
    }
    // 通道都不通时不做这两次查询：开关判断放在「能不能发」之后
    if repeat && !repeat_push_enabled(ctx).await {
        debug!(alert = %alert_code, "wecom: 同类告警已有在办工单且关闭了重复推送，跳过");
        return;
    }
    // 第几次报警：跟着工单数一遍，群里那句「同一问题第 N 次」才有依据
    let repeat_count = match (repeat, assignment.as_ref().and_then(|a| a.work_order_id)) {
        (true, Some(order_id)) => {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alerts WHERE work_order_id = ?1")
                .bind(order_id)
                .fetch_one(&ctx.db)
                .await
                .ok()
        }
        _ => None,
    };
    let row: Option<AlertLookup> = sqlx::query_as(
        "SELECT a.id, a.alert_code, a.level, a.content, a.occurred_at, a.dedup_key,
                    d.device_id AS device_code, p.park_name, z.zone_name
               FROM alerts a
               LEFT JOIN devices d ON d.id = a.device_id
               LEFT JOIN parks p ON p.id = COALESCE(a.park_id, d.park_id)
               LEFT JOIN zones z ON z.id = d.zone_id
              WHERE a.alert_code = ?1",
    )
    .bind(alert_code)
    .fetch_optional(&ctx.db)
    .await
    .unwrap_or(None);
    let Some(row) = row else {
        debug!("wecom: 告警 {alert_code} 回查不到，跳过推送");
        return;
    };

    // AI 命中 / 数量核对的告警不走「按工单」的 5 分钟去重（见上面的注释）。
    let ai_hit = row
        .dedup_key
        .as_deref()
        .is_some_and(|key| key.starts_with("ai:"));

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
            // 跟单消息要说清「挂的是原单」，否则群里会以为又新建了一张
            let suffix = if follow_up.is_some() || repeat {
                "（原工单，未新建）"
            } else {
                "（已自动派单）"
            };
            fields.push(("工单", format!("{order}{suffix}")));
        }
    }
    if let Some(change) = follow_up {
        fields.push((
            "数量变化",
            format!(
                "上一次检出 {} 个 → 本次 {} 个（同一问题继续挂在原工单）",
                change.previous, change.current
            ),
        ));
    }
    fields.push(("时间", row.occurred_at));
    fields.push(("告警码", row.alert_code.clone()));

    // 有工单就链到工单（责任人点进去就能回填处置记录），否则退回告警中心
    let link = assignment
        .as_ref()
        .and_then(|assignment| assignment.work_order_id)
        .and_then(|order_id| work_orders_link(ctx, Some(order_id)))
        .or_else(|| alerts_link(ctx));

    let mut notice = Notice {
        level: Level::parse(&row.level),
        ref_type: Channel::Alert.ref_type(),
        ref_id: Some(row.id),
        // 跟单共用「按工单」的键：5 分钟内的连刷合并，跨轮次（分钟级以上）照推。
        //
        // AI 命中反着来：**按结构化键 + 1 分钟**（`AI_HIT_DEDUPE_WINDOW`）。
        // 一轮巡检就是一次结论，按工单合并 5 分钟只会把下一轮的真实检出吞掉
        // （2026-11-19 AL-879/880/881）；但同一窗口抖动/重开时那几秒重复上报
        // 仍要合并，所以用 `ai:<设备>:<任务>:<航点>:<类别>` + 60s。
        dedupe_key: if ai_hit {
            row.dedup_key
                .clone()
                .unwrap_or_else(|| format!("alert:{}", row.alert_code))
        } else if repeat || follow_up.is_some() {
            match assignment
                .as_ref()
                .and_then(|assignment| assignment.work_order_id)
            {
                Some(order_id) => format!("alert-repeat:{order_id}"),
                None => format!("alert-repeat:{}", row.alert_code),
            }
        } else {
            format!("alert:{}", row.alert_code)
        },
        dedupe_window: ai_hit.then_some(AI_HIT_DEDUPE_WINDOW),
        title: if follow_up.is_some() {
            "设备异常告警（跟单：数值有变化）".to_string()
        } else if repeat {
            match repeat_count {
                Some(count) => format!("设备异常告警（跟单：同一问题第 {count} 次）"),
                None => "设备异常告警（跟单：同一问题仍在办）".to_string(),
            }
        } else if assignment
            .as_ref()
            .is_some_and(|assignment| assignment.work_order_code.is_some())
        {
            "设备异常告警（已自动派单）".to_string()
        } else {
            "设备异常告警".to_string()
        },
        fields,
        // 群里不带板端证据路径（见 [`without_evidence_path`]）；告警正文里照旧保留
        body: Some(without_evidence_path(&row.content)),
        action: follow_up
            .map(|change| {
                format!(
                    "同一问题的数量有变化：上次检出 {} 个，本次 {} 个；已挂到原工单，\
                     不另建单。请责任人到场按最新情况处置并把结果回填到工单",
                    change.previous, change.current
                )
            })
            .or_else(|| {
                assignment.as_ref().map(|assignment| {
                    if repeat {
                        // 重复告警不再说「已自动派单」：工单早就建好了，这里要说清
                        // 「没新建单」，否则值班长会去工单列表找根本不存在的第二张单
                        format!(
                            "同一问题仍在办（{}），本次不新建工单，继续挂在原单上。\
                             请责任人按最新情况处置并把结果回填到工单",
                            repeat_count
                                .map(|count| format!("第 {count} 次告警"))
                                .unwrap_or_else(|| "重复告警".to_string())
                        )
                    } else {
                        assignment.dispatch_sentence()
                    }
                })
            }),
        link,
        mention_all: false,
        // 跟单不再 @ 人：一次任务里的重复告警每条都 @ 一遍等于骚扰，
        // 人已经在第一条消息里被通知过了（数量变化那条仍 @，因为它带新信息）
        mentions: if repeat {
            Vec::new()
        } else {
            assignment
                .as_ref()
                .and_then(|assignment| assignment.phone.as_deref())
                .and_then(mentionable)
                .map(|phone| vec![phone])
                .unwrap_or_default()
        },
        image: None,
        image_only: false,
        cleanup: None,
        bypass_min_level: false,
    };
    // 证据图是**可选附加**：开关关掉 / 拿不到字节就只发卡片
    if let Some(image) = image {
        if image_push_enabled(ctx).await {
            notice.image = Some(image);
            // 图马上就要进群：板端这一轮存证（画框 JPG + result.json）之后没有第二次
            // 用途，推送成功后让客户端删掉（见 [`dispatch_evidence_cleanup`]）。
            // 没带图 / 图被开关挡掉就不删——文件留在板子上，还能人工补推。
            notice.cleanup = evidence_cleanup(row.dedup_key.as_deref(), row.device_code.as_deref());
        } else {
            debug!(alert = %alert_code, "wecom: 已关闭告警证据图推送，只发卡片");
        }
    }
    ctx.notifier.enqueue(notice);
}

/// 给**已经推过的**告警补一张证据图（只发 `msgtype=image`，不重发卡片）。
///
/// 为什么需要：板端的命中帧不是检出那一刻就落盘的。正常情况客户端会等 `plan_hit`
/// 把图和告警一起上报；但如果板端「连采 3 帧命中」的采样一直没跑完，图要到窗口收尾
/// （`/api/detect/stop` → `_plan_flush`）才写出来——那时告警早就发出去了（不能为了等图
/// 把告警卡住）。于是客户端停识别、扫到图之后再单独报一次，这里只补发图片。
///
/// 企业微信本来就把图片当**独立一条**消息发（见 `deliver`），所以群里看到的效果与
/// 「卡片 + 图」完全一样，只是第二条晚了几十秒（2026-11-18 19:35 现场：
/// 检出 `Trash over flow` 0.29 只命中一帧，采样没跑完，图 38s 后才落盘）。
///
/// 开关与门禁与 [`alert_created_with_image`] 一致：关图 / 不在执行计划期间 / 通道关
/// 都直接不发（卡片已经发了，图片只是辅证）。
pub async fn alert_evidence(ctx: &NotifyCtx, alert_code: &str, image: NoticeImage) {
    if !ctx.notifier.is_enabled() {
        return;
    }
    if !channel_enabled(ctx, Channel::Alert).await {
        return;
    }
    if !image_push_enabled(ctx).await {
        debug!(alert = %alert_code, "wecom: 已关闭告警证据图推送，不补发图片");
        return;
    }
    // 图跟着卡片走同一道门：卡片没推（不在执行计划期间）就不该单独冒出一张图
    if !crate::alert_flow::should_dispatch(&ctx.db, alert_code).await {
        debug!(alert = %alert_code, "wecom: 告警不在任务执行期间，补发图片也跳过");
        return;
    }
    // 级别要与原卡片一致：`enqueue` 会按 `min_level` 过滤，图片不能用别的级别
    //
    // 顺带把结构化键与设备取出来：图进群之后要通知客户端删板端存证（见 [`EvidenceCleanup`]）
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT a.level, a.dedup_key, d.device_id
           FROM alerts a
           LEFT JOIN devices d ON d.id = a.device_id
          WHERE a.alert_code = ?1",
    )
    .bind(alert_code)
    .fetch_optional(&ctx.db)
    .await
    .ok()
    .flatten();
    let Some((level, dedup_key, device_code)) = row else {
        debug!(alert = %alert_code, "wecom: 回查不到告警级别，跳过补发图片");
        return;
    };
    ctx.notifier.enqueue(Notice {
        level: Level::parse(&level),
        ref_type: Channel::Alert.ref_type(),
        ref_id: None,
        // 与卡片分开去重：同一张图重复上报时只发一次
        dedupe_key: format!("alert-image:{alert_code}"),
        dedupe_window: None,
        title: String::new(),
        fields: Vec::new(),
        body: None,
        action: None,
        link: None,
        mention_all: false,
        mentions: Vec::new(),
        image: Some(image),
        image_only: true,
        // 晚到的图马上要进群：板端存证可以清了（客户端会等窗口收尾再动手）
        cleanup: evidence_cleanup(dedup_key.as_deref(), device_code.as_deref()),
        bypass_min_level: false,
    });
}

/// 一条「数量核对一致」通知的内容。
///
/// 故意没有 `alert_code`：这类消息**不是告警**，不进告警中心、不派单、不建工单，
/// 只往企业微信群里发一条回执（见 [`count_matched`]）。
///
/// 为什么要有它：`count` 任务默认只有「不一致」才出声（见 `ai_patrol::record_count_alert`），
/// 但有的现场想把「这一轮核对上了」当巡检回执留在群里——动作字段
/// `notifyWhenCountOk` 就是这个开关（按航点开，默认关）。
#[derive(Debug, Clone)]
pub struct CountMatched<'a> {
    pub device_code: &'a str,
    pub task_code: &'a str,
    pub task_name: &'a str,
    pub point_index: i64,
    pub point_name: &'a str,
    /// 动作里的「检查目标数量」
    pub expect: i64,
    /// 板端 AI 实际检出的目标数（一致时等于 `expect`）
    pub found: i64,
    /// 与任务时间线同一句正文
    pub content: &'a str,
    /// 证据图（客户端读板端存证目录后带上）；`None` = 只发卡片
    pub image: Option<NoticeImage>,
}

/// 「数量核对一致」→ **只推企业微信群，不落告警、不派单、不建工单**。
///
/// 与 [`alert_created`] 是两条路，不能合并：那条的第一件事就是
/// `alert_flow::on_alert`（建工单）。这里是巡检**正常**的结论，工单列表里多一条
/// 「检查 3 个、检出 3 个」只会让真正要处置的单淹掉。
///
/// 门禁与告警共用（通道开关 `alert_push` / `wecom_alert_push`、图片开关
/// `wecom_alert_image`），但不走 `alert_only_in_task`：能走到这里的只有
/// 「正在执行计划、到点开窗」的那条链路，天然就在执行计划期间。
pub async fn count_matched(ctx: &NotifyCtx, matched: CountMatched<'_>) {
    if !ctx.notifier.is_enabled() {
        return;
    }
    // 关掉「企业微信告警推送」的部署不会想收到巡检回执：与告警共用同一对开关
    if !channel_enabled(ctx, Channel::Alert).await {
        return;
    }
    let park: Option<String> = sqlx::query_scalar(
        "SELECT p.park_name
           FROM devices d
           LEFT JOIN parks p ON p.id = d.park_id
          WHERE d.device_id = ?1",
    )
    .bind(matched.device_code)
    .fetch_optional(&ctx.db)
    .await
    .ok()
    .flatten();
    let mut notice = Notice {
        level: Level::Info,
        // 新来源：推送记录里能看出「这是数量核对回执」，不是告警
        ref_type: "count",
        // 没有告警行可以引用（本来就是不落库的通知）
        ref_id: None,
        // 一轮巡检就是一条回执：同一窗口重复上报合并 1 分钟，下一轮照发
        dedupe_key: format!(
            "count-ok:{}:{}:{}",
            matched.device_code, matched.task_code, matched.point_index
        ),
        dedupe_window: Some(AI_HIT_DEDUPE_WINDOW),
        title: "AI 数量核对一致".to_string(),
        fields: vec![
            ("设备", matched.device_code.to_string()),
            ("园区", park.unwrap_or_else(|| "未绑定园区".to_string())),
            (
                "计划",
                format!("{}（{}）", matched.task_name, matched.task_code),
            ),
            ("航点", matched.point_name.to_string()),
            (
                "核对",
                format!(
                    "检查 {} 个，实际检出 {} 个（一致）",
                    matched.expect, matched.found
                ),
            ),
            ("时间", now()),
        ],
        body: Some(without_evidence_path(matched.content)),
        // 正常结论：不派单、不建议处置，只说清楚结论
        action: Some("数量核对一致，无需处置；本条仅为巡检回执，不生成告警与工单".to_string()),
        link: None,
        mention_all: false,
        mentions: Vec::new(),
        image: None,
        image_only: false,
        // 存证由「计划跑完删整个目录」那条统一收（`ai_cleanup` 只认告警去重键）
        cleanup: None,
        // 级别是「提示」，而默认 `wecom_min_level=警告`：运维在航点上显式勾了
        // 「数量一致也通知」，就不该被全局级别过滤静默掉（否则表现为「配了却没发」）
        bypass_min_level: true,
    };
    if let Some(image) = matched.image
        && image_push_enabled(ctx).await
    {
        notice.image = Some(image);
    }
    ctx.notifier.enqueue(notice);
}

/// 任务被判「异常」时调用。看门狗与客户端上报终态两处都走这里，靠去重键合并。
pub async fn task_abnormal(ctx: &NotifyCtx, task_code: &str, level: Level, reason: &str) {
    if !ctx.notifier.is_enabled() {
        return;
    }
    if !channel_enabled(ctx, Channel::Task).await {
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
    .fetch_optional(&ctx.db)
    .await
    .unwrap_or(None);
    let Some(row) = row else {
        debug!("wecom: 任务 {task_code} 回查不到，跳过推送");
        return;
    };

    // 计划层的责任人：任务异常同样要 @ 到人，否则群里只是「又一条报错」
    let assignment =
        crate::alert_flow::responsible_for_task(&ctx.db, row.park_id, Some(row.id)).await;

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
        dedupe_window: None,
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
        link: tasks_link(ctx, Some(row.id)),
        mention_all: level == Level::Critical,
        mentions: assignment
            .as_ref()
            .and_then(|assignment| assignment.phone.as_deref())
            .and_then(mentionable)
            .map(|phone| vec![phone])
            .unwrap_or_default(),
        image: None,
        image_only: false,
        // 任务异常没有板端存证要清（AI 存证只跟「带图的告警」走）
        cleanup: None,
        bypass_min_level: false,
    };
    ctx.notifier.enqueue(notice);
}

/// 设置页「发送测试消息」：**同步**发一条并返回企业微信的原始结果。
///
/// 走同步是因为运维需要立刻看到 `errcode`（配错 key 时群里的静默失败最难查）。
pub async fn send_test(ctx: &NotifyCtx) -> Result<String, String> {
    if !ctx.config.wecom_enabled {
        return Err("推送通道已在配置中关闭（wecom.enabled = false）".to_string());
    }
    let Some(url) = ctx.notifier.webhook() else {
        return Err(
            "未配置 webhook：请在「系统设置 → 企业微信群机器人」填写地址，\
             或由部署方设置环境变量 ROBODOG_WECOM_WEBHOOK"
                .to_string(),
        );
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| format!("构建 HTTP 客户端失败: {error}"))?;
    let notice = Notice {
        level: Level::Info,
        ref_type: "test",
        ref_id: None,
        dedupe_key: format!("test:{}", now()),
        dedupe_window: None,
        title: "推送通道测试".to_string(),
        fields: vec![
            ("平台", ctx.config.public_url.clone().unwrap_or_default()),
            ("时间", now()),
        ],
        body: Some("这是一条手动触发的测试消息，收到即表示企业微信告警推送可用".to_string()),
        action: None,
        link: None,
        mention_all: false,
        mentions: Vec::new(),
        image: None,
        image_only: false,
        cleanup: None,
        bypass_min_level: false,
    };
    let payload = notice.markdown();
    let body = json!({ "msgtype": "markdown", "markdown": { "content": payload } });
    match post(&client, &url, &body).await {
        Ok(()) => {
            record(ctx, &notice, &payload, "sent", 1, None).await;
            info!("wecom: 测试消息发送成功");
            Ok("测试消息已发送，请检查企业微信群".to_string())
        }
        Err(failure) => {
            record(ctx, &notice, &payload, "failed", 1, Some(&failure.message)).await;
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
            dedupe_window: None,
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
            image: None,
            image_only: false,
            cleanup: None,
            bypass_min_level: false,
        }
    }

    /// 证据图 → `msgtype=image` 的三个硬约束：base64 合法、≤ 2MB、JPG/PNG，
    /// 以及 md5 算的是 **base64 之前** 的字节（算错企业微信直接拒）
    #[test]
    fn evidence_image_validates_and_computes_md5() {
        // 最小 JPEG（只看文件头）：FF D8 FF …
        const JPEG: &str = "/9j/4AAQSkZJRgAB";
        let image = evidence_image(JPEG).expect("合法 JPEG 应该可用");
        assert_eq!(
            image.md5, "cbde0fdf7fab6fefd9fe12fec4967cf9",
            "md5 必须算原图字节，不是 base64 文本"
        );
        assert_eq!(image.bytes, 12);
        assert_eq!(image.base64, JPEG, "重新编码后应完全一致（无换行）");

        // 带换行 / 空格也要能吃下去（现场 `base64` 命令默认 76 列换行）
        let wrapped = "/9j/4AAQ\nSkZJRgAB\n";
        assert_eq!(evidence_image(wrapped).expect("应忽略空白").base64, JPEG);

        // PNG 文件头 `89 50 4E 47`
        assert!(evidence_image("iVBORw0KGgo=").is_ok());

        assert!(evidence_image("").is_err(), "空值不能用");
        assert!(evidence_image("这不是 base64").is_err());
        // 合法 base64 但不是我要求的两种图片格式
        assert!(evidence_image("aGVsbG8=").is_err(), "hello 不是图片");
    }

    /// AI 告警的去重键能反解出「删哪个航点的存证」；非 AI 告警一律不删。
    ///
    /// 删目录不可逆，所以键不完整（老数据没有 `ai:` 前缀 / 航点下标不是数字）时
    /// 一律返回 `None`，宁可留一堆图，也不要删错。
    #[test]
    fn evidence_cleanup_parses_only_ai_keys() {
        let target = evidence_cleanup(Some("ai:RD-013:TK-1112-11:2:extinguisher"), Some("RD-999"))
            .expect("AI 键应该能反解");
        assert_eq!(
            target.device_code, "RD-013",
            "设备以键里报上来的那个为准（报警的客户端，存证也在它的盘上）"
        );
        assert_eq!(target.plan_id, "TK-1112-11");
        assert_eq!(target.point_index, 2);

        // 键里没带设备（老数据）：退回告警回查到的设备
        assert_eq!(
            evidence_cleanup(Some("ai::TK-1:0:count"), Some("RD-013")).map(|t| t.device_code),
            Some("RD-013".to_string())
        );

        for key in [
            None,
            Some("fault:RD-013:13382"),
            Some("hb:RD-013"),
            Some("ctrl:RD-013"),
            Some("alert:AL-001"),
        ] {
            assert!(evidence_cleanup(key, Some("RD-013")).is_none(), "{key:?}");
        }
        // 键不全 / 航点下标非法 / 计划 id 为空：都不删
        assert!(evidence_cleanup(Some("ai:RD-013:TK-1"), Some("RD-013")).is_none());
        assert!(evidence_cleanup(Some("ai:RD-013:TK-1:-1:count"), Some("RD-013")).is_none());
        assert!(evidence_cleanup(Some("ai:RD-013:TK-1:x:count"), Some("RD-013")).is_none());
        assert!(evidence_cleanup(Some("ai:RD-013::0:count"), Some("RD-013")).is_none());
    }

    /// 超过 2MB 的图不能发（企业微信整条拒掉，还不如不发图）
    #[test]
    fn evidence_image_rejects_oversized() {
        let mut bytes = vec![0xFF_u8, 0xD8, 0xFF];
        bytes.resize(IMAGE_MAX_BYTES + 1, 0);
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let error = evidence_image(&encoded).expect_err("2MB 以上应被拒");
        assert!(error.contains("2MB"), "错误要说清是体积问题: {error}");
    }

    /// 群里不带板端证据路径：图作为 `msgtype=image` 紧接着卡片发，路径一串
    /// `/root/.../detect_jobs/...` 只是噪音（告警正文里照旧保留，见调用点）。
    #[test]
    fn evidence_path_is_dropped_from_the_card_body() {
        let body = "AI 识别数量核对不一致：任务「TK-1」航点「垃圾桶」检查 3 个，实际检出 2 个（证据：/root/detect_jobs/TK-1/pt0/dogcam_1.jpg）";
        let stripped = without_evidence_path(body);
        assert_eq!(
            stripped, "AI 识别数量核对不一致：任务「TK-1」航点「垃圾桶」检查 3 个，实际检出 2 个",
            "只去掉（证据：…），前面的结论一个字都不能少"
        );

        let mut item = notice(Level::Warning);
        item.body = Some(stripped);
        let text = item.markdown();
        assert!(!text.contains("证据"), "{text}");
        assert!(!text.contains("dogcam_1.jpg"), "{text}");

        // 没有路径的正文原样返回（不能把正文搞坏）
        assert_eq!(
            without_evidence_path("导航指令非法（13440）"),
            "导航指令非法（13440）"
        );
        // 多个 / 半角冒号 / 只有开头没结尾：都不留半截路径
        assert_eq!(
            without_evidence_path("检出 A（证据:/a.jpg）（证据：/b.jpg）"),
            "检出 A"
        );
        assert_eq!(without_evidence_path("检出 A（证据：/a.jpg"), "检出 A");
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
        assert!(dedupe.accept("alert:AL-1", start).is_ok());
        let blocked = dedupe.accept("alert:AL-1", start + Duration::from_secs(10));
        assert_eq!(
            blocked,
            Err(Duration::from_secs(290)),
            "被挡时要告诉调用方还剩多少秒（这行会进生产日志）"
        );
        // 窗口外重新放行
        assert!(
            dedupe
                .accept("alert:AL-1", start + Duration::from_secs(400))
                .is_ok()
        );
        // 不同键互不影响
        assert!(dedupe.accept("alert:AL-2", start).is_ok());
    }

    /// AI 命中用 1 分钟窗口：**几分钟后的下一轮必须放行**（2026-11-19 现场：
    /// 取消任务后 246s 重跑，三条 AI 告警被上一轮 5 分钟的跟单键合并掉）。
    /// 同时又不能放开到「窗口抖动几秒一条」。
    #[test]
    fn ai_hit_window_lets_the_next_round_through() {
        let mut dedupe = Dedupe::new(Duration::from_secs(300));
        let start = Instant::now();
        let key = "ai:0202222672400230:TK-1112-11:0:extinguisher";

        // 上一轮 11:05:38 推过
        assert!(
            dedupe
                .accept_within(key, start, AI_HIT_DEDUPE_WINDOW)
                .is_ok()
        );
        // 同一窗口抖动/重开识别时几秒内重复上报 → 合并
        assert_eq!(
            dedupe.accept_within(key, start + Duration::from_secs(10), AI_HIT_DEDUPE_WINDOW),
            Err(Duration::from_secs(50))
        );
        // 下一轮 246s 后（现场那次取消→重跑）→ 必须放行
        assert!(
            dedupe
                .accept_within(key, start + Duration::from_secs(246), AI_HIT_DEDUPE_WINDOW)
                .is_ok(),
            "1 分钟之外的下一轮巡检必须重新进群"
        );
        // 反例：如果按 5 分钟窗口（旧的跟单键行为），246s 那条会被吞掉
        assert!(
            dedupe
                .accept_within(key, start + Duration::from_secs(246), DEDUPE_WINDOW)
                .is_err(),
            "旧的 5 分钟窗口正是现场丢告警的原因"
        );
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
        let mut config = Config::from_env();
        config.wecom_enabled = true;
        config.wecom_webhook = None;
        let (notifier, rx) = Notifier::channel(&config, None);
        // 一个地址都没有 → 句柄不发消息（`is_enabled` 会把告警路径直接短路）
        assert!(!notifier.is_enabled());
        assert!(!notifier.enqueue(notice(Level::Critical)));
        // 消费端仍会启动：现场在设置页填完地址就应该生效，不必重启进程
        assert!(rx.is_some());
        assert!(notifier.webhook().is_none());
    }

    fn test_config() -> Config {
        let mut config = Config::from_env();
        config.wecom_enabled = true;
        config.wecom_webhook = None;
        config
    }

    #[test]
    fn platform_webhook_wins_over_env() {
        let mut config = test_config();
        config.wecom_webhook =
            Some("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=env".into());
        let (notifier, _rx) = Notifier::channel(
            &config,
            Some("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=platform".into()),
        );
        assert!(notifier.is_enabled());
        assert_eq!(notifier.webhook_snapshot().1, Some(WebhookSource::Platform));
        assert!(notifier.webhook().unwrap().contains("key=platform"));
    }

    #[test]
    fn webhook_falls_back_to_env_and_hot_reloads() {
        let mut config = test_config();
        config.wecom_webhook = Some("https://hook.example.com/send?key=envkey".into());
        let (notifier, _rx) = Notifier::channel(&config, Some("   ".into()));
        // 平台侧是空串（页面上清空了）→ 回退环境变量
        assert_eq!(notifier.webhook_snapshot().1, Some(WebhookSource::Env));

        // 页面保存后热更新：不重启进程，下一次发送就用新地址
        notifier.set_webhook(
            Some("https://hook.example.com/send?key=new".into()),
            Some(WebhookSource::Platform),
        );
        assert!(notifier.webhook().unwrap().contains("key=new"));

        // 清空 → 立刻停发（`is_enabled` 变假，告警链路不再拼消息）
        notifier.set_webhook(None, None);
        assert!(!notifier.is_enabled());
        assert!(!notifier.enqueue(notice(Level::Critical)));
    }

    #[test]
    fn validate_webhook_rejects_obviously_wrong_input() {
        let ok = validate_webhook(
            "  https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=94fdb35e-1111  ",
        );
        assert_eq!(
            ok.unwrap(),
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=94fdb35e-1111"
        );
        assert!(validate_webhook("").is_err());
        assert!(
            validate_webhook("qyapi.weixin.qq.com/send?key=x").is_err(),
            "缺协议"
        );
        assert!(
            validate_webhook("https://qyapi.weixin.qq.com/cgi-bin/webhook/send").is_err(),
            "缺 key= 时发出去只会拿到 93000"
        );
        assert!(
            validate_webhook("https://qyapi.weixin.qq.com/send?key=a b").is_err(),
            "复制时带进了空格"
        );
    }

    #[test]
    fn mask_webhook_never_echoes_the_full_key() {
        let masked =
            mask_webhook("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=94fdb35e-1234");
        assert!(masked.contains("key=94fdb35e…"), "{masked}");
        assert!(!masked.contains("1234"), "只回显前 8 位: {masked}");
        assert_eq!(key_tail("send?key=ab12cd"), "ab12cd");
    }
}
