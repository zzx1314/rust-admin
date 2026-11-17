use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{Setting, SettingItem};
use axum::Json;
use axum::extract::State;

pub async fn list_settings(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<Setting>>>, AppError> {
    let settings = fetch_settings(&state).await?;
    Ok(ApiResponse::ok(settings))
}

pub async fn update_settings(
    State(state): State<AppState>,
    Json(items): Json<Vec<SettingItem>>,
) -> Result<Json<ApiResponse<Vec<Setting>>>, AppError> {
    if items.is_empty() {
        return Err(AppError::BadRequest("没有需要保存的配置".to_string()));
    }
    let timestamp = now();
    let mut transaction = state.db.begin().await?;
    for item in items {
        let exists: Option<(i64, String)> =
            sqlx::query_as("SELECT id, value_type FROM settings WHERE setting_key = ?1")
                .bind(&item.setting_key)
                .fetch_optional(&mut *transaction)
                .await?;
        let Some((_, value_type)) = exists else {
            return Err(AppError::BadRequest(format!(
                "未知配置项：{}",
                item.setting_key
            )));
        };
        // 敏感配置（webhook 地址这类凭据）不走通用保存：这里既不回显也不落明文字段
        if value_type == "secret" {
            return Err(AppError::BadRequest(format!(
                "{} 是敏感配置，请在对应模块里单独设置",
                item.setting_key
            )));
        }
        sqlx::query(
            "UPDATE settings SET setting_value = ?1, updated_at = ?2 WHERE setting_key = ?3",
        )
        .bind(item.setting_value)
        .bind(&timestamp)
        .bind(&item.setting_key)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(ApiResponse::ok(fetch_settings(&state).await?))
}

/// 通用设置列表。
///
/// `value_type = 'secret'` 的行（目前只有 `wecom_webhook`）**不在这里返回**：
/// 地址即凭据，设置页通过 `/api/notifications/webhook` 只拿掩码。
async fn fetch_settings(state: &AppState) -> Result<Vec<Setting>, AppError> {
    let settings = sqlx::query_as::<_, Setting>(
        "SELECT id, group_name, setting_key, setting_value, value_type, description, updated_at
         FROM settings WHERE value_type <> 'secret' ORDER BY id ASC",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(settings)
}
