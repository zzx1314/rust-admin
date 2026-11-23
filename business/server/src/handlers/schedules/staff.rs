//! 责任人库：增删改查、Excel 导入、账号绑定校验。

// 同一模块拆出来的几块共用一个词汇表（`STAFF_SELECT` / `ensure_owner` / `AppState` …），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

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
    let next_id = crate::ids::next_id(&state.db, "staff", "staff").await?;
    let staff_code = payload
        .staff_code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("EMP-{next_id:02}"));
    let timestamp = now();
    let user_id = payload.user_id.flatten();
    check_account_binding(&state, user_id, None).await?;
    let result = sqlx::query(
        "INSERT INTO staff (staff_code, name, role, team, phone, color, status, skills,
            user_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )
    .bind(&staff_code)
    .bind(name)
    .bind(payload.role)
    .bind(payload.team)
    .bind(payload.phone)
    .bind(payload.color.unwrap_or_else(|| "#0284c7".to_string()))
    .bind(payload.status.unwrap_or_else(|| "在岗".to_string()))
    .bind(payload.skills)
    .bind(user_id)
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
    // 账号绑定：请求里给了就按请求（含显式解绑 null），没给就保持原值
    let user_id = match payload.user_id {
        Some(requested) => {
            check_account_binding(&state, requested, Some(id)).await?;
            requested
        }
        None => existing.user_id,
    };
    sqlx::query(
        "UPDATE staff SET name = ?1, role = ?2, team = ?3, phone = ?4, color = ?5,
            status = ?6, skills = ?7, user_id = ?8, updated_at = ?9 WHERE id = ?10",
    )
    .bind(&name)
    .bind(payload.role.or(existing.role))
    .bind(payload.team.or(existing.team))
    .bind(payload.phone.or(existing.phone))
    .bind(payload.color.unwrap_or(existing.color))
    .bind(payload.status.unwrap_or(existing.status))
    .bind(payload.skills.or(existing.skills))
    .bind(user_id)
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
    let upcoming: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shifts WHERE staff_id = ?1 AND shift_date >= ?2")
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

// ---------------------------------------------------- 责任人库 Excel 导入

/// 导入模板下载：`GET /api/staff/import-template`
///
/// 模板里两张表：`责任人`（表头 + 列宽 + 冻结首行，由用户填）与 `填写说明`。
/// 中文文件名同时给 `filename`（老浏览器）与 `filename*`（RFC 5987）。
pub async fn staff_import_template() -> Result<Response, AppError> {
    let bytes = crate::staff_import::template().map_err(AppError::Internal)?;
    let disposition = format!(
        "attachment; filename=\"staff-import-template.xlsx\"; filename*=UTF-8''{}",
        urlencoding::encode(crate::staff_import::TEMPLATE_FILENAME)
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .map_err(|error| AppError::Internal(format!("构造导入模板响应失败: {error}")))
}

/// Excel 导入：`POST /api/staff/import`（multipart，字段名 `file`）
///
/// 逐行处理，坏行不影响好行：
/// - 姓名为空 / 在岗状态不认识 / 内容超长 → `failed`，附原因；
/// - 与库里或同一份文件里的姓名、工号重复 → `skipped`（重复导入同一份文件是安全的）。
pub async fn import_staff(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<StaffImportResult>>, AppError> {
    let mut upload: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::BadRequest(format!("无效的上传请求: {error}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let file_name = field.file_name().unwrap_or("staff.xlsx").to_string();
        let data = field
            .bytes()
            .await
            .map_err(|error| AppError::BadRequest(format!("读取上传文件失败: {error}")))?;
        upload = Some((file_name, data.to_vec()));
    }

    let (file_name, bytes) =
        upload.ok_or_else(|| AppError::BadRequest("请选择要导入的 Excel 文件".to_string()))?;
    if bytes.is_empty() {
        return Err(AppError::BadRequest("上传的文件是空的".to_string()));
    }
    if !file_name.to_lowercase().ends_with(".xlsx") {
        return Err(AppError::BadRequest(
            "只支持 .xlsx（Excel 2007 及以上）；旧版 .xls 请先另存为 .xlsx".to_string(),
        ));
    }
    if bytes.len() > crate::staff_import::MAX_FILE_BYTES {
        return Err(AppError::BadRequest(format!(
            "文件超过 {}MB，请拆分后再导入",
            crate::staff_import::MAX_FILE_BYTES / 1024 / 1024
        )));
    }

    let outcome = crate::staff_import::parse_workbook(&bytes).map_err(AppError::BadRequest)?;
    if outcome.total() == 0 {
        return Err(AppError::BadRequest(
            "文件里没有可导入的数据行：表头下面至少要填一行".to_string(),
        ));
    }

    let mut issues: Vec<StaffImportRow> = outcome
        .rejected
        .iter()
        .map(|rejected| StaffImportRow {
            row: rejected.row as i64,
            name: rejected.name.clone(),
            status: "failed".to_string(),
            message: rejected.message.clone(),
        })
        .collect();
    let mut created_names: Vec<String> = Vec::new();
    // 同一份文件内部的查重（大小写不敏感，中文姓名不受影响）
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_codes: HashSet<String> = HashSet::new();

    for row in &outcome.rows {
        let staff = &row.staff;
        if !seen_names.insert(staff.name.to_lowercase()) {
            issues.push(staff_import_issue(
                row.row,
                &staff.name,
                "skipped",
                "同一份文件里姓名重复，已跳过".to_string(),
            ));
            continue;
        }
        let same_name: Option<(String,)> =
            sqlx::query_as("SELECT staff_code FROM staff WHERE name = ?1 LIMIT 1")
                .bind(&staff.name)
                .fetch_optional(&state.db)
                .await?;
        if let Some((code,)) = same_name {
            issues.push(staff_import_issue(
                row.row,
                &staff.name,
                "skipped",
                format!("责任人库里已有同名责任人（{code}），已跳过"),
            ));
            continue;
        }

        let staff_code = match &staff.staff_code {
            Some(code) => {
                if !seen_codes.insert(code.to_lowercase()) {
                    issues.push(staff_import_issue(
                        row.row,
                        &staff.name,
                        "skipped",
                        format!("同一份文件里工号 {code} 重复，已跳过"),
                    ));
                    continue;
                }
                let taken: Option<(String,)> =
                    sqlx::query_as("SELECT name FROM staff WHERE staff_code = ?1 LIMIT 1")
                        .bind(code)
                        .fetch_optional(&state.db)
                        .await?;
                if let Some((owner,)) = taken {
                    issues.push(staff_import_issue(
                        row.row,
                        &staff.name,
                        "skipped",
                        format!("工号 {code} 已被责任人「{owner}」占用，已跳过"),
                    ));
                    continue;
                }
                code.clone()
            }
            None => allocate_staff_code(&state).await?,
        };

        let timestamp = now();
        let result = sqlx::query(
            "INSERT INTO staff (staff_code, name, role, team, phone, color, status, skills,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(&staff_code)
        .bind(&staff.name)
        .bind(&staff.role)
        .bind(&staff.team)
        .bind(&staff.phone)
        .bind(STAFF_COLORS[created_names.len() % STAFF_COLORS.len()])
        .bind(staff.status.clone().unwrap_or_else(|| "在岗".to_string()))
        .bind(&staff.skills)
        .bind(&timestamp)
        .bind(&timestamp)
        .execute(&state.db)
        .await;
        match result {
            Ok(_) => {
                seen_codes.insert(staff_code.to_lowercase());
                created_names.push(staff.name.clone());
            }
            Err(error) => issues.push(staff_import_issue(
                row.row,
                &staff.name,
                "failed",
                format!("写库失败：{error}"),
            )),
        }
    }

    issues.sort_by_key(|issue| issue.row);
    let created = created_names.len() as i64;
    let skipped = issues
        .iter()
        .filter(|issue| issue.status == "skipped")
        .count() as i64;
    let failed = issues
        .iter()
        .filter(|issue| issue.status == "failed")
        .count() as i64;
    Ok(ApiResponse::ok(StaffImportResult {
        total: outcome.total() as i64,
        created,
        skipped,
        failed,
        created_names,
        issues,
    }))
}

/// 导入时给新责任人分配的编号：跟着分配器走（与手动新增一致），
/// 遇到已被手填占用的编号就往后顺延。
pub(crate) async fn allocate_staff_code(state: &AppState) -> Result<String, AppError> {
    let mut next = crate::ids::next_id(&state.db, "staff", "staff").await?;
    loop {
        let code = format!("EMP-{next:02}");
        let taken: Option<i64> = sqlx::query_scalar("SELECT id FROM staff WHERE staff_code = ?1")
            .bind(&code)
            .fetch_optional(&state.db)
            .await?;
        if taken.is_none() {
            return Ok(code);
        }
        next += 1;
    }
}

pub(crate) fn staff_import_issue(
    row: usize,
    name: &str,
    status: &str,
    message: String,
) -> StaffImportRow {
    StaffImportRow {
        row: row as i64,
        name: name.to_string(),
        status: status.to_string(),
        message,
    }
}
/// 校验「关联平台账号」：账号必须存在，且一个账号只能绑一个责任人。
///
/// 唯一性不能只靠数据库索引：撞索引会回落成 500，现场只看到「服务器错误」。
/// `self_id` = 正在编辑的责任人（改自己时不该跟自己冲突）。
pub(crate) async fn check_account_binding(
    state: &AppState,
    user_id: Option<i64>,
    self_id: Option<i64>,
) -> Result<(), AppError> {
    let Some(user_id) = user_id else {
        return Ok(());
    };
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE id = ?1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(AppError::BadRequest(format!("账号 {user_id} 不存在")));
    }
    let bound: Option<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM staff WHERE user_id = ?1 AND id IS NOT ?2 LIMIT 1")
            .bind(user_id)
            .bind(self_id)
            .fetch_optional(&state.db)
            .await?;
    if let Some((staff_id, name)) = bound {
        return Err(AppError::BadRequest(format!(
            "该账号已关联责任人「{name}」（#{staff_id}），一个账号只能绑定一个责任人"
        )));
    }
    Ok(())
}

pub(crate) async fn fetch_staff(state: &AppState, id: i64) -> Result<Staff, AppError> {
    let sql = format!("{STAFF_SELECT} WHERE st.id = ?3");
    sqlx::query_as::<_, Staff>(&sql)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("责任人不存在".to_string()))
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
