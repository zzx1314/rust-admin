//! 发送管线
//!
//! 从原来的单文件 `notify.rs`（2278 行）里拆出来：取额度 → POST → 重试 → 落 `notifications` → 证据图 → 通知板端清理存证。

// 同一模块拆出来的几块共用一个词汇表（`Notice` / `Level` / `NotifyCtx` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;
use crate::db::now;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// 队列满丢掉的条数汇总成一条，避免"静默丢消息"。
///
/// 走**同一把** `gate`：汇总也是一条企业微信消息，另建限流器会让它绕开额度。
pub(crate) async fn flush_overflow(ctx: &NotifyCtx, client: &reqwest::Client, gate: &mut RateGate) {
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
pub(crate) async fn deliver(
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

pub(crate) enum FailureKind {
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
pub(crate) async fn send_image(
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
pub(crate) async fn dispatch_evidence_cleanup(ctx: &NotifyCtx, notice: &Notice) {
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

pub(crate) struct SendFailure {
    pub(crate) kind: FailureKind,
    pub(crate) message: String,
}

pub(crate) async fn post(
    client: &reqwest::Client,
    url: &str,
    body: &Value,
) -> Result<(), SendFailure> {
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

pub(crate) async fn record(
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
