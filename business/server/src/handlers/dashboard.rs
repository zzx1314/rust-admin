use crate::app::AppState;
use crate::error::{ApiResponse, AppError};
use crate::models::{Alert, Device, Park, TaskEvent};
use axum::Json;
use axum::extract::State;
use serde_json::{Value, json};

pub async fn summary(State(state): State<AppState>) -> Result<Json<ApiResponse<Value>>, AppError> {
    let device_rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT status, COUNT(*) FROM devices GROUP BY status")
            .fetch_all(&state.db)
            .await?;
    let device_total: i64 = device_rows.iter().map(|(_, count)| count).sum();
    let mut running = 0_i64;
    let mut charging = 0_i64;
    let mut offline = 0_i64;
    let mut fault = 0_i64;
    for (status, count) in &device_rows {
        match status.as_str() {
            "执行任务" | "在线待命" => running += count,
            "充电中" => charging += count,
            "故障" => fault += count,
            _ => offline += count,
        }
    }
    let online = running;

    let task_rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT task_status, COUNT(*) FROM tasks GROUP BY task_status")
            .fetch_all(&state.db)
            .await?;
    let task_total: i64 = task_rows.iter().map(|(_, count)| count).sum();
    let task_done = task_rows
        .iter()
        .find(|(status, _)| status == "已完成")
        .map(|(_, count)| *count)
        .unwrap_or(0);
    let task_rate = if task_total == 0 {
        0
    } else {
        task_done * 100 / task_total
    };

    let park_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parks")
        .fetch_one(&state.db)
        .await?;
    let zone_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM zones")
        .fetch_one(&state.db)
        .await?;
    let area: f64 = sqlx::query_scalar("SELECT COALESCE(SUM(area), 0) FROM parks")
        .fetch_one(&state.db)
        .await?;
    let pending_alerts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM alerts WHERE status = '未处理'")
            .fetch_one(&state.db)
            .await?;
    let agent_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM alerts")
        .fetch_one(&state.db)
        .await?;

    let parks = sqlx::query_as::<_, Park>(
        "SELECT p.id, p.park_code, p.park_name, p.park_description, p.park_address,
            p.area, p.owner_name, p.park_type, p.status, p.color, p.created_at, p.updated_at,
            (SELECT COUNT(*) FROM zones z WHERE z.park_id = p.id) AS zone_count,
            (SELECT COUNT(*) FROM devices d WHERE d.park_id = p.id) AS device_count,
            (SELECT COUNT(*) FROM tasks t WHERE t.park_id = p.id) AS task_count,
            (SELECT COUNT(*) FROM tasks t WHERE t.park_id = p.id AND t.task_status = '已完成') AS done_count
         FROM parks p ORDER BY p.id ASC",
    )
    .fetch_all(&state.db)
    .await?;

    let alerts = sqlx::query_as::<_, Alert>(
        "SELECT a.id, a.alert_code, a.level, a.content, a.device_id, d.device_id AS device_code,
            a.park_id, p.park_name, a.status, a.occurred_at, a.handled_at, a.created_at
         FROM alerts a
         LEFT JOIN devices d ON d.id = a.device_id
         LEFT JOIN parks p ON p.id = a.park_id
         ORDER BY CASE a.status WHEN '未处理' THEN 0 WHEN '处理中' THEN 1 ELSE 2 END,
                  a.occurred_at DESC
         LIMIT 3",
    )
    .fetch_all(&state.db)
    .await?;

    let trend_rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT stat_date, plan_count, done_count FROM (
            SELECT stat_date, plan_count, done_count FROM daily_stats
            ORDER BY stat_date DESC LIMIT 7
         ) ORDER BY stat_date ASC",
    )
    .fetch_all(&state.db)
    .await?;
    let days: Vec<String> = trend_rows
        .iter()
        .map(|(date, _, _)| date.get(5..).unwrap_or(date).to_string())
        .collect();
    let plan: Vec<i64> = trend_rows.iter().map(|(_, plan, _)| *plan).collect();
    let done: Vec<i64> = trend_rows.iter().map(|(_, _, done)| *done).collect();

    let timeline = sqlx::query_as::<_, TaskEvent>(
        "SELECT e.id, e.event_type, e.title, e.description, d.device_id AS device_code,
            t.task_code, e.created_at
         FROM task_events e
         LEFT JOIN devices d ON d.id = e.device_id
         LEFT JOIN tasks t ON t.id = e.task_id
         ORDER BY e.created_at DESC, e.id DESC LIMIT 6",
    )
    .fetch_all(&state.db)
    .await?;

    let map_devices = sqlx::query_as::<_, Device>(
        "SELECT d.id, d.device_id, d.model, d.park_id, p.park_name, d.zone_id, z.zone_name,
            d.status, d.battery, d.signal, d.temperature, d.runtime_minutes, d.odometer,
            d.position, d.current_task_name, d.last_heartbeat, d.firmware, d.device_ip,
            d.created_at, d.updated_at
         FROM devices d
         LEFT JOIN parks p ON p.id = d.park_id
         LEFT JOIN zones z ON z.id = d.zone_id
         WHERE d.status IN ('执行任务', '在线待命', '故障')
         ORDER BY d.id ASC LIMIT 6",
    )
    .fetch_all(&state.db)
    .await?;

    Ok(ApiResponse::ok(json!({
        "kpis": {
            "onlineDevices": {
                "value": online,
                "total": device_total,
                "foot": format!("在线率 {}%", if device_total == 0 { 0 } else { online * 100 / device_total }),
            },
            "todayTasks": {
                "value": task_total,
                "completed": task_done,
                "rate": task_rate,
                "foot": format!("已完成 {} 项 · 完成率 {}%", task_done, task_rate),
            },
            "parks": {
                "value": park_count,
                "zones": zone_count,
                "area": area,
                "foot": format!("覆盖面积 {:.1} 万㎡", area),
            },
            "pendingAlerts": {
                "value": pending_alerts,
                "total": agent_total,
                "foot": format!("共 {} 条告警记录", agent_total),
            }
        },
        "deviceStatus": [
            {"name": "在线运行", "value": running, "color": "#10b981"},
            {"name": "充电中", "value": charging, "color": "#f59e0b"},
            {"name": "离线", "value": offline, "color": "#94a3b8"},
            {"name": "故障", "value": fault, "color": "#ef4444"}
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
            "alerts": agent_total
        }
    })))
}
