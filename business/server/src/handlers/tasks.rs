use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::gateway;
use crate::gateway_types::TaskControlPayload;
use crate::models::{Task, TaskQuery, TaskRequest, TaskStatusRequest};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tracing::{debug, info, warn};

/// 任务行 + 关联名称的公共列清单（服务端各处 SELECT 与集成测试共用，避免漏列）
pub const TASK_SELECT: &str = "SELECT t.id, t.task_code, t.task_name, t.task_type, t.execute_mode,
    t.end_task, t.light_night_only,
    t.level, t.device_id, d.device_id AS device_code, t.park_id, p.park_name, t.zone_id,
    z.zone_name, t.route_id, r.route_code, r.route_name, t.plan_start, t.plan_end,
    t.schedule_days, t.last_dispatched_on, t.publish_status, t.published_at,
    t.task_status, t.progress, t.dispatch_policy, t.description, t.robot_task_id,
    t.robot_stage, t.robot_updated_at, t.created_at, t.updated_at, t.created_by
    FROM tasks t
    LEFT JOIN devices d ON d.id = t.device_id
    LEFT JOIN parks p ON p.id = t.park_id
    LEFT JOIN zones z ON z.id = t.zone_id
    LEFT JOIN routes r ON r.id = t.route_id";

/// 发布状态入库前的归一化与校验。
///
/// 认不出来的值直接 400：静默当成「已发布」会让一条本该先存着的计划立刻开跑，
/// 静默当成「草稿」会让用户以为计划已经生效、实际永远不跑——两种都比报错难查。
fn normalize_publish_status(value: Option<String>) -> Result<String, AppError> {
    let Some(raw) = value else {
        return Ok(crate::schedule::PUBLISHED.to_string());
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(crate::schedule::PUBLISHED.to_string());
    }
    if !crate::schedule::is_valid_publish_status(raw) {
        return Err(AppError::BadRequest(format!(
            "无效的发布状态「{raw}」；可选：{} / {}",
            crate::schedule::DRAFT,
            crate::schedule::PUBLISHED
        )));
    }
    Ok(raw.to_string())
}

/// 任务只能下发给已绑定园区的设备，且任务园区必须与设备园区一致。
async fn validate_device_park(
    db: &SqlitePool,
    device_id: Option<i64>,
    park_id: Option<i64>,
) -> Result<(), AppError> {
    let Some(device_id) = device_id else {
        return Ok(());
    };
    let device_park: Option<Option<i64>> =
        sqlx::query_scalar("SELECT park_id FROM devices WHERE id = ?1")
            .bind(device_id)
            .fetch_optional(db)
            .await?;
    match device_park {
        None => Err(AppError::NotFound(format!("设备 {device_id} 不存在"))),
        Some(None) => Err(AppError::BadRequest(
            "设备尚未绑定园区，无法下发任务".to_string(),
        )),
        Some(Some(bound_park)) => match park_id {
            Some(park_id) if park_id != bound_park => Err(AppError::BadRequest(
                "任务园区与设备绑定的园区不一致".to_string(),
            )),
            _ => Ok(()),
        },
    }
}

/// 结束动作入库前的归一化与校验。
///
/// 认不出来就直接 400：静默当成「跟随路线」会让用户以为选了「原地」，
/// 实际跑完还是返航，现场排查成本很高。
fn normalize_task_end_task(value: Option<String>) -> Result<Option<String>, AppError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    crate::dispatch::normalize_end_task(&raw)
        .map(|normalized| Some(normalized.to_string()))
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "无效的结束动作「{raw}」；可选：不传 = 跟随路线 / none = 原地待命 / \
                 return_to_start = 返回起点 / return_to_charge = 返回充电点"
            ))
        })
}

pub async fn list_tasks(
    State(state): State<AppState>,
    Query(query): Query<TaskQuery>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<Task>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(TASK_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(status) = query
        .task_status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND t.task_status = ").push_bind(status);
    }
    if let Some(device_id) = query.device_id {
        builder.push(" AND t.device_id = ").push_bind(device_id);
    }
    if let Some(park_id) = query.park_id {
        builder.push(" AND t.park_id = ").push_bind(park_id);
    }
    if let Some(route_id) = query.route_id {
        builder.push(" AND t.route_id = ").push_bind(route_id);
    }
    if !claims.is_admin {
        builder.push(" AND t.created_by = ").push_bind(claims.sub);
    }
    builder.push(" ORDER BY t.created_at DESC, t.id DESC");
    let tasks: Vec<Task> = builder
        .build_query_as::<crate::models::TaskRow>()
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(Task::from)
        .collect();
    Ok(ApiResponse::ok(tasks))
}

pub async fn task_stats(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    // 不过滤 / 按创建人过滤共用一个查询，用绑定参数而不是把 id 拼进 SQL
    let mut builder = QueryBuilder::<Sqlite>::new("SELECT task_status, COUNT(*) FROM tasks");
    if !claims.is_admin {
        builder.push(" WHERE created_by = ").push_bind(claims.sub);
    }
    builder.push(" GROUP BY task_status");
    let rows: Vec<(String, i64)> = builder.build_query_as().fetch_all(&state.db).await?;
    let total: i64 = rows.iter().map(|(_, count)| count).sum();
    let mut counts = serde_json::Map::new();
    for (status, count) in rows {
        counts.insert(status, json!(count));
    }
    let done = counts
        .get(crate::status::DONE)
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let rate = if total == 0 {
        0
    } else {
        (done * 100 / total) as i64
    };
    Ok(ApiResponse::ok(json!({
        "total": total,
        "counts": counts,
        "doneRate": rate,
    })))
}

pub async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let task = fetch_task(&state, id).await?;
    super::parks::ensure_owner(&task, &claims)?;
    Ok(ApiResponse::ok(task))
}

pub async fn create_task(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<TaskRequest>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let task = create_task_core(&state, payload, Some(claims.sub)).await?;
    Ok(ApiResponse::ok(task))
}

/// 建任务并尝试立即下发（设备已绑定且在线时）。
///
/// 网页端与中台内部接口共用，避免「网页建任务」与「中台下发任务」两套语义。
pub async fn create_task_core(
    state: &AppState,
    payload: TaskRequest,
    created_by: Option<i64>,
) -> Result<Task, AppError> {
    if payload.task_name.trim().is_empty() {
        return Err(AppError::BadRequest("任务名称不能为空".to_string()));
    }
    // 定时巡检的时间窗是调度依据，不是展示字段：周期任务必须带合法起止时间
    crate::schedule::validate_window(
        payload.execute_mode.as_deref(),
        payload.plan_start.as_deref(),
        payload.plan_end.as_deref(),
        payload.schedule_days.as_deref(),
    )?;
    let schedule_days = crate::schedule::normalize_days(payload.schedule_days.as_deref());
    // 中台用 datetime 选择器填时间，业务侧时间窗只认 `HH:MM`，入库前统一
    let plan_start = crate::schedule::normalize_clock(payload.plan_start.clone());
    let plan_end = crate::schedule::normalize_clock(payload.plan_end.clone());
    validate_device_park(&state.db, payload.device_id, payload.park_id).await?;
    // 坐标预检放在插入之前：路线航点不能下发时直接 400，避免留下一条「建好了但发不出去」的任务
    crate::dispatch::ensure_route_dispatchable(
        &state.db,
        payload.route_id.flatten(),
        payload.park_id,
    )
    .await?;
    // 编号走原子分配器（见 `ids` 模块）：`MAX(id)+1` 并发下会拿重号
    let next_id = crate::ids::next_id(&state.db, "tasks", "tasks").await?;
    let task_code = payload
        .task_code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("TK-{}-{next_id:02}", chrono::Local::now().format("%m%d")));
    let end_task = normalize_task_end_task(payload.end_task.flatten())?;
    // 计划级「仅夜间自动开灯」：只有航点动作里配了 lightFill / lightLed 才起作用
    let light_night_only = payload.light_night_only.unwrap_or(false);
    // 计划发布状态：只有「已发布」的计划才会被自动执行（`schedule::is_published`）
    let publish_status = normalize_publish_status(payload.publish_status.clone())?;
    // 执行状态：以前直接把请求里的字符串写库，写坏一个值之后所有
    // `matches!(task_status, …)` 都会静默走「非终态」分支，这里挡在建单之前
    let task_status = payload
        .task_status
        .unwrap_or_else(|| crate::status::PENDING.to_string());
    if !crate::status::is_valid(&task_status) {
        return Err(AppError::BadRequest(format!(
            "无效的任务状态：{task_status}"
        )));
    }
    let timestamp = now();
    let published_at = (publish_status == crate::schedule::PUBLISHED).then(|| timestamp.clone());
    let result = sqlx::query(
        "INSERT INTO tasks (task_code, task_name, task_type, execute_mode, end_task, level, device_id,
            park_id, zone_id, route_id, plan_start, plan_end, schedule_days, task_status, progress,
            dispatch_policy, description, created_at, updated_at, created_by,
            progress_updated_at, publish_status, published_at, light_night_only)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, ?18, ?21, ?22, ?23)",
    )
    .bind(&task_code)
    .bind(payload.task_name.trim())
    .bind(payload.task_type)
    .bind(payload.execute_mode)
    .bind(&end_task)
    .bind(payload.level.unwrap_or_else(|| "常规".to_string()))
    .bind(payload.device_id)
    .bind(payload.park_id)
    .bind(payload.zone_id)
    .bind(payload.route_id.flatten())
    .bind(plan_start)
    .bind(plan_end)
    .bind(schedule_days)
    .bind(task_status)
    .bind(payload.progress.unwrap_or(0))
    .bind(payload.dispatch_policy)
    .bind(payload.description)
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(created_by)
    .bind(&publish_status)
    .bind(&published_at)
    .bind(light_night_only)
    .execute(&state.db)
    .await?;
    let id = result.last_insert_rowid();
    record_event(
        state,
        "created",
        format!("创建任务：{}", payload.task_name.trim()),
        format!("任务已加入待执行队列（发布状态：{publish_status}）"),
        payload.device_id,
        Some(id),
    )
    .await?;

    // 如果任务已分配到设备，尝试通过 WebSocket 推送。
    //
    // 定时巡检先看时间窗：没到点就只排期，交给 `schedule::sweep` 到点下发。
    // 以前这里无条件现推，「计划时间 09:30」的任务刚建好（03:00）就跑去执行了。
    //
    // 先过 ready 门禁：设备刚注册、本体还没连上时就把任务推出去，客户端只能回
    // 「未连接机器人本体」，任务在平台上看起来就是「已下发但不动」。此时保持
    // 待执行，等心跳确认本体就绪后由补发逻辑自动下发。
    let task = fetch_task(state, id).await?;
    let now = chrono::Local::now();
    // 草稿只保存、不自动下发（建计划顺手下发是最容易漏掉的一条自动路径）：
    // 发布接口（`POST /tasks/{id}/publish`）把状态翻成已发布后，扫描器 30s 内接管。
    if !crate::schedule::is_published(&task) {
        record_event(
            state,
            "schedule",
            format!("计划 {} 已存为草稿", task.task_code),
            crate::schedule::schedule_hint(&task, now),
            task.device_id,
            Some(id),
        )
        .await?;
        return Ok(task);
    }
    if payload.device_id.is_some() {
        if matches!(
            crate::schedule::due_state(&task, now),
            crate::schedule::DueState::Waiting
                | crate::schedule::DueState::Missed
                | crate::schedule::DueState::Draft
        ) {
            record_event(
                state,
                "schedule",
                format!("任务 {} 已排期", task.task_code),
                crate::schedule::schedule_hint(&task, now),
                task.device_id,
                Some(id),
            )
            .await?;
            return fetch_task(state, id).await;
        }
        let blocked = match task.device_id {
            Some(device_pk) => crate::dispatch::device_block_reason(&state.db, device_pk).await?,
            None => None,
        };
        if let Some(reason) = blocked {
            warn!(
                "task {} 暂不下发（保持待执行，设备就绪后补发）：{reason}",
                task.task_code
            );
        } else {
            match push_task_dispatch(state, &task).await? {
                Some(device_code) => info!(
                    "task {} pushed via WebSocket to device {}",
                    task.task_code, device_code
                ),
                None => warn!(
                    "task {} 未下发：设备未连接 WebSocket，等设备上线补发",
                    task.task_code
                ),
            }
        }
    }

    fetch_task(state, id).await
}

/// 任务是否已经在本体侧跑起来（只有这类任务能靠 pause / resume 控制）。
///
/// `robot_task_id` 由客户端从智航 5009 快照回传；为空说明本体从未受理过这条任务，
/// 此时发 `task_control resume` 毫无意义（客户端没有执行上下文，只会回一条
/// 「执行中 / 0%」然后彻底静默）。
fn resumable(task: &Task) -> bool {
    task.robot_task_id.is_some()
        && matches!(
            task.task_status.as_str(),
            crate::status::RUNNING | crate::status::PAUSED
        )
}

/// 组装载荷并真正下发给设备，成功后把任务 / 设备切到执行态。
///
/// 返回设备编号：`Some(code)` 已下发，`None` 设备未连接（由调用方决定是否报错）。
/// 这是唯一会给客户端发 `task_dispatch`（含航点 / 地图的完整载荷）的 REST 入口。
pub(crate) async fn push_task_dispatch(
    state: &AppState,
    task: &Task,
) -> Result<Option<String>, AppError> {
    let Some(device_pk) = task.device_id else {
        return Ok(None);
    };
    let device_code: Option<String> =
        sqlx::query_scalar("SELECT device_id FROM devices WHERE id = ?1")
            .bind(device_pk)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    let Some(device_code) = device_code else {
        return Ok(None);
    };

    let dispatch = crate::dispatch::build_dispatch_payload(&state.db, task).await?;
    // 排障：把发往设备的整份 `task_dispatch` 载荷留痕（含 routeData / map / speedLimit）。
    // 服务端默认过滤器是 `robodog_ops_server=debug`，不用额外配就能看到。
    // 想核对「本体实际收到的 5000 长什么样」时，再去客户端看 `→ 智航完整帧`。
    debug!(
        payload = %serde_json::to_string(&dispatch).unwrap_or_default(),
        "task_dispatch 下发载荷"
    );
    if let Some(map) = dispatch.map.as_ref() {
        // 现场排障关键信息：业务地图 ID 与本体目录名是两回事
        info!(
            "任务 {} 绑定地图 {} → 本体目录 {}",
            task.task_code, map.map_id, map.map_dir
        );
    }

    // **先置执行态再下发**：客户端拒单（未连接本体 / 地图不在本体 / 定位未就绪）
    // 往往在几毫秒内就回报。以前是发完再 UPDATE，异常会被这条 UPDATE 盖回
    // 「执行中」，而 updated_at 又刚刷新，看门狗要再等满超时才会判异常。
    // 重新下发等于换一条本体任务：旧的 robot_task_id / stage 必须清掉，
    // 否则下一次「执行中」会被误判成可 resume 的老任务。
    let previous_status = task.task_status.clone();
    let previous_progress = task.progress;
    let timestamp = now();
    sqlx::query(
        "UPDATE tasks SET task_status = '执行中', progress = 0, progress_updated_at = ?1,
            robot_task_id = NULL, robot_stage = NULL, robot_updated_at = NULL, updated_at = ?1
         WHERE id = ?2",
    )
    .bind(&timestamp)
    .bind(task.id)
    .execute(&state.db)
    .await?;

    if !gateway::dispatch_task_to_device(&state.device_connections, &device_code, &dispatch).await {
        // 没发出去：回滚成下发前的状态。带上「仍是执行中且进度为 0」的条件，
        // 客户端如果已经回报了异常/已完成，就不要覆盖真实结果。
        sqlx::query(
            "UPDATE tasks SET task_status = ?1, progress = ?2, updated_at = ?3
             WHERE id = ?4 AND task_status = '执行中' AND progress = 0",
        )
        .bind(&previous_status)
        .bind(previous_progress)
        .bind(now())
        .bind(task.id)
        .execute(&state.db)
        .await?;
        return Ok(None);
    }

    sqlx::query(
        "UPDATE devices SET status = '执行任务', current_task_name = ?1, updated_at = ?2
         WHERE id = ?3",
    )
    .bind(&task.task_name)
    .bind(&timestamp)
    .bind(device_pk)
    .execute(&state.db)
    .await?;

    // 记下「今天这个窗口已经跑过」：周期任务否则会在窗口内反复下发（跑完回到待执行
    // 又被扫描器当成到点）。手动开始也走这里，所以手动跑一次同样不会被自动重跑。
    sqlx::query("UPDATE tasks SET last_dispatched_on = ?1 WHERE id = ?2")
        .bind(chrono::Local::now().format("%Y-%m-%d").to_string())
        .bind(task.id)
        .execute(&state.db)
        .await?;

    // updated_at 已刷新，任务看门狗从这里开始计时
    Ok(Some(device_code))
}

/// 下发任务；设备不在线时返回可读错误，绝不把状态单独改成「执行中」。
async fn dispatch_task_or_error(state: &AppState, task: &Task) -> Result<(), AppError> {
    if task.task_status == crate::status::DONE {
        return Err(AppError::BadRequest(format!(
            "任务 {} 已完成，如需重跑请新建任务",
            task.task_code
        )));
    }
    let Some(device_pk) = task.device_id else {
        return Err(AppError::BadRequest(format!(
            "任务 {} 尚未分配设备，无法下发",
            task.task_code
        )));
    };
    // 任务园区必须与设备绑定的园区一致，否则会载错地图
    validate_device_park(&state.db, task.device_id, task.park_id).await?;
    let device_code: Option<String> =
        sqlx::query_scalar("SELECT device_id FROM devices WHERE id = ?1")
            .bind(device_pk)
            .fetch_optional(&state.db)
            .await?
            .flatten();
    let Some(device_code) = device_code else {
        return Err(AppError::NotFound(format!("设备 {device_pk} 不存在")));
    };
    // 本体节点 / 急停 / 控制权 / 远程接管：与补发路径共用同一套门禁
    // （`dispatch::device_block_reason`），避免两处判据漂移。
    if let Some(reason) = crate::dispatch::device_block_reason(&state.db, device_pk).await? {
        return Err(AppError::BadRequest(reason));
    }

    match push_task_dispatch(state, task).await? {
        Some(code) => {
            record_event(
                state,
                "dispatch",
                format!("任务 {} 已下发", task.task_code),
                format!("已下发到设备 {code}"),
                task.device_id,
                Some(task.id),
            )
            .await?;
            Ok(())
        }
        None => Err(AppError::BadRequest(format!(
            "设备 {device_code} 未连接（离线或未与业务平台建立 WebSocket），任务未下发"
        ))),
    }
}

/// `POST /tasks/{id}/dispatch` — 真正把任务推给设备（前端「开始执行 / 重新下发」）。
///
/// 与 `PUT /tasks/{id}/status` 的区别：这里会发 `task_dispatch`（带航点 / 地图），
/// 设备离线时直接报错，不会留下一个假的「执行中」。
pub async fn dispatch_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let task = fetch_task(&state, id).await?;
    super::parks::ensure_owner(&task, &claims)?;
    dispatch_task_or_error(&state, &task).await?;
    Ok(ApiResponse::ok(fetch_task(&state, id).await?))
}

/// `POST /tasks/{id}/publish` —— 发布计划：此后才会到点自动执行。
///
/// 发布前把「跑不起来」的原因当场报出来（周期任务没有合法时间窗、路线坐标不能下发），
/// 而不是让一条已发布的计划在扫描器里永远 `Waiting`。
pub async fn publish_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let task = fetch_task(&state, id).await?;
    super::parks::ensure_owner(&task, &claims)?;
    if crate::schedule::is_published(&task) {
        return Err(AppError::BadRequest(format!(
            "计划 {} 已经是「已发布」状态",
            task.task_code
        )));
    }
    crate::schedule::validate_window(
        task.execute_mode.as_deref(),
        task.plan_start.as_deref(),
        task.plan_end.as_deref(),
        task.schedule_days.as_deref(),
    )?;
    // 发布即意味着会被自动下发：路线坐标不合法时宁可现在拦住，也别等设备上线才失败
    crate::dispatch::ensure_route_dispatchable(&state.db, task.route_id, task.park_id).await?;

    sqlx::query(
        "UPDATE tasks SET publish_status = ?1, published_at = ?2, updated_at = ?2 WHERE id = ?3",
    )
    .bind(crate::schedule::PUBLISHED)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;

    let updated = fetch_task(&state, id).await?;
    let hint = crate::schedule::schedule_hint(&updated, chrono::Local::now());
    record_event(
        &state,
        "published",
        format!("计划 {} 已发布", updated.task_code),
        format!("{hint}（人工「开始执行」不受发布状态限制）"),
        updated.device_id,
        Some(id),
    )
    .await?;
    info!("task {} 已发布，进入自动调度", updated.task_code);
    Ok(ApiResponse::ok(updated))
}

/// `POST /tasks/{id}/unpublish` —— 撤回成草稿：停止自动执行。
///
/// 只拦「执行中 / 已暂停」：那两条说明本体已经受理了这轮任务，先停再撤，
/// 否则运维会以为任务已经停了，实际狗还在跑。已完成 / 异常 / 待执行都可以直接撤回。
pub async fn unpublish_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let task = fetch_task(&state, id).await?;
    super::parks::ensure_owner(&task, &claims)?;
    if !crate::schedule::is_published(&task) {
        return Err(AppError::BadRequest(format!(
            "计划 {} 已经是「草稿」状态",
            task.task_code
        )));
    }
    if matches!(
        task.task_status.as_str(),
        crate::status::RUNNING | crate::status::PAUSED
    ) {
        return Err(AppError::BadRequest(format!(
            "计划 {} 正在执行（{}），请先暂停并取消后再撤回",
            task.task_code, task.task_status
        )));
    }

    sqlx::query("UPDATE tasks SET publish_status = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(crate::schedule::DRAFT)
        .bind(now())
        .bind(id)
        .execute(&state.db)
        .await?;
    record_event(
        &state,
        "draft",
        format!("计划 {} 已撤回为草稿", task.task_code),
        "不再按计划时间自动下发；已经下发的那一次不受影响，需要人跟着收尾".to_string(),
        task.device_id,
        Some(id),
    )
    .await?;
    info!("task {} 已撤回为草稿，停止自动调度", task.task_code);
    Ok(ApiResponse::ok(fetch_task(&state, id).await?))
}

pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<TaskRequest>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let existing = fetch_task(&state, id).await?;
    super::parks::ensure_owner(&existing, &claims)?;
    // 只有尚未下发的任务允许编辑，避免执行中的任务被改到一半
    if existing.task_status != crate::status::PENDING {
        return Err(AppError::BadRequest(format!(
            "任务 {} 当前状态为「{}」，仅「待执行」的任务可以修改",
            existing.task_code, existing.task_status
        )));
    }
    // 状态同样要校验：请求体里来别的字符串会写进库，破坏后续所有终态判断
    let status = payload.task_status.unwrap_or(existing.task_status);
    if !crate::status::is_valid(&status) {
        return Err(AppError::BadRequest(format!("无效的任务状态：{status}")));
    }
    let progress = payload.progress.unwrap_or(existing.progress);
    let device_id = payload.device_id.or(existing.device_id);
    let park_id = payload.park_id.or(existing.park_id);
    let route_id = payload.route_id.unwrap_or(existing.route_id);
    let task_name = if payload.task_name.trim().is_empty() {
        existing.task_name.clone()
    } else {
        payload.task_name.trim().to_string()
    };
    let plan_start = crate::schedule::normalize_clock(payload.plan_start.clone())
        .or(existing.plan_start.clone());
    let plan_end =
        crate::schedule::normalize_clock(payload.plan_end.clone()).or(existing.plan_end.clone());
    let execute_mode = payload
        .execute_mode
        .clone()
        .or(existing.execute_mode.clone());
    // 结束动作：缺省保持原值；显式 null = 改回「跟随路线」
    let end_task = match &payload.end_task {
        Some(value) => normalize_task_end_task(value.clone())?,
        None => existing.end_task.clone(),
    };
    // 仅夜间自动开灯：缺省保持原值（编辑弹窗没传就不动它）
    let light_night_only = payload
        .light_night_only
        .unwrap_or(existing.light_night_only);
    let schedule_days = match payload.schedule_days.as_deref() {
        // 显式传了就以请求为准（空字符串 = 取消周期日 = 每天）
        Some(raw) => crate::schedule::normalize_days(Some(raw)),
        None => existing.schedule_days.clone(),
    };
    crate::schedule::validate_window(
        execute_mode.as_deref(),
        plan_start.as_deref(),
        plan_end.as_deref(),
        schedule_days.as_deref(),
    )?;
    // 发布状态：缺省保持原值（编辑弹窗没传就不动它，避免"改个名字"把已发布改成草稿）
    let publish_status = match payload.publish_status.as_deref().map(str::trim) {
        Some("") => existing.publish_status.clone(),
        Some(raw) => normalize_publish_status(Some(raw.to_string()))?,
        None => existing.publish_status.clone(),
    };
    // 从草稿改成已发布时补上发布时间；已发布的仍保留首次发布时间
    let published_at = if publish_status == crate::schedule::PUBLISHED {
        existing.published_at.clone().or_else(|| Some(now()))
    } else {
        existing.published_at.clone()
    };
    validate_device_park(&state.db, device_id, park_id).await?;
    sqlx::query(
        "UPDATE tasks SET task_name = ?1, task_type = ?2, execute_mode = ?3, end_task = ?4,
            level = ?5, device_id = ?6, park_id = ?7, zone_id = ?8, route_id = ?9,
            plan_start = ?10, plan_end = ?11, schedule_days = ?12, task_status = ?13,
            progress = ?14, dispatch_policy = ?15, description = ?16, updated_at = ?17,
            publish_status = ?19, published_at = ?20, light_night_only = ?21
         WHERE id = ?18",
    )
    .bind(&task_name)
    .bind(payload.task_type.or(existing.task_type))
    .bind(&execute_mode)
    .bind(&end_task)
    .bind(payload.level.or(existing.level))
    .bind(device_id)
    .bind(park_id)
    .bind(payload.zone_id.or(existing.zone_id))
    .bind(route_id)
    .bind(plan_start)
    .bind(plan_end)
    .bind(schedule_days)
    .bind(status)
    .bind(progress)
    .bind(payload.dispatch_policy.or(existing.dispatch_policy))
    .bind(payload.description.or(existing.description))
    .bind(now())
    .bind(id)
    .bind(&publish_status)
    .bind(&published_at)
    .bind(light_night_only)
    .execute(&state.db)
    .await?;
    record_event(
        &state,
        "updated",
        format!("更新任务：{task_name}"),
        format!(
            "计划 {} - {}",
            payload
                .plan_start
                .as_deref()
                .or(existing.plan_start.as_deref())
                .unwrap_or("—"),
            payload
                .plan_end
                .as_deref()
                .or(existing.plan_end.as_deref())
                .unwrap_or("—")
        ),
        device_id,
        Some(id),
    )
    .await?;
    Ok(ApiResponse::ok(fetch_task(&state, id).await?))
}

pub async fn update_task_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<TaskStatusRequest>,
) -> Result<Json<ApiResponse<Task>>, AppError> {
    let existing = fetch_task(&state, id).await?;
    super::parks::ensure_owner(&existing, &claims)?;
    if !crate::status::is_valid(&payload.task_status) {
        return Err(AppError::BadRequest("无效的任务状态".to_string()));
    }

    // 「执行中」= 现在就跑：本体从未受理过这条任务时必须走真正的 task_dispatch。
    // 只改状态 + 发 resume 会让客户端在没有执行上下文时假报「执行中」，任务永久卡住。
    if payload.task_status == crate::status::RUNNING && !resumable(&existing) {
        dispatch_task_or_error(&state, &existing).await?;
        return Ok(ApiResponse::ok(fetch_task(&state, id).await?));
    }

    let progress = payload
        .progress
        .unwrap_or(match payload.task_status.as_str() {
            crate::status::DONE => 100,
            crate::status::PENDING => 0,
            _ => existing.progress,
        });
    sqlx::query("UPDATE tasks SET task_status = ?1, progress = ?2, updated_at = ?3 WHERE id = ?4")
        .bind(&payload.task_status)
        .bind(progress)
        .bind(now())
        .bind(id)
        .execute(&state.db)
        .await?;

    // 暂停 / 恢复 / 取消需要真正通知客户端去驱动本体
    if let Some(action) = control_action(&payload.task_status) {
        push_task_control(&state, &existing, action, payload.reason.as_deref()).await;
    }

    record_event(
        &state,
        "status",
        format!(
            "任务 {} 状态变更为 {}",
            existing.task_code, payload.task_status
        ),
        format!("当前进度 {}%", progress),
        existing.device_id,
        Some(id),
    )
    .await?;
    Ok(ApiResponse::ok(fetch_task(&state, id).await?))
}

/// 任务状态 → 客户端控制指令
fn control_action(task_status: &str) -> Option<&'static str> {
    match task_status {
        "已暂停" => Some("pause"),
        crate::status::RUNNING => Some("resume"),
        "已完成" | "异常" => Some("cancel"),
        _ => None,
    }
}

/// 通过 WebSocket 把控制指令推给设备；失败只记日志，不阻塞状态变更
async fn push_task_control(state: &AppState, task: &Task, action: &str, reason: Option<&str>) {
    let Some(device_pk) = task.device_id else {
        return;
    };
    let Ok(Some(device_code)) =
        sqlx::query_scalar::<_, String>("SELECT device_id FROM devices WHERE id = ?1")
            .bind(device_pk)
            .fetch_optional(&state.db)
            .await
    else {
        return;
    };

    let control = TaskControlPayload {
        task_id: task.id,
        task_code: task.task_code.clone(),
        action: action.to_string(),
        reason: reason.map(str::to_string),
    };
    let sent =
        gateway::send_task_control_to_device(&state.device_connections, &device_code, &control)
            .await;
    if sent {
        info!(
            "task control '{}' pushed to device {} for task {}",
            action, device_code, task.task_code
        );
    } else {
        warn!(
            "device {} offline, task control '{}' for {} not delivered",
            device_code, action, task.task_code
        );
    }
}

/// 任务事件流（进度 / 定位异常 / 指令留痕）。
///
/// 定位守卫拒发任务时，可读原因就落在这里；前端任务详情据此告诉运维下一步做什么。
pub async fn list_task_events(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Vec<crate::models::TaskEvent>>>, AppError> {
    let task = fetch_task(&state, id).await?;
    super::parks::ensure_owner(&task, &claims)?;
    let events = sqlx::query_as::<_, crate::models::TaskEvent>(
        "SELECT e.id, e.event_type, e.title, e.description, d.device_id AS device_code,
            t.task_code, e.created_at
         FROM task_events e
         LEFT JOIN devices d ON d.id = e.device_id
         LEFT JOIN tasks t ON t.id = e.task_id
         WHERE e.task_id = ?1
         ORDER BY e.created_at DESC, e.id DESC LIMIT 20",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(ApiResponse::ok(events))
}

pub async fn delete_task(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let task = fetch_task(&state, id).await?;
    super::parks::ensure_owner(&task, &claims)?;
    sqlx::query("DELETE FROM task_events WHERE task_id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    let result = sqlx::query("DELETE FROM tasks WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

impl super::parks::OwnerCheck for Task {
    fn creator_id(&self) -> Option<i64> {
        self.created_by
    }
}

pub(crate) async fn fetch_task(state: &AppState, id: i64) -> Result<Task, AppError> {
    let sql = format!("{TASK_SELECT} WHERE t.id = ?1");
    sqlx::query_as::<_, crate::models::TaskRow>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .map(Task::from)
        .ok_or_else(|| AppError::NotFound("任务不存在".to_string()))
}

async fn record_event(
    state: &AppState,
    event_type: &str,
    title: String,
    description: String,
    device_id: Option<i64>,
    task_id: Option<i64>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )
    .bind(event_type)
    .bind(title)
    .bind(description)
    .bind(device_id)
    .bind(task_id)
    .bind(now())
    .execute(&state.db)
    .await?;
    Ok(())
}
