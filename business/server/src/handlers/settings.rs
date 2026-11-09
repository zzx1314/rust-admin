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
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM settings WHERE setting_key = ?1")
                .bind(&item.setting_key)
                .fetch_optional(&mut *transaction)
                .await?;
        if exists.is_none() {
            return Err(AppError::BadRequest(format!(
                "未知配置项：{}",
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

async fn fetch_settings(state: &AppState) -> Result<Vec<Setting>, AppError> {
    let settings = sqlx::query_as::<_, Setting>(
        "SELECT id, group_name, setting_key, setting_value, value_type, description, updated_at
         FROM settings ORDER BY id ASC",
    )
    .fetch_all(&state.db)
    .await?;
    Ok(settings)
}
