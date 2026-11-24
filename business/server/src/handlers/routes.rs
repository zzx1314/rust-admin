use crate::app::AppState;
use crate::auth::Claims;
use crate::db::now;
use crate::error::{ApiResponse, AppError};
use crate::geo::{nearest_neighbor_order, route_metrics};
use crate::models::{Route, RouteQuery, RouteRequest};
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use serde_json::{Value, json};
use sqlx::QueryBuilder;

const ROUTE_SELECT: &str = "SELECT r.id, r.route_code, r.route_name, r.park_id, p.park_name,
    r.zone_id, z.zone_name, r.model, r.status, r.version, r.loop_mode, r.default_speed,
    r.route_description, r.route_data, r.point_count, r.distance_m, r.duration_s, r.battery_pct,
    r.published_at, r.map_version_id, mv.status AS map_status, r.created_at, r.updated_at,
    r.created_by,
    (SELECT COUNT(*) FROM tasks t WHERE t.route_id = r.id) AS used_task_count
    FROM routes r
    LEFT JOIN parks p ON p.id = r.park_id
    LEFT JOIN zones z ON z.id = r.zone_id
    LEFT JOIN map_versions mv ON mv.id = r.map_version_id";

pub async fn list_routes(
    State(state): State<AppState>,
    Query(query): Query<RouteQuery>,
    Extension(claims): Extension<Claims>,
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
    if !claims.is_admin {
        builder.push(" AND r.created_by = ").push_bind(claims.sub);
    }
    // 路线模板按最近更新倒序：任务规划的下拉框默认选中第一条，也应该是刚编排过的路线。
    builder.push(" ORDER BY r.updated_at DESC, r.id DESC");
    let routes = builder
        .build_query_as::<Route>()
        .fetch_all(&state.db)
        .await?;
    Ok(ApiResponse::ok(routes))
}

pub async fn get_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    let route = fetch_route(&state, id).await?;
    super::parks::ensure_owner(&route, &claims)?;
    Ok(ApiResponse::ok(route))
}

pub async fn create_route(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<RouteRequest>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    if payload.route_name.trim().is_empty() {
        return Err(AppError::BadRequest("路线名称不能为空".to_string()));
    }
    let next_id = crate::ids::next_id(&state.db, "routes", "routes").await?;
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
            duration_s, battery_pct, published_at, map_version_id, created_at, updated_at, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL, ?16, ?17, ?18, ?19)",
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
    .bind(claims.sub)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(
        fetch_route(&state, result.last_insert_rowid()).await?,
    ))
}

pub async fn update_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
    Json(payload): Json<RouteRequest>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    let existing = fetch_route(&state, id).await?;
    super::parks::ensure_owner(&existing, &claims)?;
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
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let route = fetch_route(&state, id).await?;
    super::parks::ensure_owner(&route, &claims)?;
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
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Value>>, AppError> {
    let route = fetch_route(&state, id).await?;
    super::parks::ensure_owner(&route, &claims)?;
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

    // 坐标语义与地图范围：智航 5000 的 goals 是地图米制，画布像素 / 示意坐标不能下发
    let frame = crate::coords::frame_for_map(&state.db, route.map_version_id.as_deref()).await?;
    let map_frame_ok = crate::coords::is_map_frame(&data) && !frame.schematic;
    let outside = crate::coords::points_outside(&data, &frame);
    let bounds_ok = map_frame_ok && outside.is_empty();
    let bounds_description = if !crate::coords::is_map_frame(&data) {
        "航点仍是画布像素坐标（缺 coordFrame=map），请在编辑器里重新保存一次".to_string()
    } else if frame.schematic {
        format!(
            "绑定的地图没有真实 ROS 地图包，按 {:.1} m/px 的示意坐标系估算，不能下发",
            crate::coords::SCHEMATIC_RESOLUTION
        )
    } else if outside.is_empty() {
        let (min_x, min_y, max_x, max_y) = frame.extents();
        format!("全部航点在地图范围内 x[{min_x:.1}, {max_x:.1}] y[{min_y:.1}, {max_y:.1}]")
    } else {
        let preview: Vec<String> = outside
            .iter()
            .take(3)
            .map(|(index, x, y)| format!("P{index}({x:.1}, {y:.1})"))
            .collect();
        format!(
            "{} 个航点超出地图范围：{}（米制坐标才能下发）",
            outside.len(),
            preview.join("、")
        )
    };

    // 航点净空（离最近障碍）：只有能解出地图栅格的路线才有结果，解不出就跳过这一项。
    // 「原地待命」的停车位就是最后一个航点，所以终点单独一项——2026-11-14 现场：
    // 终点离墙 0.3 m，本体反复报 `13377 无有效绕障轨迹`，任务永远到不了终态。
    let clearances = match super::maps::field_for_map(
        &state.db,
        &state.config.backend_db_path,
        route.map_version_id.as_deref(),
    )
    .await
    {
        Some(field) => super::maps::point_clearances(&field, &data),
        None => Vec::new(),
    };

    let mut items = vec![
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
            bounds_ok,
            "地图范围与坐标系".to_string(),
            bounds_description,
        ),
        validation_item(
            route.map_version_id.is_some(),
            "地图基线引用".to_string(),
            match route.map_version_id.as_deref() {
                Some(map_id) if frame.schematic => {
                    format!("{map_id}（演示基线，无真实地图包）")
                }
                Some(map_id) => format!("{map_id}（坐标随该版本冻结）"),
                None => "未绑定地图基线，无法下发".to_string(),
            },
        ),
    ];
    items.extend(clearance_items(&clearances));
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
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    let route = fetch_route(&state, id).await?;
    super::parks::ensure_owner(&route, &claims)?;
    if route.point_count < 2 {
        return Err(AppError::BadRequest("航点不足，无法发布".to_string()));
    }
    // 发布即意味着可被任务引用；坐标不能下发时宁可让运维当场知道，
    // 也不要发布一条「永远跑不起来」的路线。
    let data = route
        .route_data
        .as_deref()
        .map(parse_route_data)
        .unwrap_or_else(|| empty_route_data(&route.loop_mode));
    let frame = crate::coords::frame_for_map(&state.db, route.map_version_id.as_deref()).await?;
    if !crate::coords::is_map_frame(&data) {
        return Err(AppError::BadRequest(
            "航点还是画布像素坐标（缺 coordFrame=map），不能发布；请在编辑器里重新保存一次完成米制换算"
                .to_string(),
        ));
    }
    if frame.schematic {
        return Err(AppError::BadRequest(
            "绑定的地图没有真实 ROS 地图包（示意坐标系），不能发布；请先发布真实地图再编排路线"
                .to_string(),
        ));
    }
    if let Some((index, x, y)) = crate::coords::points_outside(&data, &frame).first() {
        return Err(AppError::BadRequest(format!(
            "第 {index} 个航点 ({x:.2}, {y:.2}) 超出地图范围，不能发布"
        )));
    }
    // 净空守卫：先把「点落在障碍上」这种谁都跑不了的情况拦在发布前（实测净空
    // <0.15 m 就是「目标被障碍物占据」，任何结束动作都规划不过去）；
    // 终点净空不足但还没到不可规划的程度时**只留痕不拦**——同一条路线也可能配
    // 「返回起点」，真正的硬拦点在下发前（客户端对「只剩最后一个点 + 反复规划失败」
    // 会提前报异常，见 `business/client/src/zhpro/route.rs`）。
    let mut clearance_note: Option<String> = None;
    if let Some(field) = super::maps::field_for_map(
        &state.db,
        &state.config.backend_db_path,
        route.map_version_id.as_deref(),
    )
    .await
    {
        let clearances = super::maps::point_clearances(&field, &data);
        if let Some(point) = clearances
            .iter()
            .find(|point| point.clearance_m < super::maps::MIN_FREE_CLEARANCE_M)
        {
            return Err(AppError::BadRequest(format!(
                "第 {} 个航点 ({:.2}, {:.2}) 落在障碍物上（离最近障碍 {:.2} m），\
                 本体规划不到该点，不能发布；请先在编辑器里把它挪到空地上",
                point.index, point.x, point.y, point.clearance_m
            )));
        }
        if let Some(last) = clearances
            .last()
            .filter(|point| point.clearance_m < super::maps::MIN_STOP_CLEARANCE_M)
        {
            clearance_note = Some(format!(
                "注意：终点 P{} 距最近障碍仅 {:.2} m（建议 ≥{:.1} m），\
                 「原地待命」的停车位就是这个点，本体可能停不进去",
                last.index,
                last.clearance_m,
                super::maps::MIN_STOP_CLEARANCE_M
            ));
            tracing::warn!(
                "路线 {} 发布：{}",
                route.route_code,
                clearance_note.as_deref().unwrap_or_default()
            );
        }
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
    .bind(match clearance_note.as_deref() {
        Some(note) => format!("路线：{}（{note}）", route.route_name),
        None => format!("路线：{}", route.route_name),
    })
    .bind(&timestamp)
    .execute(&state.db)
    .await?;
    Ok(ApiResponse::ok(fetch_route(&state, id).await?))
}

pub async fn optimize_route(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Extension(claims): Extension<Claims>,
) -> Result<Json<ApiResponse<Route>>, AppError> {
    let route = fetch_route(&state, id).await?;
    super::parks::ensure_owner(&route, &claims)?;
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

/// 净空相关的校验项（只有能解出地图栅格时才有）。
///
/// 两个口径分开报，因为处置方式不同：
/// - **终点停靠净空**：`endTaskMode = none`（原地待命）时最后一个航点就是停车位，本体
///   `end_task = 0` 要求它自己能站住；
/// - **航点净空**：中途点贴障碍同样规划不过去（`13377` 不只发生在终点）。
fn clearance_items(clearances: &[super::maps::PointClearance]) -> Vec<Value> {
    let Some(last) = clearances.last() else {
        return Vec::new();
    };
    let min = super::maps::MIN_STOP_CLEARANCE_M;
    let stop_ok = last.clearance_m >= min;
    let stop_description = if stop_ok {
        format!(
            "终点 P{} ({:.2}, {:.2}) 离最近障碍 {:.2} m，可原地停靠",
            last.index, last.x, last.y, last.clearance_m
        )
    } else {
        format!(
            "终点 P{} ({:.2}, {:.2}) 离最近障碍只有 {:.2} m（建议 ≥{min:.1} m）：\
             「原地待命」的停车位就是这个点，本体规划器会把它判成被占据，现场表现为反复\
             `13377 无有效绕障轨迹`、任务永远到不了终态。请把终点往空旷处挪 ≥{min:.1} m，\
             或把结束动作改成「返回起点」",
            last.index, last.x, last.y, last.clearance_m
        )
    };
    let tightest = clearances
        .iter()
        .min_by(|a, b| a.clearance_m.total_cmp(&b.clearance_m))
        .unwrap_or(last);
    let points_ok = tightest.clearance_m >= min;
    let points_description = if points_ok {
        format!(
            "最紧的航点 P{} 离最近障碍 {:.2} m",
            tightest.index, tightest.clearance_m
        )
    } else {
        format!(
            "航点 P{} ({:.2}, {:.2}) 离最近障碍 {:.2} m（建议 ≥{min:.1} m），\
             本体可能规划不出绕障轨迹",
            tightest.index, tightest.x, tightest.y, tightest.clearance_m
        )
    };
    vec![
        validation_item(stop_ok, "终点停靠净空".to_string(), stop_description),
        validation_item(points_ok, "航点净空".to_string(), points_description),
    ]
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

impl super::parks::OwnerCheck for Route {
    fn creator_id(&self) -> Option<i64> {
        self.created_by
    }
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
        let row = sqlx::query_as::<_, (String, Option<i64>, String)>(
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
        match (park_id, row.1) {
            // 设备地图还没归属园区：路线指定它就是把这张图认领给本园区
            (Some(park_id), None) => {
                sqlx::query("UPDATE map_versions SET park_id = ?1 WHERE id = ?2")
                    .bind(park_id)
                    .bind(&row.0)
                    .execute(&state.db)
                    .await?;
                tracing::info!("设备地图 {} 已归属园区 {park_id}（路线引用）", row.0);
            }
            (Some(park_id), Some(owner)) if owner != park_id => {
                return Err(AppError::BadRequest(
                    "地图基线不属于该园区，无法创建路线".to_string(),
                ));
            }
            (None, None) => {
                return Err(AppError::BadRequest(
                    "该地图尚未归属园区，请先创建园区并绑定地图".to_string(),
                ));
            }
            _ => {}
        }
        // 路线需要一个确定的园区：园区未知时用地图自己的归属反推
        let park_id = park_id.or(row.1);
        let Some(park_id) = park_id else {
            return Err(AppError::BadRequest(
                "该地图尚未归属园区，请先创建园区并绑定地图".to_string(),
            ));
        };
        return Ok(Some((row.0, park_id)));
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
    use super::clearance_items;
    use crate::handlers::maps::PointClearance;

    fn point(index: usize, clearance_m: f64) -> PointClearance {
        PointClearance {
            index,
            x: 1.0,
            y: 2.0,
            clearance_m,
        }
    }

    /// 终点就是「原地待命」的停车位：净空不足时必须体检不通过，且文案要说清楚原因
    #[test]
    fn tight_parking_spot_fails_the_end_point_item() {
        let items = clearance_items(&[point(1, 2.0), point(2, 0.32)]);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["title"], "终点停靠净空");
        assert_eq!(items[0]["ok"], false);
        assert!(
            items[0]["description"]
                .as_str()
                .unwrap_or_default()
                .contains("13377"),
            "文案要给出本体侧的现场现象"
        );
        // 0.32 m 也是最紧的那个航点，所以第二项一并不过
        assert_eq!(items[1]["ok"], false);
    }

    #[test]
    fn roomy_route_passes_both_items() {
        let items = clearance_items(&[point(1, 1.6), point(2, 0.9)]);
        assert_eq!(items[0]["ok"], true);
        assert_eq!(items[1]["ok"], true);
    }

    /// 中间点贴障碍也要报：`13377` 不只发生在终点
    #[test]
    fn tight_middle_point_is_reported_too() {
        let items = clearance_items(&[point(1, 0.2), point(2, 1.5)]);
        assert_eq!(items[0]["title"], "终点停靠净空");
        assert_eq!(items[0]["ok"], true, "终点本身够宽裕");
        assert_eq!(items[1]["title"], "航点净空");
        assert_eq!(items[1]["ok"], false);
    }

    #[test]
    fn empty_points_produce_no_items() {
        assert!(clearance_items(&[]).is_empty());
    }

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
