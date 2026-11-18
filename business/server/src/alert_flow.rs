//! 告警 → 责任人 → 企业微信 + 工单：执行计划时发现告警的闭环。
//!
//! # 为什么是「排班」而不是「计划」
//!
//! 计划（`tasks`）只描述「哪个设备在什么时候跑哪条路线」，**没有**「谁负责」的字段；
//! 责任人只存在于计划层的排班里：
//!
//! ```text
//! tasks（计划） ──┐
//! parks / zones ──┴─→ shifts（某天某班次谁负责哪个园区/区域/计划）─→ staff（责任人库）
//! ```
//!
//! 所以「通过计划找到关联责任人」= 拿告警的园区 / 区域 / 正在执行的任务，
//! 去**当天**的排班里找最匹配的那一条。匹配是打分制（见 [`candidate_score`]）：
//! 同一任务 > 同一区域 > 同一园区 > 已发布 > 在岗，取最高分那一条。
//!
//! # 兜底顺序
//!
//! 1. 当天当班排班（最准）
//! 2. 园区负责人（`parks.owner_name`，能对上责任人库就带上 id/电话）
//! 3. `未指派` —— 工单仍然建，让值班长在看板上改派，而不是把告警闷掉
//!
//! # 与推送的关系
//!
//! 派单 + 建工单**不依赖**企业微信配置（现场可能根本没配 webhook），
//! 所以它跑在 [`crate::notify::alert_created`] 的最前面、开关判断之前；
//! 推送才是旁路。
//!
//! 但 **哪些告警值得派单**是另一回事：[`should_dispatch`] 是两者的共同前置门——
//! 只有「执行计划期间」（告警设备当时有 `执行中` 的任务）的告警才建单 + 推群，
//! 掉线 / 控制权抢占 / 手动新建只落库。判据见 [`in_task_execution`]，
//! 开关是 `settings.alert_only_in_task`（缺行视为开启，关掉回到老口径）。
//!
//! 幂等：告警行的 `work_order_id` 非空就不再建单（重复调用 / 重试安全）。

use crate::app::AppState;
use crate::error::AppError;
use crate::models::WorkOrderRequest;

/// 一次派单的结果（既是工单入参，也是推送给群里的那几行字）。
#[derive(Debug, Clone, Default)]
pub struct Assignment {
    pub staff_id: Option<i64>,
    pub name: String,
    pub team: Option<String>,
    pub phone: Option<String>,
    pub shift_code: Option<String>,
    pub slot: Option<String>,
    pub time_range: Option<String>,
    /// 排班关联的巡检计划名（给消息里的「关联计划」用）
    pub task_name: Option<String>,
    /// 派单依据，落到 `alerts.assignee_note`
    pub note: String,
    pub work_order_id: Option<i64>,
    pub work_order_code: Option<String>,
    /// 同一设备的同一故障已经有在办工单：本条告警只是在「跟单」（挂到老单，不新建）。
    ///
    /// **不再等于「不推群」**：跟单消息默认照样推（`wecom_alert_repeat_push`），
    /// 一次任务执行就是一次告警；只有 5 分钟窗口内的连刷会被合并。
    pub repeated: bool,
    /// 跟单但**数值变了**（如数量核对 1 个 → 2 个）：仍然要推一条短消息，
    /// 否则现场只看到第一次的数，后面的变化全被去重吞掉（2026-11-17 反馈）。
    pub follow_up: Option<CountChange>,
}

/// 同一个「数量核对」问题两次告警之间，实际检出数的变化
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountChange {
    pub previous: i64,
    pub current: i64,
}

impl Assignment {
    fn unassigned(note: impl Into<String>) -> Self {
        Self {
            staff_id: None,
            name: "未指派".to_string(),
            note: note.into(),
            ..Default::default()
        }
    }

    /// `陈志远（运维一组）· 早班 08:00 - 16:00（SH-001）` 这种一句话。
    pub fn summary(&self) -> String {
        let mut out = self.name.clone();
        if let Some(team) = self.team.as_deref().filter(|team| !team.trim().is_empty()) {
            out.push_str(&format!("（{team}）"));
        }
        if let Some(slot) = self.slot.as_deref() {
            out.push_str(&format!(" · {slot}"));
            if let Some(range) = self.time_range.as_deref() {
                out.push(' ');
                out.push_str(range);
            }
            if let Some(code) = self.shift_code.as_deref() {
                out.push_str(&format!("（{code}）"));
            }
        } else if !self.note.is_empty() {
            out.push_str(&format!(" · {}", self.note));
        }
        out
    }

    /// 任务异常推送用的一句：只说「这班谁在管」，不涉及建单（任务异常不建单）。
    pub fn duty_sentence(&self) -> String {
        match self.staff_id {
            Some(_) => format!(
                "当班责任人是 {}（{}），请同步跟进",
                self.summary(),
                self.note
            ),
            None => format!(
                "未找到当班责任人（{}），请值班长先补排班或直接派单",
                self.note
            ),
        }
    }

    /// 消息里那句「处置建议」：把派单依据、工单、下一步说成一段话。
    ///
    /// `slot` 有值 = 命中了当班排班；没有 = 走的园区负责人兜底，
    /// 这时候不能说「已按当班排班」，否则群里会以为今天有人值班。
    pub fn dispatch_sentence(&self) -> String {
        let lead = if self.slot.is_some() {
            format!("已按当班排班派给 {}（{}）", self.summary(), self.note)
        } else if self.staff_id.is_some() {
            format!("当天没有当班排班，已派给园区负责人 {}", self.name)
        } else {
            "当天没有排班也没有园区负责人".to_string()
        };
        match (&self.work_order_code, self.staff_id) {
            (Some(order), _) => {
                format!("{lead}，工单 {order} 已自动生成，请责任人到场确认并把处置结果回填到工单")
            }
            (None, Some(_)) => format!("{lead}，自动建单已关闭，请人工确认后在工单看板派单"),
            (None, None) => {
                format!("{lead}，工单已按「未指派」创建，请值班长在看板上改派后再通知现场")
            }
        }
    }
}

/// 告警 → 园区 / 区域 / 设备 / 正在执行的任务。
#[derive(Debug, sqlx::FromRow)]
struct AlertContext {
    id: i64,
    alert_code: String,
    level: String,
    content: String,
    effective_park_id: Option<i64>,
    device_pk: Option<i64>,
    device_code: Option<String>,
    zone_id: Option<i64>,
    zone_name: Option<String>,
    /// 非空表示已经派过单（幂等依据）
    work_order_id: Option<i64>,
    /// 告警来源给的结构化去重键（`ai:…` / `fault:…` / `hb:…` / `ctrl:…`）
    ///
    /// 有它就按「同一设备 + 同一键」找在办工单；为空才退回「标题相等」
    /// （历史行为）。见 `migrations/024_alert_dedup_key.sql`。
    dedup_key: Option<String>,
}

/// 排班候选（当天所有非草案排班）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct ShiftCandidate {
    pub shift_code: String,
    pub slot: String,
    pub time_range: String,
    pub status: String,
    pub staff_id: Option<i64>,
    pub owner_name: String,
    pub park_id: Option<i64>,
    pub zone_id: Option<i64>,
    pub task_id: Option<i64>,
    pub task_name: Option<String>,
    pub team: Option<String>,
    pub phone: Option<String>,
    /// 责任人库里的在岗状态（排班没关联责任人库时为空）
    pub staff_status: Option<String>,
}

/// 匹配打分。**纯函数**，单测直接喂数据（`tests` 里那几条）。
///
/// 权重刻意拉开量级：同一任务(40) > 同一区域(20) > 同一园区(10)，
/// 后面三个是「同等匹配下更该派给他」的加分项。
pub(crate) fn candidate_score(
    candidate: &ShiftCandidate,
    park_id: Option<i64>,
    zone_id: Option<i64>,
    task_id: Option<i64>,
) -> i64 {
    let mut score = 0;
    if let (Some(left), Some(right)) = (candidate.task_id, task_id)
        && left == right
    {
        score += 40;
    }
    if let (Some(left), Some(right)) = (candidate.zone_id, zone_id)
        && left == right
    {
        score += 20;
    }
    if let (Some(left), Some(right)) = (candidate.park_id, park_id)
        && left == right
    {
        score += 10;
    }
    if candidate.status == "已发布" {
        score += 4;
    }
    if candidate.staff_id.is_some() {
        score += 2;
    }
    if candidate.staff_status.as_deref() == Some("在岗") {
        score += 1;
    }
    score
}

/// 告警落库后调用：找责任人 → 自动建工单 → 回写告警行。
///
/// 返回的 [`Assignment`] 供 [`crate::notify::alert_created`] 渲染群消息。
/// 全程只碰本地 SQLite（校验上限都在毫秒级），不 await 外网。
pub async fn on_alert(state: &AppState, alert_code: &str) -> Option<Assignment> {
    let context: AlertContext = sqlx::query_as(
        "SELECT a.id, a.alert_code, a.level, a.content, a.dedup_key,
                COALESCE(a.park_id, d.park_id) AS effective_park_id,
                d.id AS device_pk, d.device_id AS device_code,
                d.zone_id, z.zone_name, a.work_order_id
           FROM alerts a
           LEFT JOIN devices d ON d.id = a.device_id
           LEFT JOIN zones z ON z.id = d.zone_id
          WHERE a.alert_code = ?1",
    )
    .bind(alert_code)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None)?;

    // 设备上正在执行的计划（执行中优先于已暂停），用它把告警对到具体的排班条目
    let running_task: Option<(i64, String)> = match context.device_pk {
        Some(device_pk) => sqlx::query_as(
            "SELECT id, task_name FROM tasks
              WHERE device_id = ?1 AND task_status IN ('执行中', '已暂停')
              ORDER BY (task_status = '执行中') DESC, updated_at DESC
              LIMIT 1",
        )
        .bind(device_pk)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None),
        None => None,
    };
    let running_task_id = running_task.as_ref().map(|(id, _)| *id);
    let running_task_name = running_task.as_ref().map(|(_, name)| name.clone());

    let mut assignment = resolve(
        state,
        context.effective_park_id,
        context.zone_id,
        running_task_id,
    )
    .await;
    if assignment.task_name.is_none() {
        assignment.task_name = running_task_name;
    }

    // 同一设备的**同一件事**还在办：直接挂到那张单上。
    //
    // 现场踩过两次：
    // 1. 机器人每 ~90s 报一次 `12608 RS 与 lidar 时间不同步`，一条告警一张单，
    //    5 分钟就堆出 2 张工单（群消息那边由推送层的 5 分钟窗口挡）；
    // 2. AI 告警内容带置信度 / 证据图路径，按标题去重等于没去重，每轮巡检一张新单。
    // 所以优先用告警自带的 `dedup_key`（结构化），只有它为空时才退回标题相等。
    let title = work_order_title(context.device_code.as_deref(), &context.content);
    let existing = find_open_order(
        state,
        context.device_pk,
        context.dedup_key.as_deref(),
        &title,
    )
    .await;

    // 自动建单：已经派过就跳过（幂等），开关关掉也跳过（但推送照样带责任人）
    if let Some((id, code)) = &existing {
        assignment.work_order_id = Some(*id);
        assignment.work_order_code = Some(code.clone());
        assignment.repeated = true;
        // 同一件事的第二次告警照样推（跟单），但「数量核对」的实际检出数变了
        // 要单独标出来（1 个 → 2 个是不同的事实，消息里要写清）。
        let previous: Option<String> = sqlx::query_scalar(
            "SELECT content FROM alerts
              WHERE work_order_id = ?1 AND id <> ?2
              ORDER BY id DESC LIMIT 1",
        )
        .bind(id)
        .bind(context.id)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
        assignment.follow_up = count_change(previous.as_deref(), &context.content);
    } else if context.work_order_id.is_none() && auto_workorder_enabled(state).await {
        match create_work_order(state, &context, &assignment).await {
            Ok((id, code)) => {
                assignment.work_order_id = Some(id);
                assignment.work_order_code = Some(code);
            }
            Err(error) => {
                tracing::warn!(
                    alert = %context.alert_code,
                    "告警自动派单失败（不影响告警本身）: {error}"
                );
            }
        }
    } else if let Some(order_id) = context.work_order_id {
        // 之前已经派过：把工单号补齐，消息里仍要显示「已派单」
        assignment.work_order_id = Some(order_id);
        assignment.work_order_code =
            sqlx::query_scalar::<_, String>("SELECT order_code FROM work_orders WHERE id = ?1")
                .bind(order_id)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);
    }

    if let Err(error) = sqlx::query(
        "UPDATE alerts SET staff_id = ?1, assignee = ?2, work_order_id = ?3,
            assignee_note = ?4 WHERE id = ?5",
    )
    .bind(assignment.staff_id)
    .bind(&assignment.name)
    .bind(assignment.work_order_id)
    .bind(&assignment.note)
    .bind(context.id)
    .execute(&state.db)
    .await
    {
        tracing::warn!(alert = %context.alert_code, "告警派单回写失败: {error}");
    }

    Some(assignment)
}

/// 只找责任人、不建工单：任务异常推送（`notify::task_abnormal`）复用同一套匹配。
pub async fn responsible_for_task(
    state: &AppState,
    park_id: Option<i64>,
    task_id: Option<i64>,
) -> Option<Assignment> {
    let assignment = resolve(state, park_id, None, task_id).await;
    // 「未指派」等于没找到人，别把空派单塞进消息里
    (assignment.name != "未指派").then_some(assignment)
}

/// 责任人解析：当天排班 → 园区负责人 → 未指派。
async fn resolve(
    state: &AppState,
    park_id: Option<i64>,
    zone_id: Option<i64>,
    task_id: Option<i64>,
) -> Assignment {
    if let Some(candidate) = best_shift(state, park_id, zone_id, task_id).await {
        return Assignment {
            staff_id: candidate.staff_id,
            name: candidate.owner_name.clone(),
            team: candidate.team,
            phone: candidate.phone,
            note: format!(
                "当班排班 {}（{} {}）",
                candidate.shift_code, candidate.slot, candidate.time_range
            ),
            shift_code: Some(candidate.shift_code),
            slot: Some(candidate.slot),
            time_range: Some(candidate.time_range),
            task_name: candidate.task_name,
            ..Default::default()
        };
    }

    // 兜底一：园区负责人。排班还没排到人时，园区负责人是最合理的默认派单人
    if let Some(park_id) = park_id
        && let Ok(Some(owner)) =
            sqlx::query_scalar::<_, Option<String>>("SELECT owner_name FROM parks WHERE id = ?1")
                .bind(park_id)
                .fetch_optional(&state.db)
                .await
    {
        let owner = owner.unwrap_or_default().trim().to_string();
        if !owner.is_empty() {
            let staff: Option<(i64, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT id, team, phone FROM staff WHERE name = ?1 ORDER BY id ASC LIMIT 1",
            )
            .bind(&owner)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None);
            let (staff_id, team, phone) = match staff {
                Some((id, team, phone)) => (Some(id), team, phone),
                None => (None, None, None),
            };
            return Assignment {
                staff_id,
                name: owner,
                team,
                phone,
                note: "当天没有当班排班，派给园区负责人".to_string(),
                ..Default::default()
            };
        }
    }

    Assignment::unassigned("当天没有当班排班，园区也没登记负责人")
}

/// 当天排班里分数最高的一条（分数为 0 视为没匹配上）。
///
/// 只看「今天」+ 非草案：草案是还没发布的想法，不该收到告警推送。
async fn best_shift(
    state: &AppState,
    park_id: Option<i64>,
    zone_id: Option<i64>,
    task_id: Option<i64>,
) -> Option<ShiftCandidate> {
    let candidates: Vec<ShiftCandidate> = sqlx::query_as(
        "SELECT s.shift_code, s.slot, s.time_range, s.status, s.staff_id, s.owner_name,
                s.park_id, s.zone_id, s.task_id, s.task_name,
                st.team, st.phone, st.status AS staff_status
           FROM shifts s
           LEFT JOIN staff st ON st.id = s.staff_id
          WHERE s.shift_date = ?1 AND s.status <> '草案'
          ORDER BY s.id ASC",
    )
    .bind(crate::db::today())
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    candidates
        .into_iter()
        .map(|candidate| {
            let score = candidate_score(&candidate, park_id, zone_id, task_id);
            (score, candidate)
        })
        // 同分时 `max_by_key` 取最后一个，所以先按 id 升序排 → 稳定地取最晚建的那条
        .filter(|(score, _)| *score > 0)
        .max_by_key(|(score, _)| *score)
        .map(|(_, candidate)| candidate)
}

/// 自动建单开关（`settings.alert_auto_workorder`）。
///
/// 缺行视为**开启**：老库升级上来还没播种时，不应该把「告警自动派单」静默关掉。
async fn auto_workorder_enabled(state: &AppState) -> bool {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT setting_value FROM settings WHERE setting_key = 'alert_auto_workorder'",
    )
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    match row {
        Some((Some(value),)) => value.trim() != "false",
        _ => true,
    }
}

/// 「只在执行计划时建单 + 推群」开关（`settings.alert_only_in_task`）。
///
/// 缺行视为**开启**（现行口径）：只有「执行计划期间」产生的告警才自动建工单、推企业微信。
/// 掉线（`hb:`）/ 控制权抢占（`ctrl:`）/ 手动新建 这类一律**只落库**——告警中心照常
/// 能看到，但不再派生工单、也不再往群里刷。要恢复「所有告警都建单推群」的老口径，
/// 在设置页把它关掉即可。
pub async fn only_in_task_enabled(state: &AppState) -> bool {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT setting_value FROM settings WHERE setting_key = 'alert_only_in_task'",
    )
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    match row {
        Some((Some(value),)) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "off"
        ),
        _ => true,
    }
}

/// 这条告警是否应该派生「自动建工单 + 企业微信推送」。
///
/// [`crate::notify::alert_created`] 的第一道门：只负责判「现在该不该动作」，
/// 具体派给谁、建不建单还是后面的 [`on_alert`]。
pub async fn should_dispatch(state: &AppState, alert_code: &str) -> bool {
    if !only_in_task_enabled(state).await {
        return true;
    }
    in_task_execution(state, alert_code).await
}

/// 告警落库那一刻，它绑定的设备上是否有 `执行中` 的任务。
///
/// 这就是「执行计划期间」的判据：巡检/任务在跑时本体报障碍、故障、AI 检出才算得上是
/// 执行任务的告警；没跑任务的时候报的（掉线、遥控器抢占、页面手动新建）不属于这一类。
///
/// 未绑定设备的告警一律为 `false`（连不上任何计划）。
pub async fn in_task_execution(state: &AppState, alert_code: &str) -> bool {
    let hit: Option<i64> = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
               FROM alerts a
               JOIN tasks t ON t.device_id = a.device_id
              WHERE a.alert_code = ?1 AND t.task_status = '执行中'
         )",
    )
    .bind(alert_code)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    hit.unwrap_or(0) != 0
}

/// 告警自动建单的标题（设备 + 内容前 60 字）。
///
/// 同一设备反复上报同一句故障时标题完全相同 —— 这就是「是不是同一件事」的判据。
fn work_order_title(device_code: Option<&str>, content: &str) -> String {
    let device = device_code.unwrap_or("未绑定设备");
    short(&format!("{device} {content}"), 60)
}

/// 同一设备上因为**同一件事**而建、且还没关闭的工单。
///
/// 两条判据：
/// 1. 告警带了 `dedup_key`（结构化：设备 + 任务 + 航点 + 类别 / 故障码）→ 按
///    `work_orders.dedup_key` 比相等，内容里的置信度、证据图路径不会把工单拆成两张；
/// 2. 没带键（人工告警 / 老数据）→ 退回「标题完全相等」的老行为。
///
/// 窗口 24 小时：留着的老单说明问题还没解决，继续挂它；超过一天还没关的单
/// 不再吞新告警，避免一个僵尸单永久吸走所有同类告警。
async fn find_open_order(
    state: &AppState,
    device_pk: Option<i64>,
    dedup_key: Option<&str>,
    title: &str,
) -> Option<(i64, String)> {
    let cutoff = (chrono::Local::now() - chrono::Duration::hours(24))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let key = dedup_key.map(str::trim).filter(|key| !key.is_empty());
    if let Some(key) = key {
        return sqlx::query_as::<_, (i64, String)>(
            "SELECT id, order_code FROM work_orders
              WHERE source = '告警自动派单' AND status <> '已完成'
                AND device_id IS ?1 AND dedup_key = ?2 AND created_at >= ?3
              ORDER BY id DESC LIMIT 1",
        )
        .bind(device_pk)
        .bind(key)
        .bind(cutoff)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    }
    sqlx::query_as::<_, (i64, String)>(
        "SELECT id, order_code FROM work_orders
          WHERE source = '告警自动派单' AND status <> '已完成'
            AND device_id IS ?1 AND dedup_key IS NULL AND title = ?2 AND created_at >= ?3
          ORDER BY id DESC LIMIT 1",
    )
    .bind(device_pk)
    .bind(title)
    .bind(cutoff)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None)
}

/// 建工单：来源写「告警自动派单」，`sourceRef` / `alert_id` 保留原始告警编号，
/// 工单详情里能反查回告警。
async fn create_work_order(
    state: &AppState,
    context: &AlertContext,
    assignment: &Assignment,
) -> Result<(i64, String), AppError> {
    let title = work_order_title(context.device_code.as_deref(), &context.content);
    let mut description = format!(
        "由告警 {} 自动派单。\n派单依据：{}。\n告警内容：{}",
        context.alert_code, assignment.note, context.content
    );
    if let Some(task_name) = assignment.task_name.as_deref() {
        description.push_str(&format!("\n关联计划：{task_name}"));
    }
    if let Some(zone) = context.zone_name.as_deref() {
        description.push_str(&format!("\n区域：{zone}"));
    }
    if let Some(phone) = assignment
        .phone
        .as_deref()
        .filter(|phone| !phone.is_empty())
    {
        description.push_str(&format!("\n责任人电话：{phone}"));
    }

    let payload = WorkOrderRequest {
        title,
        order_type: Some("告警处置".to_string()),
        source: Some("告警自动派单".to_string()),
        source_ref: Some(context.alert_code.clone()),
        priority: Some(priority_of(&context.level).to_string()),
        staff_id: assignment.staff_id,
        assignee: Some(assignment.name.clone()),
        park_id: context.effective_park_id,
        device_id: context.device_pk,
        deadline: None,
        description: Some(description),
        alert_id: Some(context.id),
    };
    let order =
        crate::handlers::workorders::create_order_core(state, payload, None, "系统", false).await?;
    // 把结构化去重键落到工单上：下一轮同样的问题（同一设备 + 同一键）会挂到这张单
    if let Some(key) = context.dedup_key.as_deref()
        && !key.trim().is_empty()
        && let Err(error) = sqlx::query("UPDATE work_orders SET dedup_key = ?1 WHERE id = ?2")
            .bind(key.trim())
            .bind(order.id)
            .execute(&state.db)
            .await
    {
        tracing::warn!(order = %order.order_code, "工单写入去重键失败: {error}");
    }
    Ok((order.id, order.order_code))
}

/// 从数量核对告警的内容里取「实际检出 N 个」。
///
/// 内容形如：
/// `AI 识别数量核对不一致：任务「…」航点 0「…」检查 3 个，实际检出 2 个（证据：…）`
fn found_count_of(content: &str) -> Option<i64> {
    let rest = content.split("实际检出 ").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// 「同类告警继续挂着在办工单」时要不要补推一条群消息。
///
/// 只对**数量核对**这类带数值的结论补推：上次检出 1 个、这次 2 个是两个不同的事实；
/// 同一数值（每次巡检都会再报一遍）继续静默，避免刷屏。
/// 普通检出（`…检出 extinguisher（置信度 0.89）`）不补推——置信度抖动没有信息量。
pub fn count_change(previous: Option<&str>, current: &str) -> Option<CountChange> {
    let current = found_count_of(current)?;
    let previous = found_count_of(previous?)?;
    (previous != current).then_some(CountChange { previous, current })
}

/// 告警级别 → 工单优先级。
fn priority_of(level: &str) -> &'static str {
    match level.trim() {
        "严重" => "紧急",
        "警告" => "高",
        _ => "中",
    }
}

/// 按字符（不是字节）截断，保留可读前缀。
fn short(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        shift_code: &str,
        staff_id: Option<i64>,
        park_id: Option<i64>,
        zone_id: Option<i64>,
        task_id: Option<i64>,
        status: &str,
        staff_status: Option<&str>,
    ) -> ShiftCandidate {
        ShiftCandidate {
            shift_code: shift_code.to_string(),
            slot: "早班".to_string(),
            time_range: "08:00 - 16:00".to_string(),
            status: status.to_string(),
            staff_id,
            owner_name: "陈志远".to_string(),
            park_id,
            zone_id,
            task_id,
            task_name: Some("装卸平台早班巡查".to_string()),
            team: Some("运维一组".to_string()),
            phone: Some("13800001203".to_string()),
            staff_status: staff_status.map(str::to_string),
        }
    }

    #[test]
    fn same_task_beats_same_park() {
        let by_task = candidate(
            "SH-001",
            Some(1),
            Some(1),
            Some(2),
            Some(9),
            "已发布",
            Some("在岗"),
        );
        let by_park = candidate(
            "SH-002",
            Some(2),
            Some(1),
            Some(3),
            Some(8),
            "已发布",
            Some("在岗"),
        );
        assert!(
            candidate_score(&by_task, Some(1), Some(2), Some(9))
                > candidate_score(&by_park, Some(1), Some(2), Some(9))
        );
    }

    #[test]
    fn zone_beats_park_and_published_breaks_tie() {
        let by_zone = candidate("SH-003", Some(3), Some(1), Some(2), None, "待确认", None);
        let by_park = candidate(
            "SH-004",
            Some(4),
            Some(1),
            Some(5),
            None,
            "已发布",
            Some("在岗"),
        );
        assert!(
            candidate_score(&by_zone, Some(1), Some(2), None)
                > candidate_score(&by_park, Some(1), Some(2), None)
        );
        let drafted_park = candidate("SH-005", Some(5), Some(1), None, None, "待确认", None);
        assert!(
            candidate_score(&by_park, Some(1), None, None)
                > candidate_score(&drafted_park, Some(1), None, None)
        );
    }

    #[test]
    fn unrelated_shift_scores_zero() {
        let other = candidate(
            "SH-006",
            Some(6),
            Some(2),
            Some(7),
            Some(9),
            "已发布",
            Some("在岗"),
        );
        assert_eq!(
            candidate_score(&other, Some(1), Some(2), Some(8)),
            4 + 2 + 1
        );
        // 只有「已发布/在岗」这类通用加分，没有任何业务匹配时应当判为"没匹配上"
        let zero = candidate("SH-007", None, Some(2), Some(7), Some(9), "待确认", None);
        assert_eq!(candidate_score(&zero, Some(1), Some(2), Some(8)), 0);
    }

    #[test]
    fn summary_renders_shift_and_note() {
        let assignment = Assignment {
            staff_id: Some(1),
            name: "陈志远".to_string(),
            team: Some("运维一组".to_string()),
            phone: Some("13800001203".to_string()),
            shift_code: Some("SH-001".to_string()),
            slot: Some("早班".to_string()),
            time_range: Some("08:00 - 16:00".to_string()),
            note: "当班排班 SH-001（早班 08:00 - 16:00）".to_string(),
            work_order_code: Some("WO-1115-01".to_string()),
            ..Default::default()
        };
        assert_eq!(
            assignment.summary(),
            "陈志远（运维一组） · 早班 08:00 - 16:00（SH-001）"
        );
        assert!(assignment.dispatch_sentence().contains("WO-1115-01"));

        // 兜底派单不能说「已按当班排班」，否则群里会以为今天有人值班
        let park_owner = Assignment {
            staff_id: Some(8),
            name: "张泽新".to_string(),
            team: Some("巡检一班".to_string()),
            note: "当天没有当班排班，派给园区负责人".to_string(),
            work_order_code: Some("WO-1117-09".to_string()),
            ..Default::default()
        };
        let sentence = park_owner.dispatch_sentence();
        assert!(!sentence.contains("按当班排班"), "{sentence}");
        assert!(sentence.contains("园区负责人 张泽新"), "{sentence}");

        let fallback = Assignment::unassigned("当天没有当班排班，园区也没登记负责人");
        assert_eq!(
            fallback.summary(),
            "未指派 · 当天没有当班排班，园区也没登记负责人"
        );
        assert!(fallback.dispatch_sentence().contains("值班长"));
    }

    #[test]
    fn priority_and_title_helpers() {
        assert_eq!(priority_of("严重"), "紧急");
        assert_eq!(priority_of("警告"), "高");
        assert_eq!(priority_of("提示"), "中");
        assert_eq!(priority_of("未知"), "中");
        let long = "告".repeat(80);
        let clipped = short(&long, 60);
        assert_eq!(clipped.chars().count(), 61);
        assert!(clipped.ends_with('…'));
    }

    /// 数量核对告警的「跟单补推」判定：只有检出数变了才说话。
    ///
    /// 现场（2026-11-17 20:15）：同一航点先报 1 个（推过群），下一轮报 2 个，
    /// 全被「同类在办工单」吞掉，现场以为「配了检查 3 个却没告警」。
    #[test]
    fn count_change_only_when_number_moves() {
        let first = "AI 识别数量核对不一致：任务「巡检」航点 0「A」检查 3 个，实际检出 1 个";
        let second = "AI 识别数量核对不一致：任务「巡检」航点 0「A」检查 3 个，实际检出 2 个";

        assert_eq!(
            count_change(Some(first), second),
            Some(CountChange {
                previous: 1,
                current: 2
            })
        );
        // 同一个数重复报（每轮巡检都会）→ 不补推
        assert_eq!(count_change(Some(second), second), None);
        // 上次不是数量告警 / 缺失 → 没有可比对象，不补推
        assert_eq!(count_change(None, second), None);
        assert_eq!(
            count_change(
                Some("AI 识别：任务「巡检」航点 0「A」检出 extinguisher（置信度 0.89）"),
                second
            ),
            None
        );
        // 普通检出（置信度抖动）不补推
        assert_eq!(
            count_change(
                Some("AI 识别：任务「巡检」航点 0「A」检出 extinguisher（置信度 0.41）"),
                "AI 识别：任务「巡检」航点 0「A」检出 extinguisher（置信度 0.77）"
            ),
            None
        );
        // 「一个都没找到」也算数值（0 个）
        let zero = "AI 识别数量核对不一致：任务「巡检」航点 0「A」检查 3 个，实际检出 0 个";
        assert_eq!(
            count_change(Some(zero), second),
            Some(CountChange {
                previous: 0,
                current: 2
            })
        );
    }
}
