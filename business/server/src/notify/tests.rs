//! 推送层的单元测试
//!
//! 从原来的单文件 `notify.rs`（2278 行）里拆出来：覆盖去重 / 限流 / 渲染 / 校验这些纯逻辑。
//!
//! 这里直接 glob 兄弟模块（而不是只 `use super::*`）：测的就是模块内部的私有函数
//! （`mentionable`、`RateGate::allow`、`without_evidence_path` …），
//! 走 `notify::` 的对外 API 反而看不到它们。

#[allow(clippy::wildcard_imports)]
use crate::notify::{model::*, policy::*, throttle::*, webhook::*};

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
    config.wecom_webhook = Some("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=env".into());
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
    let ok =
        validate_webhook("  https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=94fdb35e-1111  ");
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
    let masked = mask_webhook("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=94fdb35e-1234");
    assert!(masked.contains("key=94fdb35e…"), "{masked}");
    assert!(!masked.contains("1234"), "只回显前 8 位: {masked}");
    assert_eq!(key_tail("send?key=ab12cd"), "ab12cd");
}
