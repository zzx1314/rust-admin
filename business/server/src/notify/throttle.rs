//! 本地限流与去重
//!
//! 从原来的单文件 `notify.rs`（2278 行）里拆出来：企业微信 20 条/分钟 + 同一件事不要连推，都在这里，发不发由 `policy` 决定。

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::debug;

/// 去重窗口：同一键在 `window` 内只放行一次。
pub(crate) struct Dedupe {
    seen: HashMap<String, Instant>,
    window: Duration,
}

/// 去重结果：放行返回 `Ok(())`，被挡返回 `Err(还要等多久)`。
///
/// 把「剩余窗口」返回给调用方，是为了能打成一行**生产可见**的 info：
/// 以前去重只打 debug，现场看到的只有「告警没进群」，无从分辨是没触发还是被合并。
pub(crate) type DedupeResult = Result<(), Duration>;

impl Dedupe {
    pub(crate) fn new(window: Duration) -> Self {
        Self {
            seen: HashMap::new(),
            window,
        }
    }

    pub(crate) fn accept(&mut self, key: &str, at: Instant) -> DedupeResult {
        self.accept_within(key, at, self.window)
    }

    /// 同上，但允许调用方指定窗口（AI 命中用 1 分钟，见 [`AI_HIT_DEDUPE_WINDOW`]）。
    ///
    /// 清理按「两者较大值」做：同一个 `seen` 表里同时存在两种窗口的键，
    /// 而它们的键前缀互斥（`alert:` / `alert-repeat:` / `task:` / `ai:` / …），
    /// 所以各键按自己那一档判定、互不影响。
    pub(crate) fn accept_within(
        &mut self,
        key: &str,
        at: Instant,
        window: Duration,
    ) -> DedupeResult {
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
pub(crate) struct RateGate {
    capacity: u32,
    window: Duration,
    used: u32,
    started: Instant,
}

impl RateGate {
    pub(crate) fn new(capacity: u32, window: Duration) -> Self {
        Self {
            capacity,
            window,
            used: 0,
            started: Instant::now(),
        }
    }

    pub(crate) fn allow(&mut self, at: Instant) -> bool {
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
    pub(crate) async fn acquire(&mut self) {
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
    pub(crate) fn wait_time(&self, at: Instant) -> Duration {
        self.window
            .saturating_sub(at.saturating_duration_since(self.started))
    }
}
