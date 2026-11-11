//! 业务服务下发载荷解析。
//!
//! 业务服务（`business/server`）的 `task_dispatch` 携带 `routeData`（前端三栏编辑器产出的
//! `points/legs/returnLeg`）。这里把它翻译成智航 Pro 可执行的 [`FlightPlan`]：
//!
//! ```text
//! routeData.points[]  → Waypoint（x/y 取地图坐标，heading 取朝向，dwell 取停留）
//! routeData.loop      → 闭环则在末尾回到首点
//! task.executeMode    → 结束动作（返回起点 / 返回充电 / 原地）
//! payload.map         → 地图（mapId / mapPath / mapType）
//! ```

use crate::zhpro::route::{EndTask, FlightPlan, Waypoint, WaypointAction};
use serde_json::Value;

/// 业务任务下发中的地图信息
#[derive(Debug, Clone, Default)]
pub struct MapBinding {
    pub map_id: String,
    pub map_path: String,
    /// 1 = 2D 栅格, 3 = Route 路网
    pub map_type: u32,
}

pub fn parse_map_binding(payload: &Value) -> MapBinding {
    let map = payload.get("map");
    MapBinding {
        map_id: map
            .and_then(|m| m.get("mapId"))
            .and_then(Value::as_str)
            .or_else(|| payload.get("mapId").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string(),
        map_path: map
            .and_then(|m| m.get("mapPath"))
            .and_then(Value::as_str)
            .or_else(|| payload.get("mapPath").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string(),
        map_type: map
            .and_then(|m| m.get("mapType"))
            .and_then(Value::as_u64)
            .or_else(|| payload.get("mapType").and_then(Value::as_u64))
            .unwrap_or(1) as u32,
    }
}

/// 任务类型 → 智航执行方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskMode {
    /// 建图（走 5000 kind=1）
    Mapping,
    /// 巡检 / 导航（走航线执行引擎）
    Route,
    /// 返回充电点（走 5014）
    ReturnToCharge,
    /// 回充对桩（走 5000 kind=3）
    Recharge,
    /// 出桩（走 5000 kind=5）
    Undock,
}

impl TaskMode {
    pub fn parse(task_type: &str) -> Self {
        match task_type.trim() {
            "建图" | "地图构建" | "mapping" => TaskMode::Mapping,
            "回充" | "充电" | "recharge" => TaskMode::Recharge,
            "出桩" | "undock" => TaskMode::Undock,
            "返航" | "返回充电" | "return" => TaskMode::ReturnToCharge,
            _ => TaskMode::Route,
        }
    }
}

/// 把业务下发载荷解析成飞行计划；`route_data` 缺失时返回 None
pub fn parse_flight_plan(payload: &Value, map_root: &str) -> Option<FlightPlan> {
    let task_code = payload
        .get("taskCode")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let task_name = payload
        .get("taskName")
        .and_then(Value::as_str)
        .unwrap_or("未命名任务")
        .to_string();
    let route_data = payload.get("routeData")?;
    let points = route_data.get("points").and_then(Value::as_array)?;
    if points.is_empty() {
        return None;
    }

    let binding = parse_map_binding(payload);
    let waypoints: Vec<Waypoint> = points
        .iter()
        .filter_map(|point| {
            let name = point
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("航点")
                .to_string();
            let x = point.get("x").and_then(Value::as_f64)?;
            let y = point.get("y").and_then(Value::as_f64)?;
            let actions = point
                .get("actions")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|action| WaypointAction {
                            action_type: action
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            param: action
                                .get("param")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            trigger: action
                                .get("trigger")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            threshold: action
                                .get("threshold")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            retry: action.get("retry").and_then(Value::as_i64).unwrap_or(1),
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(Waypoint {
                name,
                x,
                y,
                dwell: point.get("dwell").and_then(Value::as_i64).unwrap_or(0),
                heading: point.get("heading").and_then(Value::as_f64).unwrap_or(0.0),
                radius: point.get("radius").and_then(Value::as_f64).unwrap_or(0.3),
                must: point.get("must").and_then(Value::as_bool).unwrap_or(true),
                on_fail: point
                    .get("onFail")
                    .and_then(Value::as_str)
                    .unwrap_or("重试 2 次")
                    .to_string(),
                actions,
            })
        })
        .collect();
    if waypoints.is_empty() {
        return None;
    }

    let loop_mode = route_data
        .get("loop")
        .and_then(Value::as_str)
        .unwrap_or("往返")
        .to_string();
    let speed = payload
        .get("speedLimit")
        .and_then(Value::as_f64)
        .or_else(|| {
            route_data
                .get("legs")
                .and_then(Value::as_array)
                .and_then(|legs| legs.iter().find_map(|l| l.get("speed")?.as_f64()))
        })
        .unwrap_or(1.2);
    let end_task = parse_end_task(payload);
    let avoid_level = payload
        .get("avoidLevel")
        .and_then(Value::as_u64)
        .unwrap_or(40) as u32;

    let mut plan = FlightPlan {
        task_code,
        task_name,
        map_id: binding.map_id,
        map_path: binding.map_path,
        map_type: binding.map_type,
        waypoints,
        loop_mode,
        speed_limit: speed,
        avoid_level,
        goal_tolerance: 0.25,
        end_task: end_task.as_str().to_string(),
    };
    // mapPath 为空时按 map_root 推导一次，便于日志核对
    plan.map_path = plan.resolve_map_path(map_root);
    Some(plan)
}

fn parse_end_task(payload: &Value) -> EndTask {
    if let Some(value) = payload.get("endTask").and_then(Value::as_str) {
        let parsed = EndTask::parse(Some(value));
        if parsed != EndTask::None {
            return parsed;
        }
    }
    // executeMode: 单次 / 循环 / 返航 / 返航充电 等前端枚举
    // 注意顺序：「返航充电」包含「返航」，必须先判充电，否则会被误判成返回起点。
    let mode = payload
        .get("executeMode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mode.contains("返充") || mode.contains("返回充电") || mode.contains("充电") {
        EndTask::ReturnToCharge
    } else if mode.contains("返航") || mode.contains("返回起点") {
        EndTask::ReturnToStart
    } else {
        EndTask::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload() -> Value {
        json!({
            "taskCode": "TK-001",
            "taskName": "A区上午例行巡检",
            "taskType": "巡检",
            "executeMode": "返航充电",
            "map": { "mapId": "MAP-ZJ-01@v2.1", "mapType": 1 },
            "routeData": {
                "schemaVersion": 2,
                "loop": "闭环",
                "points": [
                    { "name": "起点", "x": 0.0, "y": 0.0, "dwell": 0, "heading": 0,
                      "must": true, "onFail": "跳过继续", "actions": [] },
                    { "name": "配电柜", "x": 5.0, "y": 2.0, "dwell": 15, "heading": 90,
                      "must": true, "onFail": "重试 2 次",
                      "actions": [{ "type": "可见光拍照", "param": "1080P" }] }
                ],
                "legs": [{ "mode": "沿道路", "speed": 1.5 }]
            }
        })
    }

    #[test]
    fn parses_plan_from_dispatch() {
        let plan = parse_flight_plan(&payload(), "/ota/alg_data/map").expect("plan");
        assert_eq!(plan.task_code, "TK-001");
        assert_eq!(plan.waypoints.len(), 2);
        assert_eq!(plan.waypoints[1].dwell, 15);
        assert!(plan.waypoints[1].wants_photo());
        assert_eq!(plan.loop_mode, "闭环");
        assert!((plan.speed_limit - 1.5).abs() < 1e-9);
        assert_eq!(plan.end_task(), EndTask::ReturnToCharge);
        assert_eq!(
            plan.map_path,
            "/ota/alg_data/map/history_map/MAP-ZJ-01_v2.1"
        );
        // 闭环仅在航点数 > 2 时补回首点；本例 2 个点保持原样
        assert_eq!(plan.execution_order().len(), 2);
    }

    #[test]
    fn missing_route_data_returns_none() {
        let mut value = payload();
        value.as_object_mut().unwrap().remove("routeData");
        assert!(parse_flight_plan(&value, "/m").is_none());
    }

    #[test]
    fn task_mode_mapping() {
        assert_eq!(TaskMode::parse("建图"), TaskMode::Mapping);
        assert_eq!(TaskMode::parse("巡检"), TaskMode::Route);
        assert_eq!(TaskMode::parse("回充"), TaskMode::Recharge);
        assert_eq!(TaskMode::parse("出桩"), TaskMode::Undock);
        assert_eq!(TaskMode::parse("返航"), TaskMode::ReturnToCharge);
    }
}
