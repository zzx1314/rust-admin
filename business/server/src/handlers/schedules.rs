use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{
    Shift, ShiftCopyRequest, ShiftCopyResult, ShiftCopySkip, ShiftQuery, ShiftRequest, Staff,
    StaffQuery, StaffRequest,
};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{Duration, NaiveDate};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Sqlite, SqlitePool, Transaction};
use std::collections::HashSet;

/// 班次 → 时段。前端下拉只传班次名，时段由后端统一给出，
/// 避免「早班」在不同页面显示成不同时间。
const SLOT_RANGES: [(&str, &str); 3] = [
    ("早班", "08:00 - 16:00"),
    ("中班", "16:00 - 00:00"),
    ("夜班", "00:00 - 08:00"),
];

/// 排班状态（复制时允许改写成其中一种）
const SHIFT_STATUSES: [&str; 3] = ["已发布", "待确认", "草案"];

fn slot_range(slot: &str) -> Option<&'static str> {
    SLOT_RANGES
        .iter()
        .find(|(name, _)| *name == slot)
        .map(|(_, range)| *range)
}

const STAFF_SELECT: &str = "SELECT st.id, st.staff_code, st.name, st.role, st.team, st.phone,
    st.color, st.status, st.skills, st.created_at, st.updated_at,
    (SELECT COUNT(*) FROM shifts s
      WHERE s.staff_id = st.id
        AND (?1 IS NULL OR s.shift_date >= ?1)
        AND (?2 IS NULL OR s.shift_date <= ?2)) AS shift_count,
    (SELECT COUNT(*) FROM work_orders w
      WHERE w.staff_id = st.id AND w.status <> '已完成') AS open_order_count
    FROM staff st";

const SHIFT_SELECT: &str = "SELECT s.id, s.shift_code, s.shift_date, s.slot, s.time_range,
    s.staff_id, s.owner_name, st.role AS staff_role, st.team AS staff_team,
    st.color AS staff_color, s.park_id, p.park_name, s.zone_id, z.zone_name,
    s.task_id, s.task_name,
    (SELECT COUNT(*) FROM devices d
      WHERE s.park_id IS NOT NULL AND d.park_id = s.park_id
        AND (s.zone_id IS NULL OR d.zone_id = s.zone_id)) AS device_count,
    s.status, s.note, s.created_at, s.updated_at, s.created_by
    FROM shifts s
    LEFT JOIN staff st ON st.id = s.staff_id
    LEFT JOIN parks p ON p.id = s.park_id
    LEFT JOIN zones z ON z.id = s.zone_id";

// ---------------------------------------------------------------- 责任人库

/// 责任人列表：排班与工单共用。
///
/// `from` / `to` 只影响「班次数」这一列（周视图里看本周负载），
/// 责任人库本身是平台级共享数据，不受创建人过滤。
pub async fn list_staff(
    State(state): State<AppState>,
    Query(query): Query<StaffQuery>,
) -> Result<Json<ApiResponse<Vec<Staff>>>, AppError> {
    let from = query.from.filter(|value| !value.trim().is_empty());
    let to = query.to.filter(|value| !value.trim().is_empty());
    let sql = format!("{STAFF_SELECT} ORDER BY st.id ASC");
    let staff = sqlx::query_as::<_, Staff>(&sql)
        .bind(from)
        .bind(to)
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(staff))
}

pub async fn create_staff(
    State(state): State<AppState>,
    Json(payload): Json<StaffRequest>,
) -> Result<Json<ApiResponse<Staff>>, AppError> {
    let name = payload.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("责任人姓名不能为空".to_string()));
    }
    let next_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM staff")
        .fetch_one(&state.db)
        .await?;
    let staff_code = payload
        .staff_code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("EMP-{next_id:02}"));
    let timestamp = now();
    let result = sqlx::query(
        "INSERT INTO staff (staff_code, name, role, team, phone, color, status, skills,
            created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(&staff_code)
    .bind(name)
    .bind(payload.role)
    .bind(payload.team)
    .bind(payload.phone)
    .bind(payload.color.unwrap_or_else(|| "#0284c7".to_string()))
    .bind(payload.status.unwrap_or_else(|| "在岗".to_string()))
    .bind(payload.skills)
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(
        fetch_staff(&state, result.last_insert_rowid()).await?,
    ))
}

pub async fn update_staff(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<StaffRequest>,
) -> Result<Json<ApiResponse<Staff>>, AppError> {
    let existing = fetch_staff(&state, id).await?;
    // 责任人改名时同步排班/工单里的冗余姓名，否则周视图还显示旧名字
    let name = if payload.name.trim().is_empty() {
        existing.name.clone()
    } else {
        payload.name.trim().to_string()
    };
    if name != existing.name {
        sqlx::query("UPDATE shifts SET owner_name = ?1 WHERE staff_id = ?2")
            .bind(&name)
            .bind(id)
            .execute(&state.db)
            .await?;
        sqlx::query("UPDATE work_orders SET assignee = ?1 WHERE staff_id = ?2")
            .bind(&name)
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    sqlx::query(
        "UPDATE staff SET name = ?1, role = ?2, team = ?3, phone = ?4, color = ?5,
            status = ?6, skills = ?7, updated_at = ?8 WHERE id = ?9",
    )
    .bind(&name)
    .bind(payload.role.or(existing.role))
    .bind(payload.team.or(existing.team))
    .bind(payload.phone.or(existing.phone))
    .bind(payload.color.unwrap_or(existing.color))
    .bind(payload.status.unwrap_or(existing.status))
    .bind(payload.skills.or(existing.skills))
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_staff(&state, id).await?))
}

pub async fn delete_staff(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    // 先确认存在，找不到给 404 而不是静默 true
    fetch_staff(&state, id).await?;

    // 今天及以后的排班 / 在办工单还指着这个人，直接删会在排班里留下空档
    let upcoming: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM shifts WHERE staff_id = ?1 AND shift_date >= ?2",
    )
    .bind(id)
    .bind(crate::db::today())
    .fetch_one(&state.db)
    .await?;
    let open_orders: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM work_orders WHERE staff_id = ?1 AND status <> '已完成'",
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;
    if upcoming > 0 || open_orders > 0 {
        return Err(AppError::BadRequest(format!(
            "该责任人还有 {upcoming} 条今天及以后的排班、{open_orders} 条在办工单，先改派或删除后再移除"
        )));
    }

    // 历史记录保留文案（owner_name / assignee），只解除对责任人库的引用
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE shifts SET staff_id = NULL WHERE staff_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE work_orders SET staff_id = NULL WHERE staff_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("DELETE FROM staff WHERE id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

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
    let from = query.from.as_deref().filter(|value| !value.trim().is_empty());
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
        owners_builder.push(" AND created_by = ").push_bind(claims.sub);
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
    let slot = payload
        .slot
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("早班")
        .to_string();
    let time_range = slot_range(&slot)
        .ok_or_else(|| AppError::BadRequest(format!("未知班次：{slot}")))?
        .to_string();
    let (staff_id, owner_name) = resolve_staff(&state, payload.staff_id, payload.owner_name).await?;
    let task_name = resolve_task_name(&state, payload.task_id.flatten(), payload.task_name).await?;

    ensure_slot_free(&state, &shift_date, &slot, &owner_name, None).await?;

    let next_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM shifts")
        .fetch_one(&state.db)
        .await?;
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
    let time_range = slot_range(&slot)
        .ok_or_else(|| AppError::BadRequest(format!("未知班次：{slot}")))?
        .to_string();
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
        None => (existing.task_id, payload.task_name.or(existing.task_name.clone())),
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

/// 整段复制排班：把「上一周 / 上个月」的排班平移到目标区间。
///
/// 对齐方式：目标日期 = 目标区间第一天 + （源日期 − 源区间第一天）的天数偏移。
/// 因此周复制保持同名星期，月复制保持同一日号；目标区间天数更少时（如 1/31 → 2 月）
/// 多出来的日期直接跳过并回报原因，不会写到区间外。
///
/// 占用策略：`skip`（默认）跳过目标时段已有排班的记录，`replace` 覆盖目标时段；
/// 非管理员的范围与列表一致，只看得到、也只改得动自己创建的排班。
pub async fn copy_shifts(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<ShiftCopyRequest>,
) -> Result<Json<ApiResponse<ShiftCopyResult>>, AppError> {
    let source_from = parse_day(&payload.source_from, "源区间开始日期")?;
    let source_to = parse_day(&payload.source_to, "源区间结束日期")?;
    let target_from = parse_day(&payload.target_from, "目标区间开始日期")?;
    let target_to = parse_day(&payload.target_to, "目标区间结束日期")?;
    if source_from > source_to {
        return Err(AppError::BadRequest(
            "源区间开始日期不能晚于结束日期".to_string(),
        ));
    }
    if target_from > target_to {
        return Err(AppError::BadRequest(
            "目标区间开始日期不能晚于结束日期".to_string(),
        ));
    }
    // 区间重叠会把刚复制出来的记录再复制一遍，直接拒绝并提示选上一周 / 上个月
    if source_from <= target_to && target_from <= source_to {
        return Err(AppError::BadRequest(
            "源区间与目标区间重叠，请选择上一周或上个月".to_string(),
        ));
    }
    let mode = payload
        .mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("skip")
        .to_string();
    if mode != "skip" && mode != "replace" {
        return Err(AppError::BadRequest(format!("未知复制模式：{mode}")));
    }
    if let Some(status) = payload.status.as_deref()
        && !SHIFT_STATUSES.contains(&status)
    {
        return Err(AppError::BadRequest(format!("未知排班状态：{status}")));
    }

    let mut builder = QueryBuilder::<Sqlite>::new(
        "SELECT shift_date, slot, time_range, staff_id, owner_name, park_id, zone_id,
            task_id, task_name, status, note
         FROM shifts WHERE shift_date >= ",
    );
    builder
        .push_bind(payload.source_from.trim().to_string())
        .push(" AND shift_date <= ")
        .push_bind(payload.source_to.trim().to_string());
    if !claims.is_admin {
        builder.push(" AND created_by = ").push_bind(claims.sub);
    }
    builder.push(" ORDER BY shift_date ASC, id ASC");
    let sources = builder
        .build_query_as::<ShiftSource>()
        .fetch_all(&state.db)
        .await?;

    let mut tx = state.db.begin().await?;
    let mut next_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM shifts")
        .fetch_one(&mut *tx)
        .await?;
    let timestamp = now();
    let mut created = 0_i64;
    let mut replaced = 0_i64;
    let mut skipped: Vec<ShiftCopySkip> = Vec::new();
    let mut replaced_slots: HashSet<(String, String)> = HashSet::new();

    for source in &sources {
        let Some(source_date) = NaiveDate::parse_from_str(&source.shift_date, "%Y-%m-%d").ok()
        else {
            skipped.push(ShiftCopySkip {
                shift_date: source.shift_date.clone(),
                slot: source.slot.clone(),
                owner_name: source.owner_name.clone(),
                reason: "源日期格式不合法".to_string(),
            });
            continue;
        };
        let target_date = target_from + Duration::days((source_date - source_from).num_days());
        if target_date > target_to {
            skipped.push(ShiftCopySkip {
                shift_date: target_date.format("%Y-%m-%d").to_string(),
                slot: source.slot.clone(),
                owner_name: source.owner_name.clone(),
                reason: "目标区间天数不足，已跳过".to_string(),
            });
            continue;
        }
        let target_date = target_date.format("%Y-%m-%d").to_string();

        let occupant =
            slot_occupant(&mut tx, &target_date, &source.slot, &claims).await?;
        if mode == "skip" {
            if let Some(code) = occupant {
                skipped.push(ShiftCopySkip {
                    shift_date: target_date,
                    slot: source.slot.clone(),
                    owner_name: source.owner_name.clone(),
                    reason: format!("目标时段已有排班（{code}）"),
                });
                continue;
            }
        } else if replaced_slots.insert((target_date.clone(), source.slot.clone())) {
            // 同一目标时段只清一次：源里有多人同班次时，后面的复制行还要保留
            replaced += delete_slot(&mut tx, &target_date, &source.slot, &claims).await?;
        }

        sqlx::query(
            "INSERT INTO shifts (shift_code, shift_date, slot, time_range, staff_id, owner_name,
                park_id, zone_id, task_id, task_name, status, note, created_at, updated_at,
                created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
        )
        .bind(format!("SH-{next_id:03}"))
        .bind(&target_date)
        .bind(&source.slot)
        .bind(&source.time_range)
        .bind(source.staff_id)
        .bind(&source.owner_name)
        .bind(source.park_id)
        .bind(source.zone_id)
        .bind(source.task_id)
        .bind(&source.task_name)
        .bind(
            payload
                .status
                .as_deref()
                .unwrap_or(source.status.as_str())
                .to_string(),
        )
        .bind(&source.note)
        .bind(&timestamp)
        .bind(claims.sub)
        .execute(&mut *tx)
        .await?;
        next_id += 1;
        created += 1;
    }

    tx.commit().await?;
    Ok(ApiResponse::ok(ShiftCopyResult {
        source: sources.len() as i64,
        created,
        replaced,
        skipped,
    }))
}

// ------------------------------------------------------------------ 内部

/// 复制源行（只取生成排班需要的列）
#[derive(Debug, sqlx::FromRow)]
struct ShiftSource {
    shift_date: String,
    slot: String,
    time_range: String,
    staff_id: Option<i64>,
    owner_name: String,
    park_id: Option<i64>,
    zone_id: Option<i64>,
    task_id: Option<i64>,
    task_name: Option<String>,
    status: String,
    note: Option<String>,
}

fn parse_day(value: &str, field: &str) -> Result<NaiveDate, AppError> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::BadRequest(format!("{field}格式应为 YYYY-MM-DD")))
}

/// 目标时段里（当前用户可见范围内）的第一条排班编号。
async fn slot_occupant(
    tx: &mut Transaction<'_, Sqlite>,
    date: &str,
    slot: &str,
    claims: &Claims,
) -> Result<Option<String>, sqlx::Error> {
    let mut builder =
        QueryBuilder::<Sqlite>::new("SELECT shift_code FROM shifts WHERE shift_date = ");
    builder
        .push_bind(date.to_string())
        .push(" AND slot = ")
        .push_bind(slot.to_string());
    if !claims.is_admin {
        builder.push(" AND created_by = ").push_bind(claims.sub);
    }
    builder.push(" ORDER BY id ASC LIMIT 1");
    builder
        .build_query_scalar::<String>()
        .fetch_optional(&mut **tx)
        .await
}

/// 覆盖模式：清掉目标时段里当前用户可动的排班，返回删除条数。
async fn delete_slot(
    tx: &mut Transaction<'_, Sqlite>,
    date: &str,
    slot: &str,
    claims: &Claims,
) -> Result<i64, sqlx::Error> {
    let mut builder = QueryBuilder::<Sqlite>::new("DELETE FROM shifts WHERE shift_date = ");
    builder
        .push_bind(date.to_string())
        .push(" AND slot = ")
        .push_bind(slot.to_string());
    if !claims.is_admin {
        builder.push(" AND created_by = ").push_bind(claims.sub);
    }
    let result = builder.build().execute(&mut **tx).await?;
    Ok(result.rows_affected() as i64)
}

fn ensure_owner(created_by: Option<i64>, claims: &Claims) -> Result<(), AppError> {
    if claims.is_admin || created_by == Some(claims.sub) {
        return Ok(());
    }
    Err(AppError::NotFound("排班记录不存在".to_string()))
}

/// 责任人解析：优先 `staffId`（姓名取责任人库），否则按姓名反查 id。
async fn resolve_staff(
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

async fn resolve_task_name(
    state: &AppState,
    task_id: Option<i64>,
    task_name: Option<String>,
) -> Result<Option<String>, AppError> {
    if let Some(task_id) = task_id {
        let name: Option<String> =
            sqlx::query_scalar("SELECT task_name FROM tasks WHERE id = ?1")
                .bind(task_id)
                .fetch_optional(&state.db)
                .await?;
        return Ok(name.or(task_name));
    }
    Ok(task_name)
}

/// 冲突校验：同一责任人、同一天、同一班次只能排一条。
async fn ensure_slot_free(
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

async fn fetch_staff(state: &AppState, id: i64) -> Result<Staff, AppError> {
    let sql = format!("{STAFF_SELECT} WHERE st.id = ?3");
    sqlx::query_as::<_, Staff>(&sql)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("责任人不存在".to_string()))
}

async fn fetch_shift(state: &AppState, id: i64) -> Result<Shift, AppError> {
    let sql = format!("{SHIFT_SELECT} WHERE s.id = ?1");
    sqlx::query_as::<_, Shift>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("排班记录不存在".to_string()))
}

/// 供工单模块按姓名补 `staff_id`，避免责任人库与工单里的冗余姓名对不上。
pub(crate) async fn staff_id_by_name(
    db: &SqlitePool,
    name: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM staff WHERE name = ?1")
        .bind(name)
        .fetch_optional(db)
        .await
}
