//! 「谁该收到什么」的业务判断
//!
//! 从原来的单文件 `notify.rs`（2278 行）里拆出来：告警要不要建单推群、@谁、带不带证据图，以及每一类消息的文案。

// 同一模块拆出来的几块共用一个词汇表（`Notice` / `Level` / `NotifyCtx` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;
use crate::db::now;
use serde_json::json;
use std::time::Duration;
use tracing::{debug, info};

pub(crate) fn alerts_link(ctx: &NotifyCtx) -> Option<String> {
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
pub(crate) fn mentionable(phone: &str) -> Option<String> {
    let phone = phone.trim();
    let ok = phone.len() >= 5
        && !phone.contains('*')
        && phone
            .chars()
            .all(|ch| ch.is_ascii_digit() || ch == '+' || ch == '-');
    ok.then(|| phone.to_string())
}

pub(crate) fn work_orders_link(ctx: &NotifyCtx, order_id: Option<i64>) -> Option<String> {
    ctx.config.public_url.as_deref().map(|base| match order_id {
        Some(id) => format!("{base}/workorders?focus={id}"),
        None => format!("{base}/workorders"),
    })
}

pub(crate) fn tasks_link(ctx: &NotifyCtx, task_id: Option<i64>) -> Option<String> {
    ctx.config.public_url.as_deref().map(|base| match task_id {
        Some(id) => format!("{base}/tasks?focus={id}"),
        None => format!("{base}/tasks"),
    })
}

/// 两个开关都为真才推。
///
/// 缺行视为开启：新增开关项在旧库上还没播种时，不应该把告警静默掉。
pub(crate) async fn channel_enabled(ctx: &NotifyCtx, channel: Channel) -> bool {
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
pub(crate) async fn repeat_push_enabled(ctx: &NotifyCtx) -> bool {
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
pub(crate) async fn image_push_enabled(ctx: &NotifyCtx) -> bool {
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
pub(crate) struct AlertLookup {
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
pub(crate) struct TaskLookup {
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
    // 低电量（`battery:`）在群里换个标题：值班的人扫一眼就知道是「该去充电了」，
    // 不用点开卡片看正文（早上群里可能还挂着别的故障）。
    let kind = if row
        .dedup_key
        .as_deref()
        .is_some_and(|key| key.starts_with("battery:"))
    {
        "设备低电量告警"
    } else {
        "设备异常告警"
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
            format!("{kind}（跟单：数值有变化）")
        } else if repeat {
            match repeat_count {
                Some(count) => format!("{kind}（跟单：同一问题第 {count} 次）"),
                None => format!("{kind}（跟单：同一问题仍在办）"),
            }
        } else if assignment
            .as_ref()
            .is_some_and(|assignment| assignment.work_order_code.is_some())
        {
            format!("{kind}（已自动派单）")
        } else {
            kind.to_string()
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
