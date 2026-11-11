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
    map_version_id.replace('@', "_")
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

/// 解析任务应绑定的地图
async fn resolve_map(db: &SqlitePool, task: &Task) -> Result<Option<DispatchMap>, AppError> {
    // 1) 路线冻结的地图版本
    // 2) 任务所属园区的当前基线
    let row: Option<(String, i64, String, String, Option<String>)> = sqlx::query_as(
        "SELECT mv.id, mv.park_id, mv.map_name, mv.version, mv.file_path
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

    let Some((map_id, park_id, map_name, version, _file_path)) = row else {
        return Ok(None);
    };

    Ok(Some(DispatchMap {
        map_path: to_map_dir_name(&map_id),
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
