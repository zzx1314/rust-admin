//! 计划时间窗（定时巡检）的判定与到点下发。
//!
//! 解决的问题：`plan_start` / `plan_end` 以前只是两个手填的展示字符串，任何下发路径
//! （建任务即推、设备上线补发、手动开始）都不看它们，于是「定时巡检」实际等于
//! 「建好就立刻跑」，用户配的时间段完全没有意义。本模块把它变成真正的调度依据：
//!
//! - **判定** [`due_state`]：`plan_start - plan_end`（`HH:MM`，允许跨零点）是要执行的
//!   时间段，[`schedule_days`] 是执行日，`last_dispatched_on` 保证同一个窗口只跑一次；
//! - **下发** [`sweep`]：每 [`TICK_SECS`] 秒扫一次「待执行」任务，到点走与手动下发
//!   完全相同的门禁（`dispatch::device_block_reason` + `handlers::tasks::push_task_dispatch`），
//!   设备没就绪就在窗口内继续重试；
//! - **等待中可见**（[`report_waiting`]）：到点但设备没就绪时，在任务事件流里写一条
//!   `schedule_wait`（原因 + 重试次数 + 窗口剩余时间），每 5 分钟刷新一次。以前这里
//!   只有 debug 日志，线上 `RUST_LOG=info` 看不到，控制台表现就是「计划到点了却什么都不
//!   发生」，只能上服务器翻日志才猜得到是设备掉线；
//! - **收尾**：窗口过了还没跑成 → 记事件 + 企微告警，但任务保持「待执行」等下一个
//!   执行日（周期计划不能因为某天设备离线就整条报废）；跑完「已完成」的周期任务
//!   在下一个执行日自动回到「待执行」。
//!
//! 为什么不做 cron 表达式：用户只需要「几点到几点 + 周几」，落库就是两个 `HH:MM`
//! 加一个 `1,2,3` 列表。表达式既多一个能填错的框，也多一套要维护的解析器。
//!
//! 「立即执行 / 临时下发」的任务不参与排期（[`is_scheduled`]），建好就下发，
//! 保持中台一键下发与现场临时任务的原有语义。

use crate::app::AppState;
use crate::db::now as now_stamp;
use crate::error::AppError;
use crate::models::Task;
use chrono::{DateTime, Datelike, Days, Local, NaiveTime, Timelike};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// 扫描周期：30s 足够准（用户填的时间精度是 5 分钟），又不会把库扫热。
pub const TICK_SECS: u64 = 30;

/// 「周期任务」这个执行模式才有执行日 / 时间窗的概念。
const PERIODIC_MODE: &str = "周期任务";

/// 发布状态取值（与路线 `routes.status` 同口径，落库就是这两个中文枚举）。
pub const DRAFT: &str = "草稿";
pub const PUBLISHED: &str = "已发布";

/// 「到点但设备没就绪」事件的刷新节流：同一个窗口内最多 5 分钟改一次描述。
///
/// 扫描是 30s 一轮，不节流的话设备离线一小时就能写出 120 条同样的「等待中」，
/// 把真正要看的事件（导航指令非法、定位失败）全冲掉——客户端 5s 一报的进度事件
/// 当年就是这么把事件流刷爆的。
const WAIT_REFRESH_SECS: u64 = 300;

/// 计次表的保留时长。
///
/// `WAIT_STATES` 以 `task_code` 为键、**只增不删**：建计划 / 删计划 / 改编号都会留下
/// 一条永不过期的记录，进程长期不重启时是一条慢性泄漏。超过保留期（一天没动过）
/// 的计划不必再记——下一个窗口重新计次就行。
const WAIT_STATE_TTL: Duration = Duration::from_hours(24);
/// 计次表超过这个条数才做一次清扫（保持热路径只有一次查表）。
const WAIT_STATE_SWEEP_AT: usize = 64;

/// 每个任务在当前窗口里的重试计数 / 上次落库时间。
///
/// 只放内存：进程重启后最多多写一条「等待中」事件，不影响正确性；反过来落库
/// 就要给 `task_events` 加列，为一条提示性事件改表不值得。key 用 `task_code`
/// 而不是自增 id，跨库/重建的场景也不会串台。
///
/// 挂在 `AppState` 上（见 [`crate::cache::Caches`]），不再是模块级 `static`：
/// 以前集成测试共用一张表，只能靠「每个用例用不同任务号」避免互相影响。
#[derive(Default)]
pub struct WaitStates {
    inner: Mutex<HashMap<String, WaitState>>,
}

impl WaitStates {
    /// 计次 + 节流：`Some(attempts)` 表示本轮要落库，`None` 表示还没到刷新周期。
    fn note(&self, task_code: &str, window_key: &str) -> Option<u32> {
        let now = Instant::now();
        let mut states = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if states.len() > WAIT_STATE_SWEEP_AT {
            states.retain(|_, state| {
                now.saturating_duration_since(state.written_at) < WAIT_STATE_TTL
            });
        }
        let state = states.entry(task_code.to_string()).or_insert(WaitState {
            window: String::new(),
            attempts: 0,
            written_at: now,
        });
        // 首次 / 换窗口（比如第二天同一时段）：重新计次，并且立刻写一条
        if state.window != window_key {
            state.window = window_key.to_string();
            state.attempts = 0;
            state.written_at = now
                .checked_sub(Duration::from_secs(WAIT_REFRESH_SECS))
                .unwrap_or(now);
        }
        state.attempts += 1;
        if now.duration_since(state.written_at).as_secs() < WAIT_REFRESH_SECS {
            return None;
        }
        state.written_at = now;
        Some(state.attempts)
    }
}

struct WaitState {
    /// 当前窗口标识（`2026-11-17 16:00-17:00`）；换窗口就重新计次
    window: String,
    attempts: u32,
    written_at: Instant,
}

/// 一天内的执行窗口
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

impl Window {
    /// `end <= start` 视为跨零点（如 `23:30 - 00:30`）
    pub fn crosses_midnight(&self) -> bool {
        self.end <= self.start
    }

    /// 当前时刻是否落在窗口内
    pub fn contains(&self, time: NaiveTime) -> bool {
        if self.crosses_midnight() {
            time >= self.start || time <= self.end
        } else {
            time >= self.start && time <= self.end
        }
    }

    /// `09:30-10:10`，给事件 / 日志用
    pub fn label(&self) -> String {
        format!("{}-{}", hhmm(self.start), hhmm(self.end))
    }
}

/// 排期状态。`Waiting` / `Missed` 的区别很重要：只有「今天该跑、窗口已经结束」
/// 才告警，否则周末不跑的日常计划会天天报「没执行」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueState {
    /// 现在就在窗口内、今天该跑、且今天还没跑过 → 该下发了
    Due,
    /// 今天已经跑过 / 还没到窗口 / 今天不是执行日 → 什么都不做
    Waiting,
    /// 今天是执行日，窗口已经结束但没跑成 → 告警，等下一个执行日
    Missed,
    /// 计划还是草稿（未发布）→ 不自动执行，等发布后再说
    Draft,
    /// 不按时间窗调度（立即执行 / 临时下发 / 没填时间）
    Immediate,
}

/// 扫描结果，用于日志与测试断言
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepReport {
    pub dispatched: usize,
    pub missed: usize,
    pub rearmed: usize,
}

impl SweepReport {
    pub fn is_empty(&self) -> bool {
        self.dispatched == 0 && self.missed == 0 && self.rearmed == 0
    }
}

/// 解析 `HH:MM`（也容忍 `H:MM`、`HH:MM:SS` 和历史数据里的 `YYYY-MM-DD HH:MM:SS`）。
pub fn parse_hhmm(raw: &str) -> Option<NaiveTime> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    // 历史数据里混着整条时间戳，取空格后的时间部分
    let time_part = match text.rsplit_once(' ') {
        Some((_, tail)) => tail,
        None => text,
    };
    let mut parts = time_part.split(':');
    let hour: u32 = parts.next()?.trim().parse().ok()?;
    let minute: u32 = parts.next()?.trim().parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// 写库前统一成 `HH:MM`。
///
/// 中台的下发对话框用的是 `datetime` 选择器（`YYYY-MM-DD HH:mm:ss`），业务平台这边
/// 的时间窗只认「几点到几点」，所以入库时截成 `HH:MM`——否则列表里会显示一长串
/// 时间戳，而调度器又只读时间部分，两边对不上。解析不了的原样保留（前端会照实显示）。
pub fn normalize_clock(raw: Option<String>) -> Option<String> {
    raw.map(|text| match parse_hhmm(&text) {
        Some(time) => hhmm(time),
        None => text,
    })
}

fn hhmm(time: NaiveTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// 从两个 `HH:MM` 拼出窗口；任一缺失或非法返回 `None`
pub fn window_of_parts(start: Option<&str>, end: Option<&str>) -> Option<Window> {
    Some(Window {
        start: parse_hhmm(start?)?,
        end: parse_hhmm(end?)?,
    })
}

/// 任务配置的执行窗口
pub fn window_of(task: &Task) -> Option<Window> {
    window_of_parts(task.plan_start.as_deref(), task.plan_end.as_deref())
}

/// 发布状态是否合法（`草稿` / `已发布`）
pub fn is_valid_publish_status(raw: &str) -> bool {
    matches!(raw, DRAFT | PUBLISHED)
}

/// 计划是否已发布——**只有已发布的计划才会被自动执行**。
///
/// 自动下发一共有三条路：建计划时顺手下发（`handlers::tasks::create_task_core`）、
/// 设备上线补发（`gateway::dispatch_pending_tasks`）、后台扫描（[`sweep`]）。
/// 三条都用 [`due_state`] 做门禁，所以草稿只要在这里被挡一次就全挡住了；
/// 人工「开始执行 / 重新下发」（`POST /tasks/{id}/dispatch`）不走这里，仍可跑。
pub fn is_published(task: &Task) -> bool {
    task.publish_status == PUBLISHED
}

/// 是否按时间窗调度。
///
/// 只有「周期任务」走排期；`立即执行` / `临时下发` 是现场临时派的活，建好就该发。
/// 中台 `task_dispatch` 不带 `execute_mode`，也不带 `task_type`，因此同样不受排期
/// 影响（保持「一键下发立即执行」的原有语义）。
pub fn is_scheduled(task: &Task) -> bool {
    task.execute_mode.as_deref() == Some(PERIODIC_MODE)
}

/// 判断一份请求参数（还没落库）是不是要按时间窗调度
pub fn is_scheduled_request(execute_mode: Option<&str>) -> bool {
    execute_mode == Some(PERIODIC_MODE)
}

/// 解析周期日 `1,2,3,4,5`（1=周一 … 7=周日）；空 / `None` 表示每天。
///
/// 非法值直接忽略：宁可少一天，也不要因为一个手写的 `星期八` 让整条计划不跑。
pub fn parse_days(raw: Option<&str>) -> Vec<u32> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut days: Vec<u32> = raw
        .split(',')
        .filter_map(|part| part.trim().parse::<u32>().ok())
        .filter(|day| (1..=7).contains(day))
        .collect();
    days.sort_unstable();
    days.dedup();
    days
}

/// 规范化写库：`["1","2"]` → `"1,2"`；全空 → `None`（= 每天）
pub fn normalize_days(raw: Option<&str>) -> Option<String> {
    let days = parse_days(raw);
    if days.is_empty() {
        return None;
    }
    Some(
        days.iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// 周期日是否包含某一天（`weekday` 用 1=周一 … 7=周日）
pub fn runs_on(schedule_days: Option<&str>, weekday: u32) -> bool {
    let days = parse_days(schedule_days);
    days.is_empty() || days.contains(&weekday)
}

/// 周期日的中文说明：`每天` / `工作日` / `周一、周三`
pub fn days_label(schedule_days: Option<&str>) -> String {
    let days = parse_days(schedule_days);
    match days.as_slice() {
        [] => "每天".to_string(),
        [1, 2, 3, 4, 5] => "工作日".to_string(),
        [1, 2, 3, 4, 5, 6, 7] => "每天".to_string(),
        other => other
            .iter()
            .map(|day| match day {
                1 => "周一",
                2 => "周二",
                3 => "周三",
                4 => "周四",
                5 => "周五",
                6 => "周六",
                _ => "周日",
            })
            .collect::<Vec<_>>()
            .join("、"),
    }
}

/// 一次窗口实例的 `(开始, 结束)`
type Span = (DateTime<Local>, DateTime<Local>);

/// `now` 所处的窗口实例 + 今天这个实例的开始时刻（还没开始时用它判断「还没到」）。
///
/// 返回 `(当前正在里面的实例, 今天的实例开始)`：跨零点窗口在凌晨会命中「昨晚那个」
/// 实例，而「今天的实例开始」仍在今天晚上。
fn window_instances(window: &Window, now: DateTime<Local>) -> (Option<Span>, DateTime<Local>) {
    let today = now.date_naive();
    let at = |date: chrono::NaiveDate, time: NaiveTime| {
        date.and_time(time).and_local_timezone(Local).earliest()
    };
    let start_today = at(today, window.start);
    let end_today = if window.crosses_midnight() {
        today
            .checked_add_days(Days::new(1))
            .and_then(|next| at(next, window.end))
    } else {
        at(today, window.end)
    };

    let mut current = None;
    if let (Some(start), Some(end)) = (start_today, end_today)
        && now >= start
        && now <= end
    {
        current = Some((start, end));
    }
    // 跨零点时，凌晨那一段属于「昨晚开始」的实例
    if current.is_none()
        && window.crosses_midnight()
        && let Some(prev) = today.checked_sub_days(Days::new(1))
        && let (Some(start), Some(end)) = (at(prev, window.start), at(today, window.end))
        && now >= start
        && now <= end
    {
        current = Some((start, end));
    }

    (current, start_today.unwrap_or(now))
}

/// 判定一条任务现在该不该下发 / 是不是错过了窗口。
///
/// 「今天」对跨零点窗口取**窗口起始日**：`23:30-00:30` 在凌晨 00:10 仍算前一晚那次。
pub fn due_state(task: &Task, now: DateTime<Local>) -> DueState {
    // 草稿先挡：没发布的计划不参与任何自动下发（执行模式是周期还是立即都一样），
    // 否则「存一份备稿」就等于「立刻让它跑起来」。
    if !is_published(task) {
        return DueState::Draft;
    }
    if !is_scheduled(task) {
        return DueState::Immediate;
    }
    // 周期任务没填时间：前端有选择器、后端建任务时也会挡；真漏了就当「不排期」，
    // 免得一条错数据把扫描器永远卡在 Waiting 里。
    let Some(window) = window_of(task) else {
        return DueState::Immediate;
    };
    let (current, start_today) = window_instances(&window, now);

    if let Some((start, _end)) = current {
        let run_day = start.date_naive();
        let run_day_text = run_day.format("%Y-%m-%d").to_string();
        if !runs_on(
            task.schedule_days.as_deref(),
            run_day.weekday().number_from_monday(),
        ) {
            return DueState::Waiting;
        }
        if task.last_dispatched_on.as_deref() == Some(run_day_text.as_str()) {
            return DueState::Waiting;
        }
        return DueState::Due;
    }

    // 不在窗口内：今天的窗口还没到就是 Waiting；已经过了且今天该跑就是 Missed
    if now < start_today {
        return DueState::Waiting;
    }
    let today = now.date_naive();
    if !runs_on(
        task.schedule_days.as_deref(),
        today.weekday().number_from_monday(),
    ) {
        return DueState::Waiting;
    }
    let today_text = today.format("%Y-%m-%d").to_string();
    if task.last_dispatched_on.as_deref() == Some(today_text.as_str()) {
        return DueState::Waiting;
    }
    DueState::Missed
}

/// 排期的人话说明，写进任务事件流，运维不用猜「为什么这条没动」。
pub fn schedule_hint(task: &Task, now: DateTime<Local>) -> String {
    let window = window_of(task);
    let range = window.map_or_else(|| "—".to_string(), |item| item.label());
    let days = days_label(task.schedule_days.as_deref());
    match due_state(task, now) {
        DueState::Due => format!("计划窗口 {range}（{days}）已到点，正在下发"),
        DueState::Missed => {
            format!("计划窗口 {range}（{days}）今天的窗口已过，将在下一个执行日的同一时段自动执行")
        }
        DueState::Waiting => format!("计划窗口 {range}（{days}），到点自动下发"),
        DueState::Draft => {
            format!(
                "计划还是草稿（窗口 {range} · {days}）：发布之后才会按时段自动执行，现在只保存不下发"
            )
        }
        DueState::Immediate => "立即执行：不受计划时间窗限制，创建后直接下发".to_string(),
    }
}

/// 建任务时的窗口校验：周期任务必须有合法的起止时间。
///
/// 前端已经用时间选择器做了限制，这里再挡一次——手填/中台接口调用都还进得来。
pub fn validate_window(
    execute_mode: Option<&str>,
    plan_start: Option<&str>,
    plan_end: Option<&str>,
    schedule_days: Option<&str>,
) -> Result<(), AppError> {
    if let Some(raw) = schedule_days
        && !raw.trim().is_empty()
        && parse_days(Some(raw)).is_empty()
    {
        return Err(AppError::BadRequest(
            "执行日格式不合法：应为 1-7（1=周一 … 7=周日）的逗号分隔列表".to_string(),
        ));
    }
    if !is_scheduled_request(execute_mode) {
        return Ok(());
    }
    let window = window_of_parts(plan_start, plan_end).ok_or_else(|| {
        AppError::BadRequest("周期任务必须选择计划时间段（HH:MM - HH:MM）".to_string())
    })?;
    if window.start == window.end {
        return Err(AppError::BadRequest(
            "计划时间段的开始与结束时间不能相同".to_string(),
        ));
    }
    Ok(())
}

/// 启动后台扫描
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(TICK_SECS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // 启动那一轮任务扫描不跑（见下行），但地图状态可以立刻对齐一次：
        // 中台发布后的状态同步纯展示用，跑早了不会把任务误标成「窗口未执行」。
        if let Err(error) = crate::handlers::maps::sync_map_statuses(&state).await {
            warn!("地图状态同步失败: {error}");
        }
        // 启动那一下不扫：进程刚起来时设备还没注册，白白把任务标成「窗口未执行」
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match sweep(&state).await {
                Ok(report) if report.is_empty() => {}
                Ok(report) => info!(
                    dispatched = report.dispatched,
                    missed = report.missed,
                    rearmed = report.rearmed,
                    "计划调度：到点下发 / 窗口未执行 / 周期回位"
                ),
                Err(error) => warn!("计划调度扫描失败: {error}"),
            }
        }
    });
}

/// 扫一轮：周期任务回位 → 到点的下发 → 错过窗口的告警。
pub async fn sweep(state: &AppState) -> Result<SweepReport, AppError> {
    let now = Local::now();
    let mut report = SweepReport::default();
    rearm_finished(state, now, &mut report).await?;
    // 录像留存：超期的行 + 文件一起收（`recording_keep_days = 0` 时是空操作）。
    // 放在这里而不是新开一个循环：清理失败绝不能影响到点下发。
    if let Err(error) = crate::recordings::sweep_expired(state).await {
        warn!("巡检录像超期清理失败: {error}");
    }
    // 航点照片留存：同一处顺手收（`photo_keep_days = 0` 时是空操作）。
    // 照片是巡检报告的证据，默认留得比录像久（90 天 vs 30 天）。
    if let Err(error) = crate::photos::sweep_expired(state).await {
        warn!("航点照片超期清理失败: {error}");
    }
    // 地图状态：中台才是主库，发布 / 归档后得把 `map_versions.status` 拉回来，
    // 否则业务平台一直显示入库时的「部分标定」。同步失败不能让计划调度停摆。
    if let Err(error) = crate::handlers::maps::sync_map_statuses(state).await {
        warn!("地图状态同步失败: {error}");
    }

    let pending: Vec<Task> = sqlx::query_as::<_, crate::models::TaskRow>(&format!(
        "{} WHERE t.task_status = '待执行' AND t.execute_mode = ?1 AND t.publish_status = ?2 \
         ORDER BY t.id ASC",
        crate::handlers::tasks::TASK_SELECT
    ))
    .bind(PERIODIC_MODE)
    .bind(PUBLISHED)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(Task::from)
    .collect();

    for task in pending {
        match due_state(&task, now) {
            DueState::Due => match dispatch_due(state, &task, now).await {
                Ok(true) => report.dispatched += 1,
                Ok(false) => {}
                // **一条任务下发失败不能拖垮整轮扫描**：以前这里把错误 `?` 出去，
                // 排在它后面的计划全部停摆（示意坐标系地图的演示任务在 09:20 窗口内、
                // 或者某条路线被改坏，就会把整轮扫描打断，且只在日志里报一行
                // `计划调度扫描失败`）。现在当成「这条暂时发不出去」记录下来继续扫。
                Err(error) => {
                    warn!(task = %task.task_code, "计划到点下发失败：{error}");
                    report_waiting(state, &task, now, &format!("下发失败：{error}")).await?;
                }
            },
            DueState::Missed => {
                if notice_missed(state, &task, now).await? {
                    report.missed += 1;
                }
            }
            DueState::Waiting | DueState::Immediate | DueState::Draft => {}
        }
    }
    Ok(report)
}

/// 到点下发。返回是否真的推出去了；设备没就绪时返回 `false`，下一轮继续试。
async fn dispatch_due(
    state: &AppState,
    task: &Task,
    now: DateTime<Local>,
) -> Result<bool, AppError> {
    let Some(device_pk) = task.device_id else {
        report_waiting(state, task, now, "任务还没有分配执行设备").await?;
        return Ok(false);
    };
    if let Some(reason) = crate::dispatch::device_block_reason(&state.db, device_pk).await? {
        report_waiting(state, task, now, &reason).await?;
        return Ok(false);
    }
    match crate::handlers::tasks::push_task_dispatch(state, task).await? {
        Some(device_code) => {
            record_event(
                state,
                task,
                format!("任务 {} 到点自动下发", task.task_code),
                format!("{}，已下发到设备 {device_code}", schedule_hint(task, now)),
            )
            .await?;
            info!(
                task = %task.task_code,
                device = %device_code,
                "计划任务到点下发成功"
            );
            Ok(true)
        }
        None => {
            report_waiting(
                state,
                task,
                now,
                "设备未与业务平台建立 WebSocket（客户端离线），任务推不出去",
            )
            .await?;
            Ok(false)
        }
    }
}

/// 到点了但没发出去：在任务事件流里留一条「到点未下发」，写清原因和进度。
///
/// 同一窗口内每 [`WAIT_REFRESH_SECS`] 秒刷新一次描述（首次立即写），标题固定为
/// `任务 X 到点未下发（2026-11-17 16:00-17:00）`：同一条事件被反复 UPDATE，
/// 运维在控制台看到的是一行「已重试 N 次 · 窗口剩余 M 分钟 · 原因 …」，
/// 而不是一屏同样的记录。
async fn report_waiting(
    state: &AppState,
    task: &Task,
    now: DateTime<Local>,
    reason: &str,
) -> Result<(), AppError> {
    let Some(window) = window_of(task) else {
        return Ok(());
    };
    let key = window_key(&window, now);
    let Some(attempts) = state.caches.wait_states.note(&task.task_code, &key) else {
        return Ok(());
    };
    let remaining = window_instances(&window, now)
        .0
        .map(|(_, end)| (end - now).num_minutes().max(0));
    let description = format!(
        "计划窗口 {label}（{days}） · 到点后已重试 {attempts} 次{remaining} · 原因：{reason} · \
         任务保持待执行，窗口内每 {TICK_SECS}s 自动重试，窗口结束仍未下发会单独告警",
        label = window.label(),
        days = days_label(task.schedule_days.as_deref()),
        remaining = remaining
            .map(|minutes| format!("（窗口剩余 {minutes} 分钟）"))
            .unwrap_or_default(),
    );
    let title = format!("任务 {} 到点未下发（{key}）", task.task_code);

    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM task_events
          WHERE task_id = ?1 AND event_type = 'schedule_wait' AND title = ?2 LIMIT 1",
    )
    .bind(task.id)
    .bind(&title)
    .fetch_optional(&state.db)
    .await?;
    match existing {
        Some(id) => {
            sqlx::query("UPDATE task_events SET description = ?1 WHERE id = ?2")
                .bind(&description)
                .bind(id)
                .execute(&state.db)
                .await?;
        }
        None => {
            sqlx::query(
                "INSERT INTO task_events
                    (event_type, title, description, device_id, task_id, created_at)
                 VALUES ('schedule_wait', ?1, ?2, ?3, ?4, ?5)",
            )
            .bind(&title)
            .bind(&description)
            .bind(task.device_id)
            .bind(task.id)
            .bind(now_stamp())
            .execute(&state.db)
            .await?;
        }
    }
    info!(
        task = %task.task_code,
        attempts,
        "计划任务到点未下发，等待设备就绪：{reason}"
    );
    Ok(())
}

/// 窗口标识：`2026-11-17 16:00-17:00`（跨零点窗口按起始日算）
fn window_key(window: &Window, now: DateTime<Local>) -> String {
    let (current, start_today) = window_instances(window, now);
    let start = current.map_or(start_today, |(start, _)| start);
    format!(
        "{} {}",
        start.date_naive().format("%Y-%m-%d"),
        window.label()
    )
}

/// 窗口过了还没跑成：记事件 + 告警（同一天只报一次）。
///
/// 任务保持「待执行」：周期计划不该因为某天设备离线就整条报废，下一个执行日照跑。
async fn notice_missed(
    state: &AppState,
    task: &Task,
    now: DateTime<Local>,
) -> Result<bool, AppError> {
    // 建任务时就错过了当天窗口（窗口在创建之前就结束了）不算「漏执行」：
    // 那种情况建任务时已经提示过「将在下一个执行日执行」。
    let Some(window) = window_of(task) else {
        return Ok(false);
    };
    let Some(created) = parse_local(task.created_at.as_str()) else {
        return Ok(false);
    };
    let window_end_today = today_window_end(&window, now);
    if window_end_today.is_some_and(|end| created >= end) {
        return Ok(false);
    }

    let stamp = now.format("%Y-%m-%d").to_string();
    let title = format!(
        "任务 {} 计划窗口未执行（{stamp} {}）",
        task.task_code,
        window.label()
    );
    // 同一天只报一次：扫描每 30s 一轮，不去重会把事件流刷爆
    let exists: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM task_events
          WHERE task_id = ?1 AND event_type = 'schedule_miss' AND title = ?2 LIMIT 1",
    )
    .bind(task.id)
    .bind(&title)
    .fetch_optional(&state.db)
    .await?;
    if exists.is_some() {
        return Ok(false);
    }

    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('schedule_miss', ?1, ?2, ?3, ?4, ?5)",
    )
    .bind(&title)
    .bind(format!(
        "窗口 {} 内未能下发（设备离线 / 本体未就绪 / 未分配设备），任务保持待执行，\
         下一个执行日同一时段自动重试",
        window.label()
    ))
    .bind(task.device_id)
    .bind(task.id)
    .bind(now_stamp())
    .execute(&state.db)
    .await?;

    crate::notify::task_abnormal(
        &state.notify(),
        &task.task_code,
        crate::notify::Level::Warning,
        &format!("计划窗口 {} 内未能执行", window.label()),
    )
    .await;
    warn!(
        task = %task.task_code,
        "计划窗口内未能执行，已记事件并推送告警"
    );
    Ok(true)
}

/// 周期任务跑完（已完成）→ 下一天回到「待执行」，等下一个窗口。
///
/// 只回位「已完成」：「异常」要人看一眼（本体故障、路线被拒等），自动重跑只会
/// 把同一个故障刷成每天一条；运维确认后把状态改回待执行即可继续排期。
async fn rearm_finished(
    state: &AppState,
    now: DateTime<Local>,
    report: &mut SweepReport,
) -> Result<(), AppError> {
    let today = now.format("%Y-%m-%d").to_string();
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, task_code FROM tasks
          WHERE task_status = '已完成'
            AND execute_mode = ?1
            AND last_dispatched_on IS NOT NULL
            AND last_dispatched_on < ?2",
    )
    .bind(PERIODIC_MODE)
    .bind(&today)
    .fetch_all(&state.db)
    .await?;

    for (id, task_code) in rows {
        let result = sqlx::query(
            "UPDATE tasks SET task_status = '待执行', progress = 0, progress_updated_at = NULL,
                robot_task_id = NULL, robot_stage = NULL, robot_updated_at = NULL, updated_at = ?1
             WHERE id = ?2 AND task_status = '已完成'",
        )
        .bind(now_stamp())
        .bind(id)
        .execute(&state.db)
        .await?;
        if result.rows_affected() == 0 {
            continue;
        }
        sqlx::query(
            "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
             VALUES ('schedule', ?1, ?2,
                     (SELECT device_id FROM tasks WHERE id = ?3), ?3, ?4)",
        )
        .bind(format!("任务 {task_code} 进入下一次执行周期"))
        .bind("周期任务已回到待执行，将在下一个执行日的计划时段自动下发")
        .bind(id)
        .bind(now_stamp())
        .execute(&state.db)
        .await
        .ok();
        report.rearmed += 1;
    }
    Ok(())
}

async fn record_event(
    state: &AppState,
    task: &Task,
    title: String,
    description: String,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('schedule', ?1, ?2, ?3, ?4, ?5)",
    )
    .bind(title)
    .bind(description)
    .bind(task.device_id)
    .bind(task.id)
    .bind(now_stamp())
    .execute(&state.db)
    .await?;
    Ok(())
}

/// 今天的窗口结束时刻（跨零点时是明天）
fn today_window_end(window: &Window, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let date = now.date_naive();
    let date = if window.crosses_midnight() {
        date.checked_add_days(Days::new(1))?
    } else {
        date
    };
    date.and_time(window.end)
        .and_local_timezone(Local)
        .earliest()
}

fn parse_local(raw: &str) -> Option<DateTime<Local>> {
    let naive = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S").ok()?;
    naive.and_local_timezone(Local).earliest()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(execute_mode: Option<&str>, start: Option<&str>, end: Option<&str>) -> Task {
        Task {
            id: 1,
            task_code: "TK-1".to_string(),
            task_name: "测试".to_string(),
            task_type: Some("定时巡检".to_string()),
            execute_mode: execute_mode.map(str::to_string),
            end_task: None,
            light_night_only: false,
            light_fill: false,
            light_led: false,
            light_night_start: None,
            light_night_end: None,
            level: Some("常规".to_string()),
            device_id: Some(1),
            device_code: Some("RD-001".to_string()),
            park_id: Some(1),
            park_name: None,
            zone_id: None,
            zone_name: None,
            route_id: None,
            route_code: None,
            route_name: None,
            plan_start: start.map(str::to_string),
            plan_end: end.map(str::to_string),
            task_status: "待执行".to_string(),
            progress: 0,
            dispatch_policy: None,
            description: None,
            robot_task_id: None,
            robot_stage: None,
            robot_updated_at: None,
            schedule_days: None,
            last_dispatched_on: None,
            publish_status: PUBLISHED.to_string(),
            published_at: None,
            created_at: "2026-11-14 08:00:00".to_string(),
            updated_at: "2026-11-14 08:00:00".to_string(),
            created_by: None,
        }
    }

    fn at(text: &str) -> DateTime<Local> {
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
            .unwrap()
            .and_local_timezone(Local)
            .earliest()
            .unwrap()
    }

    #[test]
    fn parses_clock_formats() {
        assert_eq!(parse_hhmm("09:30"), NaiveTime::from_hms_opt(9, 30, 0));
        assert_eq!(parse_hhmm("9:05"), NaiveTime::from_hms_opt(9, 5, 0));
        assert_eq!(
            parse_hhmm("2026-11-14 18:20:00"),
            NaiveTime::from_hms_opt(18, 20, 0)
        );
        assert_eq!(parse_hhmm(""), None);
        assert_eq!(parse_hhmm("9点"), None);
        assert_eq!(parse_hhmm("25:00"), None);
    }

    #[test]
    fn clock_values_are_normalized_before_storing() {
        assert_eq!(
            normalize_clock(Some("2026-11-14 08:05:00".to_string())),
            Some("08:05".to_string())
        );
        assert_eq!(
            normalize_clock(Some("9:5".to_string())),
            Some("09:05".to_string())
        );
        // 解析不了的原样留着，前端照实显示
        assert_eq!(
            normalize_clock(Some("9点".to_string())),
            Some("9点".to_string())
        );
        assert_eq!(normalize_clock(None), None);
    }

    #[test]
    fn immediate_modes_ignore_window() {
        let now = at("2026-11-14 03:00:00");
        for mode in ["立即执行", "临时下发"] {
            assert_eq!(
                due_state(&task(Some(mode), Some("09:30"), Some("10:10")), now),
                DueState::Immediate
            );
        }
        // 周期任务没填时间也不排期（建任务时已被挡）
        assert_eq!(
            due_state(&task(Some("周期任务"), None, None), now),
            DueState::Immediate
        );
    }

    #[test]
    fn due_only_inside_window() {
        let t = task(Some("周期任务"), Some("09:30"), Some("10:10"));
        assert_eq!(due_state(&t, at("2026-11-14 09:00:00")), DueState::Waiting);
        assert_eq!(due_state(&t, at("2026-11-14 09:30:00")), DueState::Due);
        assert_eq!(due_state(&t, at("2026-11-14 10:10:00")), DueState::Due);
        assert_eq!(due_state(&t, at("2026-11-14 10:10:30")), DueState::Missed);
        assert_eq!(due_state(&t, at("2026-11-14 10:11:00")), DueState::Missed);
    }

    #[test]
    fn already_ran_today_is_waiting() {
        let mut t = task(Some("周期任务"), Some("09:30"), Some("10:10"));
        t.last_dispatched_on = Some("2026-11-14".to_string());
        assert_eq!(due_state(&t, at("2026-11-14 09:40:00")), DueState::Waiting);
        // 错过判定同样不重复
        assert_eq!(due_state(&t, at("2026-11-14 11:00:00")), DueState::Waiting);
    }

    #[test]
    fn draft_plans_are_never_auto_dispatched() {
        // 草稿：窗口内、今天该跑、设备就绪也不下发，且不能报「漏执行」
        let mut t = task(Some("周期任务"), Some("09:30"), Some("10:10"));
        t.publish_status = DRAFT.to_string();
        assert_eq!(due_state(&t, at("2026-11-14 09:40:00")), DueState::Draft);
        assert_eq!(due_state(&t, at("2026-11-14 11:00:00")), DueState::Draft);
        // 「立即执行 / 临时下发」的草稿同样不自动跑：存一份备稿不等于立刻开跑
        let mut immediate = task(Some("立即执行"), None, None);
        immediate.publish_status = DRAFT.to_string();
        assert_eq!(
            due_state(&immediate, at("2026-11-14 09:40:00")),
            DueState::Draft
        );
        // 发布之后回到正常判定
        t.publish_status = PUBLISHED.to_string();
        assert_eq!(due_state(&t, at("2026-11-14 09:40:00")), DueState::Due);
    }

    #[test]
    fn only_known_publish_status_values_pass_validation() {
        assert!(is_valid_publish_status(PUBLISHED));
        assert!(is_valid_publish_status(DRAFT));
        assert!(!is_valid_publish_status("发布"));
        assert!(!is_valid_publish_status(""));
    }

    #[test]
    fn schedule_days_gate_the_run() {
        let mut t = task(Some("周期任务"), Some("09:30"), Some("10:10"));
        t.schedule_days = Some("1,2,3,4,5".to_string()); // 工作日
        // 2026-11-14 是周六
        assert_eq!(due_state(&t, at("2026-11-14 09:40:00")), DueState::Waiting);
        assert_eq!(due_state(&t, at("2026-11-14 11:00:00")), DueState::Waiting);
        // 周五正常
        assert_eq!(due_state(&t, at("2026-11-13 09:40:00")), DueState::Due);
    }

    #[test]
    fn overnight_window_belongs_to_previous_day() {
        let mut t = task(Some("周期任务"), Some("23:30"), Some("00:30"));
        t.schedule_days = Some("5".to_string()); // 只周五
        // 周五 23:45 → 窗口内
        assert_eq!(due_state(&t, at("2026-11-13 23:45:00")), DueState::Due);
        // 周六 00:10 → 仍是「周五那一次」
        assert_eq!(due_state(&t, at("2026-11-14 00:10:00")), DueState::Due);
        // 周五当天跑过之后，凌晨仍是同一次
        t.last_dispatched_on = Some("2026-11-13".to_string());
        assert_eq!(due_state(&t, at("2026-11-14 00:10:00")), DueState::Waiting);
    }

    #[test]
    fn days_normalization_and_label() {
        assert_eq!(normalize_days(Some("5,1,1,9")), Some("1,5".to_string()));
        assert_eq!(normalize_days(Some("")), None);
        assert_eq!(normalize_days(None), None);
        assert_eq!(days_label(None), "每天");
        assert_eq!(days_label(Some("1,2,3,4,5")), "工作日");
        assert_eq!(days_label(Some("1,3")), "周一、周三");
    }

    #[test]
    fn wait_state_throttles_but_counts_every_attempt() {
        // 每个用例自己一份（挂在 `AppState` 上以后就是这样），不必再挑任务号避串台
        let states = WaitStates::default();
        let code = "TK-UNIT-WAIT";
        let window = "2026-11-17 16:00-17:00";
        // 首次一定落库，并记 1 次
        assert_eq!(states.note(code, window), Some(1));
        // 30s 一轮的后续尝试只攒着，不刷事件
        for _ in 0..5 {
            assert_eq!(states.note(code, window), None);
        }
        // 换窗口（第二天同一时段）→ 立刻落库并重新计次
        assert_eq!(states.note(code, "2026-11-18 16:00-17:00"), Some(1));
    }

    #[test]
    fn window_key_pins_the_start_day() {
        let overnight = Window {
            start: parse_hhmm("23:30").unwrap(),
            end: parse_hhmm("00:30").unwrap(),
        };
        // 凌晨 00:10 属于「昨天那次」窗口
        assert_eq!(
            window_key(&overnight, at("2026-11-14 00:10:00")),
            "2026-11-13 23:30-00:30"
        );
        assert_eq!(
            window_key(&overnight, at("2026-11-14 23:45:00")),
            "2026-11-14 23:30-00:30"
        );
    }

    #[test]
    fn validate_rejects_bad_periodic_window() {
        assert!(validate_window(Some("周期任务"), Some("09:30"), None, None).is_err());
        assert!(validate_window(Some("周期任务"), Some("09:30"), Some("09:30"), None).is_err());
        assert!(validate_window(Some("周期任务"), Some("09:30"), Some("10:10"), Some("1")).is_ok());
        // 非周期模式不强制时间
        assert!(validate_window(Some("立即执行"), None, None, None).is_ok());
        assert!(
            validate_window(Some("周期任务"), Some("09:30"), Some("10:10"), Some("8")).is_err()
        );
    }
}
