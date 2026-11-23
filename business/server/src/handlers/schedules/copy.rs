//! 整段复制：把上一周 / 上个月的排班搬到目标区间（skip / replace 两种模式）。

// 同一模块拆出来的几块共用一个词汇表（`STAFF_SELECT` / `ensure_owner` / `AppState` …），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

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

        let occupant = slot_occupant(&mut tx, &target_date, &source.slot, &claims).await?;
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

        // 取号也走事务里的同一条连接：用池子取号会拿另一条连接，
        // 事务已持有写锁时直接 `SQLITE_BUSY`。
        let shift_code = crate::ids::next_id_on(&mut tx, "shifts", "shifts").await?;
        sqlx::query(
            "INSERT INTO shifts (shift_code, shift_date, slot, time_range, staff_id, owner_name,
                park_id, zone_id, task_id, task_name, status, note, created_at, updated_at,
                created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14)",
        )
        .bind(format!("SH-{shift_code:03}"))
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
pub(crate) struct ShiftSource {
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

pub(crate) fn parse_day(value: &str, field: &str) -> Result<NaiveDate, AppError> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|_| AppError::BadRequest(format!("{field}格式应为 YYYY-MM-DD")))
}

/// 目标时段里（当前用户可见范围内）的第一条排班编号。
pub(crate) async fn slot_occupant(
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
pub(crate) async fn delete_slot(
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
