//! 排班（计划层）：增删改查、统计、单条复制。

// 同一模块拆出来的几块共用一个词汇表（`STAFF_SELECT` / `ensure_owner` / `AppState` …），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

// ------------------------------------------------------------------ 排班

pub async fn list_shifts(
    State(state): State<AppState>,
    Query(query): Query<ShiftQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<Shift>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(SHIFT_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(from) = query.from.filter(|value| !value.trim().is_empty()) {
        builder.push(" AND s.shift_date >= ").push_bind(from);
    }
    if let Some(to) = query.to.filter(|value| !value.trim().is_empty()) {
        builder.push(" AND s.shift_date <= ").push_bind(to);
    }
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND s.status = ").push_bind(status);
    }
    if let Some(staff_id) = query.staff_id {
        builder.push(" AND s.staff_id = ").push_bind(staff_id);
    }
    if let Some(park_id) = query.park_id {
        builder.push(" AND s.park_id = ").push_bind(park_id);
    }
    if !claims.is_admin {
        builder.push(" AND s.created_by = ").push_bind(claims.sub);
    }
    builder.push(" ORDER BY s.shift_date ASC, s.id ASC");
    let shifts = builder
        .build_query_as::<Shift>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(shifts))
}

pub async fn shift_stats(
    State(state): State<AppState>,
    Query(query): Query<ShiftQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    // 借用而不是移动：同一组过滤条件要跑两条 SQL（分组计数 + 去重人数）
    let from = query
        .from
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let to = query.to.as_deref().filter(|value| !value.trim().is_empty());

    let mut builder =
        QueryBuilder::<sqlx::Sqlite>::new("SELECT status, COUNT(*) FROM shifts WHERE 1 = 1");
    if let Some(from) = from {
        builder.push(" AND shift_date >= ").push_bind(from);
    }
    if let Some(to) = to {
        builder.push(" AND shift_date <= ").push_bind(to);
    }
    if !claims.is_admin {
        builder.push(" AND created_by = ").push_bind(claims.sub);
    }
    builder.push(" GROUP BY status");
    let rows: Vec<(String, i64)> = builder.build_query_as().fetch_all(&state.db).await?;
    let total: i64 = rows.iter().map(|(_, count)| count).sum();

    // 在岗责任人单独统计：按状态分组算出来的是「每组的去重人数」，取最大值会漏人
    let mut owners_builder = QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT COUNT(DISTINCT owner_name) FROM shifts WHERE 1 = 1",
    );
    if let Some(from) = from {
        owners_builder.push(" AND shift_date >= ").push_bind(from);
    }
    if let Some(to) = to {
        owners_builder.push(" AND shift_date <= ").push_bind(to);
    }
    if !claims.is_admin {
        owners_builder
            .push(" AND created_by = ")
            .push_bind(claims.sub);
    }
    let owners: i64 = owners_builder
        .build_query_scalar()
        .fetch_one(&state.db)
        .await?;

    let mut counts = serde_json::Map::new();
    for (status, count) in &rows {
        counts.insert(status.clone(), json!(count));
    }
    let published = counts.get("已发布").and_then(Value::as_i64).unwrap_or(0);
    let pending = counts.get("待确认").and_then(Value::as_i64).unwrap_or(0)
        + counts.get("草案").and_then(Value::as_i64).unwrap_or(0);
    Ok(ApiResponse::ok(json!({
        "total": total,
        "counts": counts,
        "published": published,
        "pending": pending,
        "owners": owners,
    })))
}

pub async fn get_shift(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Shift>>, AppError> {
    let shift = fetch_shift(&state, id).await?;
    ensure_owner(shift.created_by, &claims)?;
    Ok(ApiResponse::ok(shift))
}

pub async fn create_shift(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ShiftRequest>,
) -> Result<Json<ApiResponse<Shift>>, AppError> {
    // 日期 / 班次 / 责任人都在计划层，缺一个就不该落库（周视图会落在空格里）
    let shift_date = payload
        .shift_date
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("请选择排班日期".to_string()))?
        .to_string();
    let slot = match payload
        .slot
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(slot) => slot.to_string(),
        None => default_slot_name(&state).await?,
    };
    let time_range = slot_time_range(&state, &slot).await?;
    let (staff_id, owner_name) =
        resolve_staff(&state, payload.staff_id, payload.owner_name).await?;
    let task_name = resolve_task_name(&state, payload.task_id.flatten(), payload.task_name).await?;

    ensure_slot_free(&state, &shift_date, &slot, &owner_name, None).await?;

    let next_id = crate::ids::next_id(&state.db, "shifts", "shifts").await?;
    let shift_code = format!("SH-{next_id:03}");
    let timestamp = now();
    let result = sqlx::query(
        "INSERT INTO shifts (shift_code, shift_date, slot, time_range, staff_id, owner_name,
            park_id, zone_id, task_id, task_name, status, note, created_at, updated_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
    )
    .bind(&shift_code)
    .bind(&shift_date)
    .bind(&slot)
    .bind(&time_range)
    .bind(staff_id)
    .bind(&owner_name)
    .bind(payload.park_id)
    .bind(payload.zone_id)
    .bind(payload.task_id.flatten())
    .bind(&task_name)
    .bind(payload.status.unwrap_or_else(|| "草案".to_string()))
    .bind(payload.note)
    .bind(&timestamp)
    .bind(claims.sub)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(
        fetch_shift(&state, result.last_insert_rowid()).await?,
    ))
}

pub async fn update_shift(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ShiftRequest>,
) -> Result<Json<ApiResponse<Shift>>, AppError> {
    let existing = fetch_shift(&state, id).await?;
    ensure_owner(existing.created_by, &claims)?;

    let shift_date = payload
        .shift_date
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(existing.shift_date.clone());
    let slot = payload.slot.unwrap_or(existing.slot.clone());
    let time_range = slot_time_range(&state, &slot).await?;
    let (staff_id, owner_name) = if payload.staff_id.is_some() || payload.owner_name.is_some() {
        resolve_staff(&state, payload.staff_id, payload.owner_name).await?
    } else {
        (existing.staff_id, existing.owner_name.clone())
    };
    let (task_id, task_name) = match payload.task_id {
        Some(task_id) => (
            task_id,
            resolve_task_name(&state, task_id, payload.task_name.clone()).await?,
        ),
        None => (
            existing.task_id,
            payload.task_name.or(existing.task_name.clone()),
        ),
    };

    ensure_slot_free(&state, &shift_date, &slot, &owner_name, Some(id)).await?;

    sqlx::query(
        "UPDATE shifts SET shift_date = ?1, slot = ?2, time_range = ?3, staff_id = ?4,
            owner_name = ?5, park_id = ?6, zone_id = ?7, task_id = ?8, task_name = ?9,
            status = ?10, note = ?11, updated_at = ?12 WHERE id = ?13",
    )
    .bind(&shift_date)
    .bind(&slot)
    .bind(&time_range)
    .bind(staff_id)
    .bind(&owner_name)
    .bind(payload.park_id.or(existing.park_id))
    .bind(payload.zone_id.or(existing.zone_id))
    .bind(task_id)
    .bind(&task_name)
    .bind(payload.status.unwrap_or(existing.status))
    .bind(payload.note.or(existing.note))
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_shift(&state, id).await?))
}

pub async fn delete_shift(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let shift = fetch_shift(&state, id).await?;
    ensure_owner(shift.created_by, &claims)?;
    let result = sqlx::query("DELETE FROM shifts WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

/// 复制单条排班到另一天 / 另一班次（周视图把卡片拖到目标格子）。
///
/// 与「整段复制」的分工：这条只复制**一条**，用于「周三早班拖到周四」这种手工补排，
/// 不用再打开表单重填一遍。冲突规则与新建完全一致（同人 + 同天 + 同班次只能一条），
/// 撞上直接回带原因的 400，前端原样提示。
pub async fn duplicate_shift(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ShiftDuplicateRequest>,
) -> Result<Json<ApiResponse<Shift>>, AppError> {
    let source = fetch_shift(&state, id).await?;
    ensure_owner(source.created_by, &claims)?;

    let shift_date = payload.shift_date.trim().to_string();
    let parsed = parse_day(&shift_date, "目标日期")?;
    let shift_date = parsed.format("%Y-%m-%d").to_string();
    let slot = payload
        .slot
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| source.slot.clone());
    let time_range = slot_time_range(&state, &slot).await?;
    if shift_date == source.shift_date && slot == source.slot {
        return Err(AppError::BadRequest(
            "目标就是原排班所在的日期与班次".to_string(),
        ));
    }
    // 同一个人同一天同一班次只能一条：拖重了就直接说清是谁冲突
    ensure_slot_free(&state, &shift_date, &slot, &source.owner_name, None).await?;

    let next_id = crate::ids::next_id(&state.db, "shifts", "shifts").await?;
    let timestamp = now();
    let result = sqlx::query(
        "INSERT INTO shifts (shift_code, shift_date, slot, time_range, staff_id, owner_name,
            park_id, zone_id, task_id, task_name, status, note, created_at, updated_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
    )
    .bind(format!("SH-{next_id:03}"))
    .bind(&shift_date)
    .bind(&slot)
    .bind(&time_range)
    .bind(source.staff_id)
    .bind(&source.owner_name)
    .bind(source.park_id)
    .bind(source.zone_id)
    .bind(source.task_id)
    .bind(&source.task_name)
    .bind(&source.status)
    .bind(&source.note)
    .bind(&timestamp)
    .bind(claims.sub)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(
        fetch_shift(&state, result.last_insert_rowid()).await?,
    ))
}
pub(crate) fn ensure_owner(created_by: Option<i64>, claims: &Claims) -> Result<(), AppError> {
    if claims.is_admin || created_by == Some(claims.sub) {
        return Ok(());
    }
    Err(AppError::NotFound("排班记录不存在".to_string()))
}

/// 责任人解析：优先 `staffId`（姓名取责任人库），否则按姓名反查 id。
pub(crate) async fn resolve_staff(
    state: &AppState,
    staff_id: Option<i64>,
    owner_name: Option<String>,
) -> Result<(Option<i64>, String), AppError> {
    if let Some(staff_id) = staff_id {
        let row: Option<(Option<String>,)> = sqlx::query_as("SELECT name FROM staff WHERE id = ?1")
            .bind(staff_id)
            .fetch_optional(&state.db)
            .await?;
        let Some((name,)) = row else {
            return Err(AppError::NotFound(format!("责任人 {staff_id} 不存在")));
        };
        let name = match name {
            Some(name) if !name.trim().is_empty() => name,
            _ => owner_name
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| AppError::BadRequest("责任人姓名不能为空".to_string()))?,
        };
        return Ok((Some(staff_id), name));
    }
    let name = owner_name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("请选择责任人".to_string()))?;
    let matched: Option<i64> = sqlx::query_scalar("SELECT id FROM staff WHERE name = ?1")
        .bind(&name)
        .fetch_optional(&state.db)
        .await?;
    Ok((matched, name))
}

pub(crate) async fn resolve_task_name(
    state: &AppState,
    task_id: Option<i64>,
    task_name: Option<String>,
) -> Result<Option<String>, AppError> {
    if let Some(task_id) = task_id {
        let name: Option<String> = sqlx::query_scalar("SELECT task_name FROM tasks WHERE id = ?1")
            .bind(task_id)
            .fetch_optional(&state.db)
            .await?;
        return Ok(name.or(task_name));
    }
    Ok(task_name)
}

/// 冲突校验：同一责任人、同一天、同一班次只能排一条。
pub(crate) async fn ensure_slot_free(
    state: &AppState,
    shift_date: &str,
    slot: &str,
    owner_name: &str,
    exclude_id: Option<i64>,
) -> Result<(), AppError> {
    let conflict: Option<(String,)> = sqlx::query_as(
        "SELECT shift_code FROM shifts
         WHERE shift_date = ?1 AND slot = ?2 AND owner_name = ?3
           AND (?4 IS NULL OR id <> ?4)",
    )
    .bind(shift_date)
    .bind(slot)
    .bind(owner_name)
    .bind(exclude_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some((code,)) = conflict {
        return Err(AppError::BadRequest(format!(
            "冲突：{owner_name} 在 {shift_date} {slot} 已有排班（{code}）"
        )));
    }
    Ok(())
}
pub(crate) async fn fetch_shift(state: &AppState, id: i64) -> Result<Shift, AppError> {
    let sql = format!("{SHIFT_SELECT} WHERE s.id = ?1");
    sqlx::query_as::<_, Shift>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("排班记录不存在".to_string()))
}
