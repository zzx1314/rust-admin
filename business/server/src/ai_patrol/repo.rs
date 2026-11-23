//! 数据读取与落库：任务/航点/动作解析、告警与任务事件写入。

// 同一模块拆出来的几块共用一个词汇表（`Window` / `WindowConfig` / `Hit` …），
// 逐个列名只会在每次加类型时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/* ------------------------------- 数据读取 ------------------------------- */

pub(crate) struct TaskRow {
    pub(crate) park_id: Option<i64>,
    pub(crate) device_id: Option<i64>,
    pub(crate) route_id: Option<i64>,
    pub(crate) name: String,
}

pub(crate) struct PointRow {
    pub(crate) name: String,
    pub(crate) dwell: i64,
    pub(crate) actions: Vec<Value>,
}

pub(crate) struct AiAction {
    /// 动作 `param`：场景提示（老数据 / 手填关键词）
    pub(crate) hint: String,
    /// 动作 `threshold`：置信度原文
    pub(crate) threshold: String,
    /// 动作 `scene`：路线编辑器从实时视频模型目录里选的模型包（精确命中，不猜词）
    pub(crate) scene: Option<String>,
    /// 动作 `labels`：勾选的识别类别（Agent 的 label `name`，空 = 该场景全部类别）
    pub(crate) labels: Vec<String>,
    /// 动作 `expectCount`：检查目标数量（数量任务必填）
    pub(crate) expect_count: Option<i64>,
    /// 动作 `checkType`：`exist`（是否存在）/ `count`（数量是否正确）
    pub(crate) check_type: String,
    /// 动作 `alertWhenMissing`：`exist` 任务未检出时也告警（「应该有却没有」）
    pub(crate) alert_when_missing: bool,
    /// 动作 `notifyWhenCountOk`：`count` 任务数量一致时也推企业微信（不建单）
    pub(crate) notify_when_count_ok: bool,
}

impl PointRow {
    pub(crate) fn find_ai_action(&self) -> Option<AiAction> {
        self.actions.iter().find_map(|action| {
            let kind = action.get("type").and_then(Value::as_str).unwrap_or("");
            if !is_ai_action(kind) {
                return None;
            }
            Some(AiAction {
                hint: action
                    .get("param")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                threshold: action
                    .get("threshold")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                expect_count: parse_expect_count(action.get("expectCount")),
                check_type: normalize_check_type(
                    action.get("checkType").and_then(Value::as_str),
                    parse_expect_count(action.get("expectCount")),
                ),
                alert_when_missing: action
                    .get("alertWhenMissing")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                notify_when_count_ok: action
                    .get("notifyWhenCountOk")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                scene: action
                    .get("scene")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                labels: action
                    .get("labels")
                    .and_then(Value::as_array)
                    .map(|labels| {
                        labels
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
    }
}

/// `tasks` 里 AI 窗口需要的那几列（避免 clippy 的 `type_complexity`）
pub(crate) type TaskColumns = (i64, Option<i64>, Option<i64>, Option<i64>, String);

pub(crate) async fn load_task(
    db: &sqlx::SqlitePool,
    task_code: &str,
) -> Result<Option<TaskRow>, sqlx::Error> {
    let row: Option<TaskColumns> = sqlx::query_as(
        "SELECT id, park_id, device_id, route_id, task_name FROM tasks WHERE task_code = ?1",
    )
    .bind(task_code)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(_, park_id, device_id, route_id, name)| TaskRow {
        park_id,
        device_id,
        route_id,
        name,
    }))
}

pub(crate) async fn load_point(
    db: &sqlx::SqlitePool,
    route_id: i64,
    index: i64,
) -> Result<Option<PointRow>, sqlx::Error> {
    let raw: Option<(Option<String>,)> =
        sqlx::query_as("SELECT route_data FROM routes WHERE id = ?1")
            .bind(route_id)
            .fetch_optional(db)
            .await?;
    let Some((Some(text),)) = raw else {
        return Ok(None);
    };
    let Ok(data) = serde_json::from_str::<Value>(&text) else {
        return Ok(None);
    };
    let Some(point) = data
        .get("points")
        .and_then(Value::as_array)
        .and_then(|points| points.get(index as usize))
    else {
        return Ok(None);
    };
    Ok(Some(PointRow {
        name: point
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("航点")
            .to_string(),
        dwell: point.get("dwell").and_then(Value::as_i64).unwrap_or(0),
        actions: point
            .get("actions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    }))
}

/// 返回写入的 `alert_code`，供企业微信推送按码回查（见 `notify::alert_created`）。
///
/// `dedup_key` 是这个航点上的「同一个问题」的结构化标识（见
/// `migrations/024_alert_dedup_key.sql`）：自动工单按它去重，不再拿带置信度/证据路径的
/// 标题比相等，否则每轮巡检都会建一张新单。
pub(crate) async fn insert_alert(
    db: &sqlx::SqlitePool,
    config: &WindowConfig,
    content: &str,
    dedup_key: &str,
) -> Result<String, sqlx::Error> {
    let next_id = crate::ids::next_id(db, "alerts", "alerts").await?;
    let timestamp = now();
    let alert_code = format!("AL-{next_id:03}");
    sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, device_id, park_id, status,
            occurred_at, handled_at, created_at, dedup_key)
         VALUES (?1, '警告', ?2, ?3, ?4, '未处理', ?5, NULL, ?5, ?6)",
    )
    .bind(&alert_code)
    .bind(content)
    .bind(config.device_id)
    .bind(config.park_id)
    .bind(&timestamp)
    .bind(dedup_key)
    .execute(db)
    .await?;
    Ok(alert_code)
}

/// 航点 AI 告警的结构化去重键：`ai:<device>:<taskCode>:<point>:<what>`
///
/// `what` 区分同一航点上的三件事：`<label>` 检出 / `count` 数量不一致 /
/// `missing` 未检出。同一轮与跨轮的同一件事都能靠它命中同一张在办工单。
pub(crate) fn ai_dedup_key(config: &WindowConfig, what: &str) -> String {
    format!(
        "ai:{}:{}:{}:{}",
        config.device_code, config.task_code, config.point_index, what
    )
}

pub(crate) async fn insert_task_event(
    db: &sqlx::SqlitePool,
    config: &WindowConfig,
    at: &str,
    content: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('ai', ?1, ?2, ?3,
                 (SELECT id FROM tasks WHERE task_code = ?4), ?5)",
    )
    .bind(format!("AI 识别 · 航点「{}」", config.point_name))
    .bind(content)
    .bind(config.device_id)
    .bind(&config.task_code)
    .bind(at)
    .execute(db)
    .await?;
    Ok(())
}
