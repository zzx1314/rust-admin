//! 推送消息的模型与渲染
//!
//! 从原来的单文件 `notify.rs`（2278 行）里拆出来：消息长什么样、怎么拼成企业微信的 markdown / image。

// 同一模块拆出来的几块共用一个词汇表（`Notice` / `Level` / `NotifyCtx` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;
use std::time::Duration;

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

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Critical => "🔴",
            Self::Warning => "🟠",
            Self::Info => "🔵",
        }
    }

    /// 企业微信 markdown 只认这三个颜色。
    pub(crate) fn color(self) -> &'static str {
        match self {
            Self::Critical => "warning",
            Self::Warning => "comment",
            Self::Info => "info",
        }
    }
}

/// 推送的业务来源（决定取哪两个设置开关）。
#[derive(Clone, Copy)]
pub(crate) enum Channel {
    Alert,
    Task,
}

impl Channel {
    /// 两个开关都必须为 `true`：一个是运营在页面上的总开关，一个是本渠道的开关。
    pub(crate) fn settings_keys(self) -> [&'static str; 2] {
        match self {
            Self::Alert => ["alert_push", "wecom_alert_push"],
            Self::Task => ["task_timeout_notice", "wecom_task_abnormal"],
        }
    }

    pub(crate) fn ref_type(self) -> &'static str {
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
pub(crate) fn is_jpeg_or_png(bytes: &[u8]) -> bool {
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
pub(crate) fn without_evidence_path(text: &str) -> String {
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
