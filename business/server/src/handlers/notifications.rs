use crate::app::AppState;
use crate::error::{ApiResponse, AppError};
use crate::notify;
use axum::Json;
use axum::extract::{Query, State};
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

/// 手动发一条测试消息，把企业微信的原始错误直接返回给设置页。
///
/// 同步发送（不走队列）是故意的：配错 key 时群里是「静默不出现」，
/// 只有拿到 `errcode` 才查得动。
pub async fn send_test(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    match notify::send_test(&state).await {
        Ok(message) => Ok(ApiResponse::ok(json!({
            "sent": true,
            "message": message,
            "webhook": state.config.wecom_webhook.as_deref().map(mask_webhook),
        }))),
        Err(error) => Ok(ApiResponse::ok(json!({
            "sent": false,
            "message": error,
            "webhook": state.config.wecom_webhook.as_deref().map(mask_webhook),
        }))),
    }
}

/// webhook 只回显 key 的前 8 位：足够核对「填的是哪一个」，又不至于泄漏。
fn mask_webhook(url: &str) -> String {
    match url.split_once("key=") {
        Some((prefix, key)) => {
            let head: String = key.chars().take(8).collect();
            format!("{prefix}key={head}…")
        }
        None => "（格式异常：缺少 key= 参数）".to_string(),
    }
}
