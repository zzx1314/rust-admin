//! 任务下发载荷组装。
//!
//! `tasks.rs`（创建即推送）与 `gateway.rs`（设备上线补发）都需要把一条 `tasks` 记录
//! 组装成 [`TaskDispatchPayload`]。这里集中实现，避免两处字段漂移。
//!
//! 地图绑定规则：
//! 1. 优先使用路线冻结的 `routes.map_version_id`（路线与地图的一致性由发布时保证）；
//! 2. 路线未冻结地图时，回退到任务所属园区的当前基线 `parks.map_version_id`；
//! 3. 都没有则不下发 `map`，客户端会拒绝执行并在事件流中报错。

use crate::error::AppError;
use crate::gateway_types::{DispatchMap, TaskDispatchPayload};
use crate::models::Task;
use serde_json::{Value, json};
use sqlx::SqlitePool;

/// 智航本体地图目录名。
///
/// 业务侧地图版本 ID 形如 `ROS-MAP-3@v1.0`，`@` 在本体侧文件系统里不安全，
/// 统一替换为 `_`。与客户端 `zhpro::mapping::to_map_dir_name` 保持一致。
pub fn to_map_dir_name(map_version_id: &str) -> String {
    crate::mapdir::synthetic_dir(map_version_id)
}

/// 行程结束动作。
///
/// `executeMode` 为前端枚举（单次 / 循环 / 返航 / 返航充电），映射到智航侧
/// 的结束策略。注意「返航充电」包含「返航」，必须先判充电。
pub fn resolve_end_task(execute_mode: Option<&str>) -> &'static str {
    let mode = execute_mode.unwrap_or_default();
    if mode.contains("返充") || mode.contains("返回充电") || mode.contains("充电") {
        "return_to_charge"
    } else if mode.contains("返航") || mode.contains("返回起点") {
        "return_to_start"
    } else {
        "none"
    }
}

/// `resolve_map` 读取的地图行：(id, park, map_name, version, file_path, source_ref)
type MapRow = (String, i64, String, String, Option<String>, Option<String>);

/// 解析任务应绑定的地图
async fn resolve_map(db: &SqlitePool, task: &Task) -> Result<Option<DispatchMap>, AppError> {
    // 1) 路线冻结的地图版本
    // 2) 任务所属园区的当前基线
    // 本体目录名：`source_ref`（建图回传写入）优先，缺失才退回业务 ID 合成名。
    // 合成名在本体上可能根本不存在（目录名是建图会话名），客户端会用 4013
    // 列表核对，对不上就不发 5000。
    let row: Option<MapRow> = sqlx::query_as(
        "SELECT mv.id, mv.park_id, mv.map_name, mv.version, mv.file_path, mv.source_ref
         FROM map_versions mv
         WHERE mv.id = COALESCE(
             (SELECT map_version_id FROM routes WHERE id = ?1),
             (SELECT map_version_id FROM parks WHERE id = ?2)
         )",
    )
    .bind(task.route_id)
    .bind(task.park_id)
    .fetch_optional(db)
    .await?;

    let Some((map_id, park_id, map_name, version, _file_path, source_ref)) = row else {
        return Ok(None);
    };
    let map_dir = source_ref
        .as_deref()
        .and_then(crate::mapdir::dir_from_source_ref)
        .unwrap_or_else(|| crate::mapdir::synthetic_dir(&map_id));

    Ok(Some(DispatchMap {
        map_path: map_dir.clone(),
        map_dir,
        map_id,
        map_type: 1,
        park_id,
        map_name,
        version,
    }))
}

/// 组装一条任务的下发载荷
pub async fn build_dispatch_payload(
    db: &SqlitePool,
    task: &Task,
) -> Result<TaskDispatchPayload, AppError> {
    // 路线数据与速度上限
    let route_row: Option<(Option<String>, Option<f64>, Option<String>)> = match task.route_id {
        Some(route_id) => {
            sqlx::query_as("SELECT route_data, default_speed, loop_mode FROM routes WHERE id = ?1")
                .bind(route_id)
                .fetch_optional(db)
                .await?
        }
        None => None,
    };
    let (route_data_text, default_speed, _loop_mode) = route_row.unwrap_or((None, None, None));
    let route_data: Option<Value> = route_data_text.and_then(|s| serde_json::from_str(&s).ok());

    let map = resolve_map(db, task).await?;

    // 坐标守卫：航点必须是地图米制且在所选地图范围内（详见 [`check_route_data`]）。
    if let Some(data) = route_data.as_ref() {
        let frame = crate::coords::frame_for_map(db, map.as_ref().map(|item| item.map_id.as_str()))
            .await?;
        check_route_data(task.route_code.as_deref().unwrap_or("—"), data, &frame)?;
    }

    Ok(TaskDispatchPayload {
        task_id: task.id,
        task_code: task.task_code.clone(),
        task_name: task.task_name.clone(),
        task_type: task.task_type.clone(),
        execute_mode: task.execute_mode.clone(),
        route: route_data.as_ref().map(|_| {
            json!({
                "routeId": task.route_id,
                "routeCode": task.route_code,
                "routeName": task.route_name,
            })
        }),
        route_data,
        plan_start: task.plan_start.clone(),
        plan_end: task.plan_end.clone(),
        dispatch_policy: task.dispatch_policy.clone(),
        description: task.description.clone(),
        map,
        speed_limit: default_speed,
        avoid_level: Some(40),
        end_task: Some(resolve_end_task(task.execute_mode.as_deref()).to_string()),
    })
}

/// 航点坐标能否下发：米制标记 + 非示意坐标系 + 全部落在图内。
///
/// 智航 5000 kind=2 的 `goals[].position` 是地图坐标系下的米（协议 §2.5.1.4），
/// 而路线编辑器用的是 800×460 画布像素：没有这道检查，`x=335` 会被当成 335 米
/// 发给本体，整条航线落到图外，现场只能看到一句 `invalid request parameters`。
pub fn check_route_data(
    route_code: &str,
    data: &Value,
    frame: &crate::coords::MapFrame,
) -> Result<(), AppError> {
    if !crate::coords::is_map_frame(data) {
        return Err(AppError::BadRequest(format!(
            "路线 {route_code} 的航点还是画布像素坐标（缺 `coordFrame=map` 标记），不能下发；\
             请在路线编辑器里重新保存一次完成米制换算"
        )));
    }
    if frame.schematic {
        return Err(AppError::BadRequest(format!(
            "路线 {route_code} 绑定的地图没有真实 ROS 地图包（示意坐标系，1 画布像素按 {:.1} m 估算），\
             无法下发；请先在地图基线里发布真实地图再重新编排路线",
            crate::coords::SCHEMATIC_RESOLUTION
        )));
    }
    if let Some((index, x, y)) = crate::coords::points_outside(data, frame).first() {
        let (min_x, min_y, max_x, max_y) = frame.extents();
        return Err(AppError::BadRequest(format!(
            "路线 {route_code} 的第 {index} 个航点 ({x:.2}, {y:.2}) 超出地图范围\
             x[{min_x:.1}, {max_x:.1}] y[{min_y:.1}, {max_y:.1}]，已拒绝下发"
        )));
    }
    Ok(())
}

/// 建任务前的坐标预检：避免「任务插入成功但下发报错」的半截状态。
///
/// 地图解析顺序与 [`resolve_map`] 保持一致：路线冻结版本 → 园区当前基线。
pub async fn ensure_route_dispatchable(
    db: &SqlitePool,
    route_id: Option<i64>,
    park_id: Option<i64>,
) -> Result<(), AppError> {
    let Some(route_id) = route_id else {
        return Ok(());
    };
    let row: Option<(Option<String>, String)> =
        sqlx::query_as("SELECT route_data, route_code FROM routes WHERE id = ?1")
            .bind(route_id)
            .fetch_optional(db)
            .await?;
    let Some((route_data, route_code)) = row else {
        return Ok(());
    };
    let Some(raw) = route_data else {
        return Ok(());
    };
    let Ok(data) = serde_json::from_str::<Value>(&raw) else {
        return Ok(());
    };
    // 与 resolve_map 相同的 COALESCE：路线冻结版本优先，否则园区基线
    let map_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM map_versions WHERE id = COALESCE(
            (SELECT map_version_id FROM routes WHERE id = ?1),
            (SELECT map_version_id FROM parks WHERE id = ?2))",
    )
    .bind(route_id)
    .bind(park_id)
    .fetch_optional(db)
    .await?;
    let frame = crate::coords::frame_for_map(db, map_id.as_deref()).await?;
    check_route_data(&route_code, &data, &frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_dir_name_replaces_at_sign() {
        assert_eq!(to_map_dir_name("ROS-MAP-3@v1.0"), "ROS-MAP-3_v1.0");
        assert_eq!(to_map_dir_name("MAP-ZJ-01"), "MAP-ZJ-01");
    }

    #[test]
    fn end_task_prefers_charge_over_return() {
        assert_eq!(resolve_end_task(Some("返航充电")), "return_to_charge");
        assert_eq!(resolve_end_task(Some("返航")), "return_to_start");
        assert_eq!(resolve_end_task(Some("单次")), "none");
        assert_eq!(resolve_end_task(None), "none");
    }
}
