//! 班次设置：早 / 中 / 夜之外可自定义的时段（名称 + 时间 + 顺序 + 停用）。

// 同一模块拆出来的几块共用一个词汇表（`STAFF_SELECT` / `ensure_owner` / `AppState` …），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

// ------------------------------------------------------------------ 班次

/// 班次列表：早 / 中 / 夜是默认行，`sort_order` 决定周视图行顺序。
///
/// 停用的班次也会返回（`active = false`）：历史排班里还在用它，
/// 「班次设置」里要能看到并重新启用。
pub async fn list_shift_slots(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<Vec<ShiftSlot>>>, AppError> {
    let sql = format!("{SHIFT_SLOT_SELECT} ORDER BY ss.sort_order ASC, ss.id ASC");
    let slots = sqlx::query_as::<_, ShiftSlot>(&sql)
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(slots))
}

/// 新增班次：名称 + 开始时间 + 结束时间必填，`end <= start` 表示跨零点。
pub async fn create_shift_slot(
    State(state): State<AppState>,
    Json(payload): Json<ShiftSlotRequest>,
) -> Result<Json<ApiResponse<ShiftSlot>>, AppError> {
    let name = payload
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("请填写班次名称".to_string()))?
        .to_string();
    validate_slot_name(&name)?;
    let start_time = require_clock(payload.start_time.as_deref(), "开始时间")?;
    let end_time = require_clock(payload.end_time.as_deref(), "结束时间")?;
    let taken: Option<i64> = sqlx::query_scalar("SELECT id FROM shift_slots WHERE name = ?1")
        .bind(&name)
        .fetch_optional(&state.db)
        .await?;
    if taken.is_some() {
        return Err(AppError::BadRequest(format!("班次「{name}」已存在")));
    }
    // 不指定顺序就排到最后一行，避免新班次插到早班前面
    let sort_order = match payload.sort_order {
        Some(value) => value,
        None => {
            let max: i64 =
                sqlx::query_scalar("SELECT COALESCE(MAX(sort_order), 0) FROM shift_slots")
                    .fetch_one(&state.db)
                    .await?;
            max + 1
        }
    };
    let timestamp = now();
    let result = sqlx::query(
        "INSERT INTO shift_slots (name, start_time, end_time, color, sort_order, active, note,
            created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
    )
    .bind(&name)
    .bind(&start_time)
    .bind(&end_time)
    .bind(
        payload
            .color
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "#0284c7".to_string()),
    )
    .bind(sort_order)
    .bind(payload.active.unwrap_or(true))
    .bind(payload.note)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(
        fetch_shift_slot(&state, result.last_insert_rowid()).await?,
    ))
}

/// 修改班次：改名 / 改时段会同步刷到引用它的排班（`shifts.slot` 与时段快照），
/// 否则同一班次会在列表里出现两种名称或两种时段。
pub async fn update_shift_slot(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<ShiftSlotRequest>,
) -> Result<Json<ApiResponse<ShiftSlot>>, AppError> {
    let existing = fetch_shift_slot(&state, id).await?;
    let name = match payload.name.as_deref().map(str::trim) {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => existing.name.clone(),
    };
    validate_slot_name(&name)?;
    let start_time = match payload.start_time.as_deref() {
        Some(value) => normalize_clock(value, "开始时间")?,
        None => existing.start_time.clone(),
    };
    let end_time = match payload.end_time.as_deref() {
        Some(value) => normalize_clock(value, "结束时间")?,
        None => existing.end_time.clone(),
    };
    let active = payload.active.unwrap_or(existing.active);
    if existing.active && !active {
        // 全部停用后就无法新建排班了，这里先拦住
        let others: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM shift_slots WHERE active = 1 AND id <> ?1")
                .bind(id)
                .fetch_one(&state.db)
                .await?;
        if others == 0 {
            return Err(AppError::BadRequest(
                "至少要保留一个启用中的班次".to_string(),
            ));
        }
    }

    let renaming = name != existing.name;
    if renaming {
        let taken: Option<i64> =
            sqlx::query_scalar("SELECT id FROM shift_slots WHERE name = ?1 AND id <> ?2")
                .bind(&name)
                .bind(id)
                .fetch_optional(&state.db)
                .await?;
        if taken.is_some() {
            return Err(AppError::BadRequest(format!("班次「{name}」已存在")));
        }
        // 「同人 + 同一天 + 同班次」有唯一索引：两个班次里若有同人同天的排班，改名会直接撞库
        let conflict: Option<String> = sqlx::query_scalar(
            "SELECT a.shift_code FROM shifts a
               JOIN shifts b ON b.shift_date = a.shift_date AND b.owner_name = a.owner_name
              WHERE a.slot = ?1 AND b.slot = ?2
              LIMIT 1",
        )
        .bind(&existing.name)
        .bind(&name)
        .fetch_optional(&state.db)
        .await?;
        if let Some(code) = conflict {
            return Err(AppError::BadRequest(format!(
                "「{}」里已有与「{name}」同一天同一责任人的排班（{code}），先调整这些排班再改名",
                existing.name
            )));
        }
    }
    let time_range = format!("{start_time} - {end_time}");

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE shift_slots SET name = ?1, start_time = ?2, end_time = ?3, color = ?4,
            sort_order = ?5, active = ?6, note = ?7, updated_at = ?8 WHERE id = ?9",
    )
    .bind(&name)
    .bind(&start_time)
    .bind(&end_time)
    .bind(payload.color.unwrap_or(existing.color))
    .bind(payload.sort_order.unwrap_or(existing.sort_order))
    .bind(active)
    .bind(payload.note.or(existing.note))
    .bind(now())
    .bind(id)
    .execute(&mut *tx)
    .await?;
    if renaming || time_range != existing.time_range {
        sqlx::query("UPDATE shifts SET slot = ?1, time_range = ?2 WHERE slot = ?3")
            .bind(&name)
            .bind(&time_range)
            .bind(&existing.name)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(ApiResponse::ok(fetch_shift_slot(&state, id).await?))
}

/// 删除班次：还有排班在用就不让删（改成「停用」即可保留历史排班）。
pub async fn delete_shift_slot(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let existing = fetch_shift_slot(&state, id).await?;
    if existing.shift_count > 0 {
        return Err(AppError::BadRequest(format!(
            "还有 {} 条排班在用「{}」，请先改排班或改为停用",
            existing.shift_count, existing.name
        )));
    }
    if existing.active {
        let others: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM shift_slots WHERE active = 1 AND id <> ?1")
                .bind(id)
                .fetch_one(&state.db)
                .await?;
        if others == 0 {
            return Err(AppError::BadRequest(
                "至少要保留一个启用中的班次".to_string(),
            ));
        }
    }
    let result = sqlx::query("DELETE FROM shift_slots WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

pub(crate) fn validate_slot_name(name: &str) -> Result<(), AppError> {
    if name.chars().count() > 20 {
        return Err(AppError::BadRequest("班次名称不要超过 20 个字".to_string()));
    }
    Ok(())
}

pub(crate) fn require_clock(value: Option<&str>, field: &str) -> Result<String, AppError> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => normalize_clock(value, field),
        None => Err(AppError::BadRequest(format!("请填写{field}"))),
    }
}

/// `8:00` / `08:00` / `08:00:00` 都归一成 `HH:MM`，其余格式直接拒绝。
pub(crate) fn normalize_clock(value: &str, field: &str) -> Result<String, AppError> {
    let value = value.trim();
    NaiveTime::parse_from_str(value, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(value, "%H:%M:%S"))
        .map(|time| time.format("%H:%M").to_string())
        .map_err(|_| AppError::BadRequest(format!("{field}格式应为 HH:MM")))
}

/// 班次 → 时段。前端下拉只传班次名，时段一律由服务端按班次表给出，
/// 避免「早班」在不同页面显示成不同时间。停用的班次也认（历史排班还得能编辑）。
pub(crate) async fn slot_time_range(state: &AppState, slot: &str) -> Result<String, AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT start_time, end_time FROM shift_slots WHERE name = ?1")
            .bind(slot)
            .fetch_optional(&state.db)
            .await?;
    match row {
        Some((start, end)) => Ok(format!("{start} - {end}")),
        None => Err(AppError::BadRequest(format!(
            "未知班次「{slot}」，请先在「班次设置」里添加"
        ))),
    }
}

/// 新建排班没带班次时落到第一个启用中的班次（顺序同周视图）。
pub(crate) async fn default_slot_name(state: &AppState) -> Result<String, AppError> {
    let name: Option<String> = sqlx::query_scalar(
        "SELECT name FROM shift_slots WHERE active = 1 ORDER BY sort_order ASC, id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;
    name.ok_or_else(|| AppError::BadRequest("还没有可用班次，请先在「班次设置」里添加".to_string()))
}

pub(crate) async fn fetch_shift_slot(state: &AppState, id: i64) -> Result<ShiftSlot, AppError> {
    let sql = format!("{SHIFT_SLOT_SELECT} WHERE ss.id = ?1");
    sqlx::query_as::<_, ShiftSlot>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("班次不存在".to_string()))
}
