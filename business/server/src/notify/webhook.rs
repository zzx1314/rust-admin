//! webhook 地址（也就是凭据）的存取与校验
//!
//! 从原来的单文件 `notify.rs`（2278 行）里拆出来：地址只回掩码、日志只打末 6 位，读写都走这里。

// 同一模块拆出来的几块共用一个词汇表（`Notice` / `Level` / `NotifyCtx` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

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
pub(crate) struct WebhookTarget {
    pub(crate) url: Option<String>,
    pub(crate) source: Option<WebhookSource>,
}

impl WebhookTarget {
    /// 平台配置优先，空则回退环境变量的地址。
    pub(crate) fn resolve(platform: Option<String>, env: Option<String>) -> Self {
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
