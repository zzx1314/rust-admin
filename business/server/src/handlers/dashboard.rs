//! 总览驾驶舱。
//!
//! 驾驶舱上的每一项数据都必须能回答「这是谁的」：
//!
//! - 设备：业务平台不存归属关系，按中台设备分配关系（`allowed_device_codes`）过滤；
//! - 园区 / 任务 / 路线：按 `created_by` 过滤；
//! - 园区实时态势：只放当前人可见设备所在的园区，点位来自设备真实上报坐标；
//! - 告警：既按可见园区、也按可见设备收敛，避免非管理员看到别人园区的告警。
//!
//! 管理员 / 本地服务账号（没有中台归属）保持全局视角。

use crate::app::AppState;
use crate::auth::{self, Claims};
use crate::error::{ApiResponse, AppError};
use crate::models::{Alert, Device, Park, TaskEvent};
use axum::Extension;
use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Sqlite};
use std::collections::{HashMap, HashSet};

/// 告警动态行：(id, level, content, device_code, park_name, status, occurred_at)
type AlertActivityRow = (
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
);

/// 任务计划日期。
///
/// 历史任务的 `plan_start` 只存 `HH:MM`，不能直接 `date()`：SQLite 会把
/// `'09:30'` 解析成 `2000-01-01`。只有 `YYYY-MM-DD` 开头时才用 `plan_start`，
/// 否则退回 `created_at`。
const TASK_PLAN_DAY: &str = "CASE WHEN t.plan_start GLOB \
    '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]*' \
    THEN date(t.plan_start) ELSE date(t.created_at) END";

/// 把可见设备编号拼进 SQL（`allowed` 为 `None` 表示不限制）。
///
/// 调用方需要保证 `column` 前已有 `WHERE` / `AND` 条件，函数自带 `AND`。
fn push_device_codes<'q>(
    builder: &mut QueryBuilder<'q, Sqlite>,
    column: &str,
    allowed: Option<&HashSet<String>>,
) {
    let Some(codes) = allowed else {
        return;
    };
    if codes.is_empty() {
        // 没有任何分配设备：直接让条件不成立，不要退化成「不过滤」
        builder.push(" AND 1 = 0");
        return;
    }
    builder.push(format!(" AND {column} IN ("));
    for (index, code) in codes.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push_bind(code.clone());
    }
    builder.push(")");
}

/// 拼一段 `IN (...)` 列表（已带前导 `AND`）。
fn push_id_list<'q>(builder: &mut QueryBuilder<'q, Sqlite>, table: &str, ids: &[i64]) {
    if ids.is_empty() {
        builder.push(" AND 1 = 0");
        return;
    }
    builder.push(format!(" AND {table} IN ("));
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            builder.push(", ");
        }
        builder.push_bind(*id);
    }
    builder.push(")");
}

/// 近 7 日任务趋势：直接按任务表聚合，不再读取预置的 `daily_stats`。
///
/// 计划日期优先取 `plan_start`（能解析出真实日期时），历史数据里只存
/// `HH:MM` 的记录退回 `created_at`；完成日期用 `updated_at` 近似（任务表没有
/// 单独的完成时间列）。非管理员只统计自己创建的任务。
async fn task_trend(
    state: &AppState,
    claims: &Claims,
) -> Result<(Vec<String>, Vec<i64>, Vec<i64>), AppError> {
    let today = chrono::Local::now().date_naive();
    let days: Vec<String> = (0..7)
        .map(|offset| {
            (today - chrono::Duration::days(6 - offset))
                .format("%Y-%m-%d")
                .to_string()
        })
        .collect();
    let start = days.first().cloned().unwrap_or_default();

    let mut plan_builder = QueryBuilder::<Sqlite>::new(format!(
        "SELECT {TASK_PLAN_DAY} AS day, COUNT(*) FROM tasks t WHERE 1 = 1"
    ));
    if !claims.is_admin {
        plan_builder
            .push(" AND t.created_by = ")
            .push_bind(claims.sub);
    }
    plan_builder
        .push(format!(" AND {TASK_PLAN_DAY} >= "))
        .push_bind(start.clone())
        .push(" GROUP BY day");
    let plan_rows: Vec<(String, i64)> = plan_builder
        .build_query_as::<(String, i64)>()
        .fetch_all(&state.db)
        .await?;

    let mut done_builder = QueryBuilder::<Sqlite>::new(
        "SELECT date(t.updated_at) AS day, COUNT(*) FROM tasks t
         WHERE t.task_status = '已完成'",
    );
    if !claims.is_admin {
        done_builder
            .push(" AND t.created_by = ")
            .push_bind(claims.sub);
    }
    done_builder
        .push(" AND date(t.updated_at) >= ")
        .push_bind(start)
        .push(" GROUP BY day");
    let done_rows: Vec<(String, i64)> = done_builder
        .build_query_as::<(String, i64)>()
        .fetch_all(&state.db)
        .await?;

    let plan_map: HashMap<String, i64> = plan_rows.into_iter().collect();
    let done_map: HashMap<String, i64> = done_rows.into_iter().collect();
    let plan = days
        .iter()
        .map(|day| plan_map.get(day).copied().unwrap_or(0))
        .collect();
    let done = days
        .iter()
        .map(|day| done_map.get(day).copied().unwrap_or(0))
        .collect();
    Ok((days, plan, done))
}

pub async fn summary(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    // 当前登录人的设备可见范围：None = 全部（管理员 / 本地账号）
    let allowed = auth::allowed_device_codes(&state, &claims).await?;

    // ── 设备统计（按中台分配关系过滤） ────────────────────────
    let mut device_builder = QueryBuilder::<Sqlite>::new(super::devices::DEVICE_SELECT);
    device_builder.push(" WHERE 1 = 1");
    push_device_codes(&mut device_builder, "d.device_id", allowed.as_ref());
    device_builder.push(" ORDER BY d.id ASC");
    let devices = device_builder
        .build_query_as::<Device>()
        .fetch_all(&state.db)
        .await?;

    let device_total = devices.len() as i64;
    let mut executing = 0_i64;
    let mut standby = 0_i64;
    let mut charging = 0_i64;
    let mut offline = 0_i64;
    let mut fault = 0_i64;
    let mut unbound = 0_i64;
    for device in &devices {
        match device.status.as_str() {
            "执行任务" => executing += 1,
            "在线待命" => standby += 1,
            "充电中" => charging += 1,
            "故障" => fault += 1,
            "待绑定" => unbound += 1,
            _ => offline += 1,
        }
    }
    let online = executing + standby;

    // ── 任务统计（只统计当前人创建的任务） ──────────────────────
    let mut task_builder =
        QueryBuilder::<Sqlite>::new("SELECT task_status, COUNT(*) FROM tasks t WHERE 1 = 1");
    if !claims.is_admin {
        task_builder
            .push(" AND t.created_by = ")
            .push_bind(claims.sub);
    }
    task_builder.push(" GROUP BY task_status");
    let task_rows: Vec<(String, i64)> = task_builder
        .build_query_as::<(String, i64)>()
        .fetch_all(&state.db)
        .await?;
    let task_total: i64 = task_rows.iter().map(|(_, count)| count).sum();

    // 「今日巡检任务」：按计划日期（无日期时按创建日期）落在今天的任务统计，
    // 不再拿任务总数冒充今日任务。
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut today_builder = QueryBuilder::<Sqlite>::new(format!(
        "SELECT task_status, COUNT(*) FROM tasks t WHERE {TASK_PLAN_DAY} = "
    ));
    today_builder.push_bind(today);
    if !claims.is_admin {
        today_builder
            .push(" AND t.created_by = ")
            .push_bind(claims.sub);
    }
    today_builder.push(" GROUP BY task_status");
    let today_rows: Vec<(String, i64)> = today_builder
        .build_query_as::<(String, i64)>()
        .fetch_all(&state.db)
        .await?;
    let today_total: i64 = today_rows.iter().map(|(_, count)| count).sum();
    let today_done = today_rows
        .iter()
        .find(|(status, _)| status == "已完成")
        .map(|(_, count)| *count)
        .unwrap_or(0);
    let today_rate = if today_total == 0 {
        0
    } else {
        today_done * 100 / today_total
    };

    // ── 园区（自己创建的 + 自己管辖设备所在的） ────────────────
    let mut parks_builder = QueryBuilder::<Sqlite>::new(
        "SELECT p.id, p.park_code, p.park_name, p.park_description, p.park_address,
            p.area, p.owner_name, p.park_type, p.status, p.color, p.created_at, p.updated_at,
            p.map_version_id, p.map_status, p.created_by,
            (SELECT COUNT(*) FROM zones z WHERE z.park_id = p.id) AS zone_count,
            (SELECT COUNT(*) FROM devices d2 WHERE d2.park_id = p.id",
    );
    // 设备数量同样只统计当前人管辖的设备
    push_device_codes(&mut parks_builder, "d2.device_id", allowed.as_ref());
    parks_builder.push(") AS device_count, (SELECT COUNT(*) FROM tasks t2 WHERE t2.park_id = p.id");
    if !claims.is_admin {
        parks_builder
            .push(" AND t2.created_by = ")
            .push_bind(claims.sub);
    }
    parks_builder.push(
        ") AS task_count, (SELECT COUNT(*) FROM tasks t3 WHERE t3.park_id = p.id AND t3.task_status = '已完成'",
    );
    if !claims.is_admin {
        parks_builder
            .push(" AND t3.created_by = ")
            .push_bind(claims.sub);
    }
    parks_builder.push(") AS done_count FROM parks p WHERE 1 = 1");
    if !claims.is_admin {
        parks_builder
            .push(" AND (p.created_by = ")
            .push_bind(claims.sub);
        if let Some(codes) = allowed.as_ref()
            && !codes.is_empty()
        {
            parks_builder.push(
                " OR p.id IN (SELECT d0.park_id FROM devices d0
                        WHERE d0.park_id IS NOT NULL AND d0.device_id IN (",
            );
            for (index, code) in codes.iter().enumerate() {
                if index > 0 {
                    parks_builder.push(", ");
                }
                parks_builder.push_bind(code.clone());
            }
            parks_builder.push("))");
        }
        parks_builder.push(")");
    }
    parks_builder.push(" ORDER BY p.id ASC");
    let parks = parks_builder
        .build_query_as::<Park>()
        .fetch_all(&state.db)
        .await?;

    let park_ids: Vec<i64> = parks.iter().map(|park| park.id).collect();
    let area: f64 = parks.iter().map(|park| park.area).sum();
    let park_count = parks.len() as i64;

    // ── 区域数：只统计可见园区下的区域 ────────────────────────
    let mut zone_builder = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM zones z WHERE 1 = 1");
    push_id_list(&mut zone_builder, "z.park_id", &park_ids);
    let zone_count: i64 = zone_builder
        .build_query_scalar()
        .fetch_one(&state.db)
        .await?;

    // ── 告警：可见园区 or 可见设备 ────────────────────────────
    let alert_scope = |builder: &mut QueryBuilder<'_, Sqlite>| {
        if claims.is_admin {
            return;
        }
        builder.push(" AND (");
        let mut has_scope = false;
        if !park_ids.is_empty() {
            builder.push("a.park_id IN (");
            for (index, id) in park_ids.iter().enumerate() {
                if index > 0 {
                    builder.push(", ");
                }
                builder.push_bind(*id);
            }
            builder.push(")");
            has_scope = true;
        }
        if let Some(codes) = allowed.as_ref()
            && !codes.is_empty()
        {
            if has_scope {
                builder.push(" OR ");
            }
            builder.push("a.device_id IN (SELECT ad.id FROM devices ad WHERE ad.device_id IN (");
            for (index, code) in codes.iter().enumerate() {
                if index > 0 {
                    builder.push(", ");
                }
                builder.push_bind(code.clone());
            }
            builder.push("))");
            has_scope = true;
        }
        if !has_scope {
            // 没有园区也没有设备：一条告警都不该看到
            builder.push("1 = 0");
        }
        builder.push(")");
    };

    let mut alert_list_builder = QueryBuilder::<Sqlite>::new(
        "SELECT a.id, a.alert_code, a.level, a.content, a.device_id, d.device_id AS device_code,
            a.park_id, p.park_name, a.status, a.occurred_at, a.handled_at, a.created_at
         FROM alerts a
         LEFT JOIN devices d ON d.id = a.device_id
         LEFT JOIN parks p ON p.id = a.park_id
         WHERE 1 = 1",
    );
    alert_scope(&mut alert_list_builder);
    alert_list_builder.push(
        " ORDER BY CASE a.status WHEN '未处理' THEN 0 WHEN '处理中' THEN 1 ELSE 2 END,
                  a.occurred_at DESC LIMIT 3",
    );
    let alerts = alert_list_builder
        .build_query_as::<Alert>()
        .fetch_all(&state.db)
        .await?;

    let mut pending_builder =
        QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM alerts a WHERE a.status = '未处理'");
    alert_scope(&mut pending_builder);
    let pending_alerts: i64 = pending_builder
        .build_query_scalar()
        .fetch_one(&state.db)
        .await?;

    let mut alert_total_builder =
        QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM alerts a WHERE 1 = 1");
    alert_scope(&mut alert_total_builder);
    let alert_total: i64 = alert_total_builder
        .build_query_scalar()
        .fetch_one(&state.db)
        .await?;

    // ── 任务趋势（按真实任务表聚合） ────────────────────────
    let (days, plan, done) = task_trend(&state, &claims).await?;

    // ── 最新动态：当前人的任务事件 + 当前人设备事件 + 告警 ──────
    let mut timeline_builder = QueryBuilder::<Sqlite>::new(
        "SELECT e.id, e.event_type, e.title, e.description, d.device_id AS device_code,
            t.task_code, e.created_at
         FROM task_events e
         LEFT JOIN devices d ON d.id = e.device_id
         LEFT JOIN tasks t ON t.id = e.task_id
         WHERE 1 = 1",
    );
    if !claims.is_admin {
        timeline_builder
            .push(" AND (t.created_by = ")
            .push_bind(claims.sub);
        if let Some(codes) = allowed.as_ref()
            && !codes.is_empty()
        {
            timeline_builder.push(" OR d.device_id IN (");
            for (index, code) in codes.iter().enumerate() {
                if index > 0 {
                    timeline_builder.push(", ");
                }
                timeline_builder.push_bind(code.clone());
            }
            timeline_builder.push(")");
        }
        timeline_builder.push(")");
    }
    timeline_builder.push(" ORDER BY e.created_at DESC, e.id DESC LIMIT 12");
    let mut timeline = timeline_builder
        .build_query_as::<TaskEvent>()
        .fetch_all(&state.db)
        .await?;

    // 告警也是真实动态的一部分（心跳超时、本体故障等未必会写 task_events）
    let mut alert_timeline_builder = QueryBuilder::<Sqlite>::new(
        "SELECT a.id, a.level, a.content, d.device_id AS device_code, p.park_name,
            a.status, a.occurred_at
         FROM alerts a
         LEFT JOIN devices d ON d.id = a.device_id
         LEFT JOIN parks p ON p.id = a.park_id
         WHERE 1 = 1",
    );
    alert_scope(&mut alert_timeline_builder);
    alert_timeline_builder.push(" ORDER BY a.occurred_at DESC, a.id DESC LIMIT 12");
    let alert_events: Vec<AlertActivityRow> = alert_timeline_builder
        .build_query_as::<AlertActivityRow>()
        .fetch_all(&state.db)
        .await?;
    timeline.extend(alert_events.into_iter().map(
        |(id, level, content, device_code, park_name, status, occurred_at)| TaskEvent {
            // 告警 id 与任务事件 id 来自不同表，加偏移避免前端 key 冲突
            id: 1_000_000 + id,
            event_type: "alert".to_string(),
            title: format!("{level}告警：{content}"),
            description: Some(format!(
                "{} · {status}",
                park_name.unwrap_or_else(|| "未归属园区".to_string())
            )),
            device_code,
            task_code: None,
            created_at: occurred_at,
        },
    ));
    timeline.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    timeline.truncate(6);

    // ── 态势图设备：当前人可见设备，前端按园区分组渲染 ─────────
    // 不再截断：园区态势要展示该园区下所有可见设备，点位由 `position` 换算。
    let map_devices = devices;

    Ok(ApiResponse::ok(json!({
        "kpis": {
            "onlineDevices": {
                "value": online,
                "total": device_total,
                "foot": format!("在线率 {}%", if device_total == 0 { 0 } else { online * 100 / device_total }),
            },
            "todayTasks": {
                "value": today_total,
                "completed": today_done,
                "rate": today_rate,
                "foot": format!("已完成 {} 项 · 完成率 {}%", today_done, today_rate),
            },
            "parks": {
                "value": park_count,
                "zones": zone_count,
                "area": area,
                "foot": format!("覆盖面积 {:.1} 万㎡", area),
            },
            "pendingAlerts": {
                "value": pending_alerts,
                "total": alert_total,
                "foot": format!("共 {} 条告警记录", alert_total),
            }
        },
        "deviceStatus": [
            {"name": "执行任务", "value": executing, "color": "#0284c7"},
            {"name": "在线待命", "value": standby, "color": "#10b981"},
            {"name": "充电中", "value": charging, "color": "#f59e0b"},
            {"name": "故障", "value": fault, "color": "#ef4444"},
            {"name": "待绑定", "value": unbound, "color": "#a78bfa"},
            {"name": "离线", "value": offline, "color": "#94a3b8"}
        ],
        "alerts": alerts,
        "trend": {"days": days, "plan": plan, "done": done},
        "timeline": timeline,
        "parks": parks,
        "mapDevices": map_devices,
        "totals": {
            "devices": device_total,
            "zones": zone_count,
            "tasks": task_total,
            "alerts": alert_total
        }
    })))
}
