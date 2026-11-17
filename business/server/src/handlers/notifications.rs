use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::notify::{self, WebhookSource};
use axum::Json;
use axum::extract::{Query, State};
use axum::Extension;
use serde::Deserialize;
use serde_json::{Value, json};

/// 推送记录（`notifications` 表）。
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub id: i64,
    pub channel: String,
    pub ref_type: String,
    pub ref_id: Option<i64>,
    pub level: String,
    pub dedupe_key: Option<String>,
    pub payload: String,
    pub status: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub sent_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationQuery {
    pub status: Option<String>,
    pub ref_type: Option<String>,
    pub limit: Option<i64>,
}

/// 企业微信群机器人地址的当前状态（**永不含完整地址**）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookStatus {
    /// 有没有可用地址（决定告警会不会真的进群）
    pub configured: bool,
    /// 地址来源：`db`（平台设置页）/ `env`（环境变量）/ `null`
    pub source: Option<&'static str>,
    /// 掩码后的地址，只回显 key 前 8 位
    pub masked: Option<String>,
    /// 部署级总开关（`wecom.enabled`）：关掉时填了地址也不会发
    pub enabled: bool,
    /// 完整地址：**只有显式 `?reveal=true` 且是管理员**时才返回，
    /// 平时这个字段直接不出现在响应里（连 `null` 都不占位）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookQuery {
    /// 管理员在页面上点「显示」时才为真：找回之前配的地址（例如要粘到别处 / 核对）
    pub reveal: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookUpdate {
    /// 新地址；空 / 缺省 = 清除平台配置，回退到环境变量
    pub webhook: Option<String>,
}

/// 推送记录列表。
///
/// 现场排查最常见的问题是「告警推没推出去」和「为什么群里没看到」，
/// 所以失败行必须带 `last_error`（企业微信的 errcode + errmsg）。
pub async fn list_notifications(
    State(state): State<AppState>,
    Query(query): Query<NotificationQuery>,
) -> Result<Json<ApiResponse<Vec<NotificationRecord>>>, AppError> {
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT id, channel, ref_type, ref_id, level, dedupe_key, payload, status, attempts,
                last_error, created_at, sent_at
           FROM notifications WHERE 1 = 1",
    );
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND status = ").push_bind(status);
    }
    if let Some(ref_type) = query
        .ref_type
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND ref_type = ").push_bind(ref_type);
    }
    builder.push(" ORDER BY id DESC LIMIT ").push_bind(limit);
    let records = builder
        .build_query_as::<NotificationRecord>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(records))
}

/// 当前 webhook 配置（来源 + 掩码地址）。
///
/// 默认**永不返回完整地址**（地址即凭据，不该随手出现在屏幕 / 截图 / 录屏里）；
/// 管理员在设置页点「显示」时才带 `?reveal=true` 取一次原文，用于
/// 「我上次配的到底是什么」这种核对场景。
pub async fn get_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<WebhookQuery>,
) -> Result<Json<ApiResponse<WebhookStatus>>, AppError> {
    let mut status = webhook_status(&state);
    if query.reveal.unwrap_or(false) {
        if claims.role != "admin" && !claims.is_admin {
            return Err(AppError::Forbidden(
                "只有管理员可以查看 webhook 地址原文".to_string(),
            ));
        }
        status.url = state.notifier.webhook();
        // 只记「谁看了」，不记地址本身
        tracing::info!(user = %claims.username, "企业微信 webhook 原文被查看（设置页「显示」）");
    }
    Ok(ApiResponse::ok(status))
}

/// 在平台上配置 / 清除 webhook 地址。
///
/// 为什么走独立接口而不是通用 `PUT /api/settings`：
/// 地址本身就是凭据（谁拿到谁能往群里发消息），通用设置接口按原样回显明文，
/// 一进页面凭据就上了屏幕；这里读只回掩码、写做格式校验，改完热更新句柄。
pub async fn update_webhook(
    State(state): State<AppState>,
    Json(payload): Json<WebhookUpdate>,
) -> Result<Json<ApiResponse<WebhookStatus>>, AppError> {
    let trimmed = payload.webhook.as_deref().map(str::trim).unwrap_or("");
    if trimmed.is_empty() {
        // 清空平台配置 → 回退到环境变量（部署方填的那个还在的话继续用）
        save_webhook(&state, "").await?;
        match state.config.wecom_webhook.clone() {
            Some(url) if !url.trim().is_empty() => {
                state
                    .notifier
                    .set_webhook(Some(url.trim().to_string()), Some(WebhookSource::Env));
            }
            _ => state.notifier.set_webhook(None, None),
        }
        tracing::info!("企业微信 webhook 已清除（平台配置为空，按环境变量决定）");
        return Ok(ApiResponse::ok(webhook_status(&state)));
    }

    let url = notify::validate_webhook(trimmed).map_err(AppError::BadRequest)?;
    save_webhook(&state, &url).await?;
    state
        .notifier
        .set_webhook(Some(url.clone()), Some(WebhookSource::Platform));
    // 只打 key 末 6 位：能核对是哪个机器人，又不把凭据写进日志
    tracing::info!(key_tail = %notify::key_tail(&url), "企业微信 webhook 已更新（平台配置）");
    Ok(ApiResponse::ok(webhook_status(&state)))
}

/// 手动发一条测试消息，把企业微信的原始错误直接返回给设置页。
///
/// 同步发送（不走队列）是故意的：配错 key 时群里是「静默不出现」，
/// 只有拿到 `errcode` 才查得动。
pub async fn send_test(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let masked = state.notifier.webhook().as_deref().map(notify::mask_webhook);
    match notify::send_test(&state).await {
        Ok(message) => Ok(ApiResponse::ok(json!({
            "sent": true,
            "message": message,
            "webhook": masked,
        }))),
        Err(error) => Ok(ApiResponse::ok(json!({
            "sent": false,
            "message": error,
            "webhook": masked,
        }))),
    }
}

fn webhook_status(state: &AppState) -> WebhookStatus {
    let (url, source) = state.notifier.webhook_snapshot();
    WebhookStatus {
        configured: url.is_some(),
        source: source.map(WebhookSource::label),
        masked: url.as_deref().map(notify::mask_webhook),
        enabled: state.config.wecom_enabled,
        url: None,
    }
}

/// 把地址写进 `settings.wecom_webhook`（`value_type = 'secret'`，不出现在通用设置列表里）。
async fn save_webhook(state: &AppState, value: &str) -> Result<(), AppError> {
    let updated = sqlx::query(
        "UPDATE settings SET setting_value = ?1, updated_at = ?2 WHERE setting_key = ?3",
    )
    .bind(value)
    .bind(now())
    .bind(notify::WEBHOOK_SETTING_KEY)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        // 老库升级后还没播种这一行：补一行，避免「保存成功但其实没存」
        sqlx::query(
            "INSERT OR IGNORE INTO settings
                (group_name, setting_key, setting_value, value_type, description, updated_at)
             VALUES ('notification', ?1, ?2, 'secret', ?3, ?4)",
        )
        .bind(notify::WEBHOOK_SETTING_KEY)
        .bind(value)
        .bind("企业微信群机器人 webhook 地址（平台内配置，留空则用环境变量）")
        .bind(now())
        .execute(&state.db)
        .await?;
    }
    Ok(())
}
