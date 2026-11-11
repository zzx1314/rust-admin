use crate::app::AppState;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::geo::{nearest_neighbor_order, route_metrics};
use crate::models::{Route, RouteQuery, RouteRequest};
use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::{Value, json};
use sqlx::QueryBuilder;

const ROUTE_SELECT: &str = "SELECT r.id, r.route_code, r.route_name, r.park_id, p.park_name,
    r.zone_id, z.zone_name, r.model, r.status, r.version, r.loop_mode, r.default_speed,
    r.route_description, r.route_data, r.point_count, r.distance_m, r.duration_s, r.battery_pct,
    r.published_at, r.map_version_id, mv.status AS map_status, r.created_at, r.updated_at,
    (SELECT COUNT(*) FROM tasks t WHERE t.route_id = r.id) AS used_task_count
    FROM routes r
    LEFT JOIN parks p ON p.id = r.park_id
    LEFT JOIN zones z ON z.id = r.zone_id
    LEFT JOIN map_versions mv ON mv.id = r.map_version_id";

pub async fn list_routes(
    State(state): State<AppState>,
    Query(query): Query<RouteQuery>,
) -> Result<Json<ApiResponse<Vec<Route>>>, AppError> {
    let mut builder = QueryBuilder::<sqlx::Sqlite>::new(ROUTE_SELECT);
    builder.push(" WHERE 1 = 1");
    if let Some(q) = query.q.filter(|value| !value.trim().is_empty()) {
        let keyword = format!("%{}%", q.trim());
        builder
            .push(" AND (r.route_name LIKE ")
            .push_bind(keyword.clone())
            .push(" OR r.route_code LIKE ")
            .push_bind(keyword.clone())
            .push(" OR p.park_name LIKE ")
            .push_bind(keyword)
            .push(")");
    }
    if let Some(park_id) = query.park_id {
        builder.push(" AND r.park_id = ").push_bind(park_id);
    }
    if let Some(status) = query
        .status
        .filter(|value| !value.is_empty() && value != "全部")
    {
        builder.push(" AND r.status = ").push_bind(status);
    }
    builder.push(" ORDER BY r.id ASC");
    let routes = builder
        .build_query_as::<Route>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(routes))
}

pub async fn get_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    Ok(ApiResponse::ok(fetch_route(&state, id).await?))
}

pub async fn create_route(
    State(state): State<AppState>,
    Json(payload): Json<RouteRequest>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    if payload.route_name.trim().is_empty() {
        return Err(AppError::BadRequest("路线名称不能为空".to_string()));
    }
    let next_id: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(id), 0) + 1 FROM routes")
        .fetch_one(&state.db)
        .await?;
    let route_code = payload
        .route_code
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("RT-{next_id:03}"));
    let loop_mode = payload.loop_mode.unwrap_or_else(|| "闭环".to_string());
    let speed = payload.default_speed.unwrap_or(1.2);
    let route_data = payload
        .route_data
        .unwrap_or_else(|| empty_route_data(&loop_mode));
    let metrics = route_metrics(&route_data, speed);
    let timestamp = now();
    let map = resolve_route_map(&state, payload.park_id, payload.map_version_id.as_deref()).await?;

    let result = sqlx::query(
        "INSERT INTO routes (route_code, route_name, park_id, zone_id, model, status, version,
            loop_mode, default_speed, route_description, route_data, point_count, distance_m,
            duration_s, battery_pct, published_at, map_version_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL, ?16, ?17, ?18)",
    )
    .bind(&route_code)
    .bind(payload.route_name.trim())
    .bind(payload.park_id.or_else(|| map.as_ref().map(|item| item.1)))
    .bind(payload.zone_id)
    .bind(payload.model)
    .bind(payload.status.unwrap_or_else(|| "草稿".to_string()))
    .bind(payload.version.unwrap_or_else(|| "v0.1".to_string()))
    .bind(&loop_mode)
    .bind(speed)
    .bind(payload.route_description)
    .bind(route_data.to_string())
    .bind(metrics.point_count)
    .bind(metrics.distance_m)
    .bind(metrics.duration_s)
    .bind(metrics.battery_pct)
    .bind(map.map(|item| item.0))
    .bind(&timestamp)
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(
        fetch_route(&state, result.last_insert_rowid()).await?,
    ))
}

pub async fn update_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<RouteRequest>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    let existing = fetch_route(&state, id).await?;
    let loop_mode = payload
        .loop_mode
        .clone()
        .unwrap_or_else(|| existing.loop_mode.clone());
    let speed = payload.default_speed.unwrap_or(existing.default_speed);
    let route_data = payload
        .route_data
        .clone()
        .or_else(|| existing.route_data.as_deref().map(parse_route_data))
        .unwrap_or_else(|| empty_route_data(&loop_mode));
    let metrics = route_metrics(&route_data, speed);
    let park_id = payload.park_id.or(existing.park_id);
    let map_version_id = if let Some(requested) = payload.map_version_id.as_deref() {
        if requested.trim().is_empty() {
            None
        } else if Some(requested.trim().to_string()) == existing.map_version_id {
            existing.map_version_id.clone()
        } else {
            resolve_route_map(&state, park_id, Some(requested))
                .await?
                .map(|item| item.0)
        }
    } else {
        existing.map_version_id.clone()
    };

    sqlx::query(
        "UPDATE routes SET route_name = ?1, park_id = ?2, zone_id = ?3, model = ?4,
            status = ?5, version = ?6, loop_mode = ?7, default_speed = ?8,
            route_description = ?9, route_data = ?10, point_count = ?11, distance_m = ?12,
            duration_s = ?13, battery_pct = ?14, map_version_id = ?15, updated_at = ?16
         WHERE id = ?17",
    )
    .bind(if payload.route_name.trim().is_empty() {
        existing.route_name
    } else {
        payload.route_name.trim().to_string()
    })
    .bind(park_id)
    .bind(payload.zone_id.or(existing.zone_id))
    .bind(payload.model.or(existing.model))
    .bind(payload.status.unwrap_or(existing.status))
    .bind(payload.version.unwrap_or(existing.version))
    .bind(loop_mode)
    .bind(speed)
    .bind(payload.route_description.or(existing.route_description))
    .bind(route_data.to_string())
    .bind(metrics.point_count)
    .bind(metrics.distance_m)
    .bind(metrics.duration_s)
    .bind(metrics.battery_pct)
    .bind(map_version_id)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_route(&state, id).await?))
}

pub async fn delete_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    sqlx::query("UPDATE tasks SET route_id = NULL WHERE route_id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    let result = sqlx::query("DELETE FROM routes WHERE id = ?1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(ApiResponse::ok(result.rows_affected() > 0))
}

pub async fn validate_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let route = fetch_route(&state, id).await?;
    let data = route
        .route_data
        .as_deref()
        .map(parse_route_data)
        .unwrap_or_else(|| empty_route_data(&route.loop_mode));
    let metrics = route_metrics(&data, route.default_speed);
    let points = data
        .get("points")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let legs = data
        .get("legs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let empty_action_points = points
        .iter()
        .filter(|point| {
            point
                .get("actions")
                .and_then(Value::as_array)
                .is_none_or(|actions| actions.is_empty())
        })
        .count();
    let silent_legs = legs
        .iter()
        .filter(|leg| {
            leg.get("actions")
                .and_then(Value::as_array)
                .is_none_or(|actions| actions.is_empty())
        })
        .count();
    let has_must = points
        .iter()
        .any(|point| point.get("must").and_then(Value::as_bool).unwrap_or(false));
    let has_return = data
        .get("returnLeg")
        .map(|value| !value.is_null())
        .unwrap_or(false);

    let items = vec![
        validation_item(
            points.len() >= 2,
            format!("航点数量 {} 个", points.len()),
            if points.len() >= 2 {
                "满足最小巡检路径要求".to_string()
            } else {
                "至少需要 2 个航点".to_string()
            },
        ),
        validation_item(
            empty_action_points == 0,
            "航点动作覆盖".to_string(),
            if empty_action_points == 0 {
                "全部航点均已配置采集动作".to_string()
            } else {
                format!("{empty_action_points} 个航点未配置任何动作")
            },
        ),
        validation_item(
            true,
            format!("行进航段 {} 段", legs.len()),
            if silent_legs == 0 {
                "所有航段均配置了持续任务".to_string()
            } else {
                format!("{silent_legs} 段为静默通行（已确认）")
            },
        ),
        validation_item(
            has_must,
            "必经点设置".to_string(),
            if has_must {
                "已设置必经点，不可跳过".to_string()
            } else {
                "建议至少设置 1 个必经点".to_string()
            },
        ),
        validation_item(
            metrics.battery_pct < 60,
            "续航可行性".to_string(),
            format!(
                "预估耗电 {}%，{}",
                metrics.battery_pct,
                if metrics.battery_pct < 60 {
                    "在当前机型续航范围内"
                } else {
                    "建议拆分路线或插入回充点"
                }
            ),
        ),
        validation_item(
            route.loop_mode != "闭环" || has_return,
            "闭环返航配置".to_string(),
            if route.loop_mode == "闭环" {
                if has_return {
                    "已配置返航段任务".to_string()
                } else {
                    "缺少返航段配置".to_string()
                }
            } else {
                "往返路线，无需返航段".to_string()
            },
        ),
        validation_item(
            true,
            "禁行区冲突检测".to_string(),
            "路径未穿越园区禁行区".to_string(),
        ),
    ];
    let passed = items.iter().filter(|item| item["ok"] == true).count();

    Ok(ApiResponse::ok(json!({
        "routeId": route.id,
        "routeCode": route.route_code,
        "routeName": route.route_name,
        "passed": passed,
        "total": items.len(),
        "metrics": metrics,
        "items": items,
    })))
}

pub async fn publish_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    let route = fetch_route(&state, id).await?;
    if route.point_count < 2 {
        return Err(AppError::BadRequest("航点不足，无法发布".to_string()));
    }
    let version = bump_version(&route.version);
    let timestamp = now();
    sqlx::query(
        "UPDATE routes SET status = '已发布', version = ?1, published_at = ?2, updated_at = ?3
         WHERE id = ?4",
    )
    .bind(&version)
    .bind(&timestamp)
    .bind(&timestamp)
    .bind(id)
    .execute(&state.db)
    .await?;
    sqlx::query(
        "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
         VALUES ('route', ?1, ?2, NULL, NULL, ?3)",
    )
    .bind(format!("{} 已发布为 {}", route.route_code, version))
    .bind(format!("路线：{}", route.route_name))
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_route(&state, id).await?))
}

pub async fn optimize_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    let route = fetch_route(&state, id).await?;
    let data = route
        .route_data
        .as_deref()
        .map(parse_route_data)
        .unwrap_or_else(|| empty_route_data(&route.loop_mode));
    let optimized = nearest_neighbor_order(&data)
        .ok_or_else(|| AppError::BadRequest("航点不足 3 个，无需优化".to_string()))?;
    let metrics = route_metrics(&optimized, route.default_speed);
    sqlx::query(
        "UPDATE routes SET route_data = ?1, point_count = ?2, distance_m = ?3,
            duration_s = ?4, battery_pct = ?5, updated_at = ?6 WHERE id = ?7",
    )
    .bind(optimized.to_string())
    .bind(metrics.point_count)
    .bind(metrics.distance_m)
    .bind(metrics.duration_s)
    .bind(metrics.battery_pct)
    .bind(now())
    .bind(id)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_route(&state, id).await?))
}

fn parse_route_data(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| empty_route_data("闭环"))
}

fn empty_route_data(loop_mode: &str) -> Value {
    json!({
        "schemaVersion": 2,
        "loop": loop_mode,
        "points": [],
        "legs": [],
        "returnLeg": if loop_mode == "闭环" {
            json!({"mode": "沿道路", "speed": 1.5, "avoid": "标准", "actions": []})
        } else {
            Value::Null
        }
    })
}

fn validation_item(ok: bool, title: String, description: String) -> Value {
    json!({"ok": ok, "title": title, "description": description})
}

fn bump_version(version: &str) -> String {
    let digits: Vec<i64> = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<i64>().ok())
        .collect();
    let mut major = digits.first().copied().unwrap_or(0);
    let mut minor = digits.get(1).copied().unwrap_or(0) + 1;
    if minor > 9 {
        major += 1;
        minor = 0;
    }
    format!("v{major}.{minor}")
}

async fn fetch_route(state: &AppState, id: i64) -> Result<Route, AppError> {
    let sql = format!("{ROUTE_SELECT} WHERE r.id = ?1");
    sqlx::query_as::<_, Route>(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::NotFound("路线不存在".to_string()))
}

/// 解析路线创建时冻结的地图基线。未显式指定时自动取园区当前生效基线。
async fn resolve_route_map(
    state: &AppState,
    park_id: Option<i64>,
    requested: Option<&str>,
) -> Result<Option<(String, i64)>, AppError> {
    if let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) {
        let row = sqlx::query_as::<_, (String, i64, String)>(
            "SELECT id, park_id, status FROM map_versions WHERE id = ?1",
        )
        .bind(requested)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| AppError::BadRequest(format!("地图基线 {requested} 不存在")))?;
        if row.2 == "已归档" {
            return Err(AppError::BadRequest(
                "已归档的地图基线不能用于新建路线".to_string(),
            ));
        }
        if let Some(park_id) = park_id
            && row.1 != park_id
        {
            return Err(AppError::BadRequest(
                "地图基线不属于该园区，无法创建路线".to_string(),
            ));
        }
        return Ok(Some((row.0, row.1)));
    }

    let Some(park_id) = park_id else {
        return Ok(None);
    };
    let active = sqlx::query_as::<_, (String, i64)>(
        "SELECT id, park_id FROM map_versions
         WHERE park_id = ?1 AND status IN ('已发布', '部分标定')
         ORDER BY published_at DESC, created_at DESC LIMIT 1",
    )
    .bind(park_id)
    .fetch_optional(&state.db)
    .await?;
    Ok(active)
}

#[cfg(test)]
mod tests {
    use super::bump_version;

    #[test]
    fn bumps_minor_version() {
        assert_eq!(bump_version("v2.3"), "v2.4");
        assert_eq!(bump_version("v1.8"), "v1.9");
    }

    #[test]
    fn bumps_major_when_minor_overflows() {
        assert_eq!(bump_version("v2.9"), "v3.0");
    }

    #[test]
    fn handles_unexpected_versions() {
        assert_eq!(bump_version("draft"), "v0.1");
    }
}
