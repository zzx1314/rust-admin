use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::models::{
    WorkOrder, WorkOrderAlert, WorkOrderDetail, WorkOrderLog, WorkOrderLogRequest, WorkOrderQuery,
    WorkOrderRequest, WorkOrderStatusRequest,
};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::{Value, json};
use sqlx::QueryBuilder;

const WORK_ORDER_SELECT: &str = "SELECT w.id, w.order_code, w.title, w.order_type, w.source,
    w.source_ref, w.priority, w.staff_id, w.assignee, st.team AS staff_team,
    st.color AS staff_color, w.park_id, p.park_name, w.device_id, d.device_id AS device_code,
    w.deadline, w.status, w.progress, w.description, w.alert_id, w.closed_at,
    w.created_at, w.updated_at, w.created_by
    FROM work_orders w
    LEFT JOIN staff st ON st.id = w.staff_id
    LEFT JOIN parks p ON p.id = w.park_id
    LEFT JOIN devices d ON d.id = w.device_id";

/// 工单状态机：待接单 → 处理中 → 待验收 → 已完成。
/// 看板拖拽允许回退（例如验收不通过退回处理中），回退时进度跟着状态走。
const STATUS_FLOW: [&str; 4] = ["待接单", "处理中", "待验收", "已完成"];

fn default_progress(status: &str) -> i64 {
    match status {
        "处理中" => 30,
        "待验收" => 90,
        "已完成" => 100,
        _ => 0,
    }
}

fn default_note(status: &str) -> &'static str {
    match status {
        "待接单" => "工单退回待接单",
        "处理中" => "已接单，开始现场处理",
        "待验收" => "处理完成，提交验收",
        "已完成" => "验收通过，工单关闭",
        _ => "工单状态变更",
    }
}

fn action_of(status: &str) -> &'static str {
    match status {
        "处理中" => "accept",
        "待验收" => "submit",
        "已完成" => "close",
        _ => "status",
    }
}

pub async fn list_work_orders(
    State(state): State<AppState>,
    Query(query): Query<WorkOrderQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<WorkOrder>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(WORK_ORDER_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND w.status = ").push_bind(status);
    }
    if let Some(priority) = query
        .priority
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND w.priority = ").push_bind(priority);
    }
    if let Some(staff_id) = query.staff_id {
        builder.push(" AND w.staff_id = ").push_bind(staff_id);
    }
    if let Some(park_id) = query.park_id {
        builder.push(" AND w.park_id = ").push_bind(park_id);
    }
    if let Some(keyword) = query.q.filter(|value| !value.trim().is_empty()) {
        let like = format!("%{}%", keyword.trim());
        builder
            .push(" AND (w.title LIKE ")
            .push_bind(like.clone())
            .push(" OR w.order_code LIKE ")
            .push_bind(like.clone())
            .push(" OR w.assignee LIKE ")
            .push_bind(like)
            .push(")");
    }
    let my_staff = my_staff_id(&state, &claims).await?;
    if !claims.is_admin {
        // 非管理员能看到的：
        //   1. 自己建的；
        //   2. 系统自动派单（`created_by` 为空）—— 派单本来就是「派给现场人去干」的，
        //      2026-11-17 现场就因为这里只认创建人，运营账号打开工单管理一片空白；
        //   3. 但**派给已经绑定了别的账号的责任人**的系统单要藏起来，
        //      否则「派给谁」在平台上没有意义（未绑定账号的责任人仍按公共待办）。
        append_visible_filter(&mut builder, &claims, my_staff);
    }
    if query.mine == Some(true) {
        match my_staff {
            Some(staff_id) => {
                builder.push(" AND w.staff_id = ").push_bind(staff_id);
            }
            // 账号没绑责任人：「我的」= 自己建的（否则这个开关点了就空白，更容易误解）
            None => {
                builder.push(" AND w.created_by = ").push_bind(claims.sub);
            }
        }
    }
    builder.push(" ORDER BY w.created_at DESC, w.id DESC");
    let orders = builder
        .build_query_as::<WorkOrder>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(orders))
}

pub async fn work_order_stats(
    State(state): State<AppState>,
    Query(query): Query<WorkOrderQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let mut builder =
        QueryBuilder::<sqlx::Sqlite>::new("SELECT status, COUNT(*) FROM work_orders w WHERE 1 = 1");
    let my_staff = my_staff_id(&state, &claims).await?;
    if !claims.is_admin {
        // 与列表同一套可见性（看板顶部的数字必须和列表条数对得上）
        append_visible_filter(&mut builder, &claims, my_staff);
    }
    if query.mine == Some(true) {
        match my_staff {
            Some(staff_id) => {
                builder.push(" AND w.staff_id = ").push_bind(staff_id);
            }
            None => {
                builder.push(" AND w.created_by = ").push_bind(claims.sub);
            }
        }
    }
    builder.push(" GROUP BY w.status");
    let rows: Vec<(String, i64)> = builder.build_query_as().fetch_all(&state.db).await?;
    let total: i64 = rows.iter().map(|(_, count)| count).sum();
    let mut counts = serde_json::Map::new();
    for (status, count) in &rows {
        counts.insert(status.clone(), json!(count));
    }
    let done = counts.get("已完成").and_then(Value::as_i64).unwrap_or(0);
    Ok(ApiResponse::ok(json!({
        "total": total,
        "counts": counts,
        "open": total - done,
    })))
}

pub async fn get_work_order(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<WorkOrderDetail>>, AppError> {
    let order = fetch_order(&state, id).await?;
    ensure_order_access(&state, &order, &claims).await?;
    let logs = fetch_logs(&state, id).await?;
    let alerts = fetch_alerts(&state, id).await?;
    Ok(ApiResponse::ok(WorkOrderDetail {
        order,
        logs,
        alerts,
    }))
}

pub async fn create_work_order(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<WorkOrderRequest>,
) -> Result<Json<ApiResponse<WorkOrderDetail>>, AppError> {
    // 页面上手动「转工单」算人工接手：原告警同步推进到「处理中」
    let order =
        create_order_core(&state, payload, Some(claims.sub), &claims.username, true).await?;
    let logs = fetch_logs(&state, order.id).await?;
    let alerts = fetch_alerts(&state, order.id).await?;
    Ok(ApiResponse::ok(WorkOrderDetail {
        order,
        logs,
        alerts,
    }))
}

/// 建工单的核心逻辑：网页新建、「告警转工单」与「告警自动派单」共用，避免三套默认值。
///
/// `created_by` 可为空：自动派单没有登录用户，工单的 `created_by` 落 NULL，
/// 处理轨迹里的操作人写「系统」。
///
/// `promote_alert`：建单时要不要把原告警从「未处理」推进到「处理中」。
/// 人工「转工单」= 已接手 → true；**告警自动派单 → false**，因为工单还停在
/// 「待接单」，责任人没接单前告警仍然是未处理，不能被数量统计悄悄吞掉。
pub(crate) async fn create_order_core(
    state: &AppState,
    payload: WorkOrderRequest,
    created_by: Option<i64>,
    actor: &str,
    promote_alert: bool,
) -> Result<WorkOrder, AppError> {
    let title = payload.title.trim();
    if title.is_empty() {
        return Err(AppError::BadRequest("工单标题不能为空".to_string()));
    }
    // 一条告警只派一单：自动派单已经建过单时，页面上再点「转工单」直接给回原文号，
    // 否则同一件事会在看板上出现两张单，责任人也不知道该关哪张。
    if let Some(alert_id) = payload.alert_id
        && let Some((order_code,)) = sqlx::query_as::<_, (String,)>(
            "SELECT w.order_code FROM alerts a
               JOIN work_orders w ON w.id = a.work_order_id
              WHERE a.id = ?1",
        )
        .bind(alert_id)
        .fetch_optional(&state.db)
        .await?
    {
        return Err(AppError::BadRequest(format!(
            "该告警已派单（{order_code}），请到工单看板处理"
        )));
    }
    // 责任人：优先 staffId，其次姓名；姓名能对上责任人库时补 staffId
    let (staff_id, assignee) = match (payload.staff_id, payload.assignee.as_deref()) {
        (Some(staff_id), _) => {
            let name: Option<String> = sqlx::query_scalar("SELECT name FROM staff WHERE id = ?1")
                .bind(staff_id)
                .fetch_optional(&state.db)
                .await?;
            let Some(name) = name else {
                return Err(AppError::NotFound(format!("责任人 {staff_id} 不存在")));
            };
            (Some(staff_id), name)
        }
        (None, Some(assignee)) if !assignee.trim().is_empty() => {
            let name = assignee.trim().to_string();
            let matched = super::schedules::staff_id_by_name(&state.db, &name).await?;
            (matched, name)
        }
        _ => {
            return Err(AppError::BadRequest("请选择处理责任人".to_string()));
        }
    };

    // 时限缺省：2 小时内（与原型「紧急单 1 小时内闭环」的直觉一致）
    let deadline = payload
        .deadline
        .filter(|value| !value.trim().is_empty())
        .or_else(|| Some(default_deadline()));
    let source = payload
        .source
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "手动创建".to_string());

    let next_id = crate::ids::next_id(&state.db, "work_orders", "work_orders").await?;
    let order_code = format!("WO-{}-{next_id:02}", chrono::Local::now().format("%m%d"));
    let timestamp = now();
    let status = "待接单";
    let result = sqlx::query(
        "INSERT INTO work_orders (order_code, title, order_type, source, source_ref, priority,
            staff_id, assignee, park_id, device_id, deadline, status, progress, description,
            alert_id, created_at, updated_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0, ?13, ?14, ?15, ?15, ?16)",
    )
    .bind(&order_code)
    .bind(title)
    .bind(payload.order_type.unwrap_or_else(|| "告警处置".to_string()))
    .bind(&source)
    .bind(payload.source_ref)
    .bind(payload.priority.unwrap_or_else(|| "中".to_string()))
    .bind(staff_id)
    .bind(&assignee)
    .bind(payload.park_id)
    .bind(payload.device_id)
    .bind(&deadline)
    .bind(status)
    .bind(payload.description)
    .bind(payload.alert_id)
    .bind(&timestamp)
    .bind(created_by)
    .execute(&state.db)
    .await?;
    let id = result.last_insert_rowid();

    let content = if payload.alert_id.is_some() {
        format!("由{source}转人工单，指派 {assignee}")
    } else {
        format!("创建工单并指派 {assignee}")
    };
    insert_log(state, id, actor, "created", &content).await?;

    // 告警转工单：原告警从「未处理」推进到「处理中」，两边状态不再互相打架
    if let Some(alert_id) = payload.alert_id.filter(|_| promote_alert) {
        sqlx::query("UPDATE alerts SET status = '处理中' WHERE id = ?1 AND status = '未处理'")
            .bind(alert_id)
            .execute(&state.db)
            .await?;
    }

    fetch_order(state, id).await
}

pub async fn update_work_order(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<WorkOrderRequest>,
) -> Result<Json<ApiResponse<WorkOrderDetail>>, AppError> {
    let existing = fetch_order(&state, id).await?;
    ensure_order_access(&state, &existing, &claims).await?;

    // 改派：显式给 staffId 就按 id 取姓名；只给姓名时按姓名反查责任人库
    let (staff_id, assignee) = match (payload.staff_id, payload.assignee.as_deref()) {
        (None, None) => (existing.staff_id, existing.assignee.clone()),
        (Some(staff_id), _) => {
            let name: Option<String> = sqlx::query_scalar("SELECT name FROM staff WHERE id = ?1")
                .bind(staff_id)
                .fetch_optional(&state.db)
                .await?;
            let Some(name) = name.filter(|name| !name.trim().is_empty()) else {
                return Err(AppError::NotFound(format!("责任人 {staff_id} 不存在")));
            };
            (Some(staff_id), name)
        }
        (None, Some(assignee)) => {
            let name = assignee.trim().to_string();
            if name.is_empty() {
                return Err(AppError::BadRequest("处理责任人不能为空".to_string()));
            }
            let matched = super::schedules::staff_id_by_name(&state.db, &name).await?;
            (matched, name)
        }
    };

    sqlx::query(
        "UPDATE work_orders SET title = ?1, order_type = ?2, priority = ?3, staff_id = ?4,
            assignee = ?5, park_id = ?6, device_id = ?7, deadline = ?8, description = ?9,
            updated_at = ?10 WHERE id = ?11",
    )
    .bind(if payload.title.trim().is_empty() {
        existing.title
    } else {
        payload.title.trim().to_string()
    })
    .bind(payload.order_type.unwrap_or(existing.order_type))
    .bind(payload.priority.unwrap_or(existing.priority))
    .bind(staff_id)
    .bind(&assignee)
    .bind(payload.park_id.or(existing.park_id))
    .bind(payload.device_id.or(existing.device_id))
    .bind(payload.deadline.or(existing.deadline))
    .bind(payload.description.or(existing.description))
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;

    if assignee != existing.assignee {
        insert_log(
            &state,
            id,
            &claims.username,
            "status",
            &format!("改派给 {assignee}（原处理人 {}）", existing.assignee),
        )
        .await?;
    }

    let order = fetch_order(&state, id).await?;
    let logs = fetch_logs(&state, id).await?;
    let alerts = fetch_alerts(&state, id).await?;
    Ok(ApiResponse::ok(WorkOrderDetail {
        order,
        logs,
        alerts,
    }))
}

/// 状态流转：详情页按钮与看板拖拽都走这里。
pub async fn update_work_order_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<WorkOrderStatusRequest>,
) -> Result<Json<ApiResponse<WorkOrderDetail>>, AppError> {
    let existing = fetch_order(&state, id).await?;
    ensure_order_access(&state, &existing, &claims).await?;

    let status = payload.status.trim().to_string();
    if !STATUS_FLOW.contains(&status.as_str()) {
        return Err(AppError::BadRequest(format!("未知工单状态：{status}")));
    }
    if status == existing.status && payload.note.is_none() {
        let logs = fetch_logs(&state, id).await?;
        let alerts = fetch_alerts(&state, id).await?;
        return Ok(ApiResponse::ok(WorkOrderDetail {
            order: existing,
            logs,
            alerts,
        }));
    }

    let progress = payload
        .progress
        .unwrap_or_else(|| default_progress(&status));
    let closed_at = if status == "已完成" {
        Some(now())
    } else {
        None
    };
    sqlx::query(
        "UPDATE work_orders SET status = ?1, progress = ?2, closed_at = ?3, updated_at = ?4
         WHERE id = ?5",
    )
    .bind(&status)
    .bind(progress)
    .bind(&closed_at)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;

    let actor = payload
        .actor
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| claims.username.clone());
    let note = payload
        .note
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default_note(&status).to_string());
    insert_log(&state, id, &actor, action_of(&status), &note).await?;

    let order = fetch_order(&state, id).await?;
    let logs = fetch_logs(&state, id).await?;
    let alerts = fetch_alerts(&state, id).await?;
    Ok(ApiResponse::ok(WorkOrderDetail {
        order,
        logs,
        alerts,
    }))
}

pub async fn add_work_order_log(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<WorkOrderLogRequest>,
) -> Result<Json<ApiResponse<WorkOrderDetail>>, AppError> {
    let order = fetch_order(&state, id).await?;
    ensure_order_access(&state, &order, &claims).await?;
    if payload.content.trim().is_empty() {
        return Err(AppError::BadRequest("处理记录不能为空".to_string()));
    }
    let actor = payload
        .actor
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| claims.username.clone());
    insert_log(&state, id, &actor, "comment", payload.content.trim()).await?;
    sqlx::query("UPDATE work_orders SET updated_at = ?1 WHERE id = ?2")
        .bind(now())
        .bind(id)
        .execute(&state.db)
        .await?;
    let order = fetch_order(&state, id).await?;
    let logs = fetch_logs(&state, id).await?;
    let alerts = fetch_alerts(&state, id).await?;
    Ok(ApiResponse::ok(WorkOrderDetail {
        order,
        logs,
        alerts,
    }))
}

pub async fn delete_work_order(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let order = fetch_order(&state, id).await?;
    ensure_can_delete(order.created_by, &claims)?;
    // 自动派单会在 `alerts.work_order_id` 上指回来：先解除引用再删本单，
    // 否则撞 FOREIGN KEY（告警行本身保留，`assignee` / `assignee_note` 仍是当时的记录）。
    // logs 由外键 ON DELETE CASCADE 一并清掉（连接已开启 foreign_keys）
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE alerts SET work_order_id = NULL WHERE work_order_id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let result = sqlx::query("DELETE FROM work_orders WHERE id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

// ------------------------------------------------------------------ 内部

/// 工单可读可处置：管理员 / 自己建的 / **系统自动派单的（`created_by` 为空）**。
///
/// 系统单是「告警自动派单」建的，`created_by` 落 NULL（派单人是「系统」，没有登录用户）。
/// 2026-11-17 现场：`zhangzexin`（中台 role=common → `is_admin=false`）在列表里能看到
/// `WO-1117-29` 是排班派给他的单，点开却报「工单不存在」—— 因为这里只认创建人。
/// 系统派单本质上是**公共待办**：告警中心本来就对所有人可见，工单不能再藏起来，
/// 否则「排班派给谁」这条链路在平台上就断了。
/// 当前登录账号绑定的责任人（`staff.user_id = claims.sub`）。
async fn my_staff_id(state: &AppState, claims: &Claims) -> Result<Option<i64>, AppError> {
    Ok(
        sqlx::query_scalar("SELECT id FROM staff WHERE user_id = ?1")
            .bind(claims.sub)
            .fetch_optional(&state.db)
            .await?,
    )
}

/// 非管理员在列表/统计里的可见范围。
///
/// - 自己建的；
/// - 系统自动派单（`created_by` 为空）；
/// - 但派给「已经绑定了别的账号」的责任人的系统单不给他看（否则「派给谁」没意义）；
/// - 派给未绑定账号的责任人的单仍算公共待办（现场可能是外包 / 临时工，没有平台账号）。
fn append_visible_filter(
    builder: &mut QueryBuilder<'_, sqlx::Sqlite>,
    claims: &Claims,
    my_staff: Option<i64>,
) {
    if let Some(staff_id) = my_staff {
        // (自己建的) OR (系统派单 AND (未指派 OR 派给我 OR 派给没绑账号的人))
        builder
            .push(" AND (w.created_by = ")
            .push_bind(claims.sub)
            .push(" OR (w.created_by IS NULL AND (w.staff_id IS NULL OR w.staff_id = ")
            .push_bind(staff_id)
            .push(" OR w.staff_id IN (SELECT id FROM staff WHERE user_id IS NULL))))");
    } else {
        // 还没绑责任人：保持老口径（自己建的 + 全部系统派单），否则一升级就全空白
        builder
            .push(" AND (w.created_by = ")
            .push_bind(claims.sub)
            .push(" OR w.created_by IS NULL)");
    }
}

/// 这张单能不能由当前账号处置。
///
/// - 管理员 / 创建人：可以；
/// - 系统派单（`created_by` 为空）：派给他自己 → 可以；派给没绑定账号的责任人 → 也算公共待办；
///   派给人家的单（对方已绑定账号）→ 不行，去「只看我负责的」里那条才是他的。
fn ensure_visible(
    created_by: Option<i64>,
    staff_id: Option<i64>,
    assigned_user: Option<i64>,
    claims: &Claims,
    my_staff: Option<i64>,
) -> Result<(), AppError> {
    if claims.is_admin || created_by == Some(claims.sub) {
        return Ok(());
    }
    if created_by.is_none() {
        let public_pool =
            staff_id.is_none() || Some(staff_id.unwrap()) == my_staff || assigned_user.is_none();
        if public_pool {
            return Ok(());
        }
    }
    Err(AppError::NotFound("工单不存在".to_string()))
}

/// 取这张单的责任人绑定信息后做一次处置权限判断（每次处置只需一条极小的查询）。
async fn ensure_order_access(
    state: &AppState,
    order: &WorkOrder,
    claims: &Claims,
) -> Result<(), AppError> {
    let assigned_user: Option<Option<i64>> = match order.staff_id {
        Some(staff_id) => {
            sqlx::query_scalar("SELECT user_id FROM staff WHERE id = ?1")
                .bind(staff_id)
                .fetch_optional(&state.db)
                .await?
        }
        None => None,
    };
    let my_staff = my_staff_id(state, claims).await?;
    ensure_visible(
        order.created_by,
        order.staff_id,
        assigned_user.flatten(),
        claims,
        my_staff,
    )
}

/// 删除比处置更严格：系统单是告警的存证链，只允许管理员删。
fn ensure_can_delete(created_by: Option<i64>, claims: &Claims) -> Result<(), AppError> {
    if claims.is_admin || created_by == Some(claims.sub) {
        return Ok(());
    }
    Err(AppError::NotFound("工单不存在".to_string()))
}

fn default_deadline() -> String {
    (chrono::Local::now() + chrono::Duration::hours(2))
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

async fn fetch_order(state: &AppState, id: i64) -> Result<WorkOrder, AppError> {
    let sql = format!("{WORK_ORDER_SELECT} WHERE w.id = ?1");
    sqlx::query_as::<_, WorkOrder>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("工单不存在".to_string()))
}

async fn fetch_logs(state: &AppState, id: i64) -> Result<Vec<WorkOrderLog>, AppError> {
    let logs = sqlx::query_as::<_, WorkOrderLog>(
        "SELECT id, work_order_id, actor, action, content, created_at
         FROM work_order_logs WHERE work_order_id = ?1 ORDER BY id ASC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(logs)
}

/// 这张工单挂着的全部告警（自动派单 = 告警建单，同类告警会持续挂到同一张单上）
async fn fetch_alerts(state: &AppState, id: i64) -> Result<Vec<WorkOrderAlert>, AppError> {
    let alerts = sqlx::query_as::<_, WorkOrderAlert>(
        "SELECT id, alert_code, level, content, status, occurred_at
           FROM alerts WHERE work_order_id = ?1 ORDER BY id ASC",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(alerts)
}

async fn insert_log(
    state: &AppState,
    id: i64,
    actor: &str,
    action: &str,
    content: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO work_order_logs (work_order_id, actor, action, content, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(id)
    .bind(actor)
    .bind(action)
    .bind(content)
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(())
}
