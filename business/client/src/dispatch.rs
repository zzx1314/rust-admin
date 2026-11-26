//! 业务服务下发载荷解析。
//!
//! 业务服务（`business/server`）的 `task_dispatch` 携带 `routeData`（前端三栏编辑器产出的
//! `points/legs/returnLeg`）。这里把它翻译成智航 Pro 可执行的 [`FlightPlan`]：
//!
//! ```text
//! routeData.points[]  → Waypoint（x/y 取地图坐标，heading 取到点机身偏航角，dwell 取停留）
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
    /// 服务端给出的本体目录名 / 路径（可能缺失，客户端会再用 4013 列表核对）
    pub map_path: String,
    /// 本体地图目录名（4013 `path` 最后一段）；`mapPath` 是路径时用它
    pub map_dir: String,
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
        map_dir: map
            .and_then(|m| m.get("mapDir"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                map.and_then(|m| m.get("mapPath"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default(),
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
                            record_seconds: action
                                .get("recordSeconds")
                                .and_then(Value::as_u64)
                                .map(|value| value as u32),
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
    let (end_task, end_task_explicit) = parse_end_task(payload);
    let avoid_level = payload
        .get("avoidLevel")
        .and_then(Value::as_u64)
        .unwrap_or(40) as u32;

    // 断点续跑起点：业务平台「从断点继续」时带 `startIndex`（0 基，routeData.points 下标）。
    // 越界不在这里夹：`RouteRunner::run_patrol` 会拒绝执行并把原因报回平台。
    let start_index = payload
        .get("startIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
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
        end_task_explicit,
        // 官方 App 的 cruise_mode 是整条任务级开关：只有所有航段都写
        // “沿道路”才走路网；任意一段是直线/曲线平滑/原地转向就按自由导航，
        // 避免混合路线被整体强推到路网。
        road_network: route_data
            .get("legs")
            .and_then(Value::as_array)
            .map(|legs| {
                !legs.is_empty()
                    && legs
                        .iter()
                        .all(|leg| leg.get("mode").and_then(Value::as_str) == Some("沿道路"))
            })
            .unwrap_or(false),
        fence_path: payload
            .get("fencePath")
            .and_then(Value::as_str)
            .or_else(|| route_data.get("fencePath").and_then(Value::as_str))
            .or_else(|| route_data.get("fence_path").and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        start_index,
    };
    // mapPath 为空时按 map_root 推导一次，便于日志核对
    plan.map_path = plan.resolve_map_path(map_root);
    Some(plan)
}

/// 解析任务结束动作，返回 `(结束动作, 是否为任务级显式配置)`。
///
/// 优先级：`endTaskMode`（新版服务端，仅显式配置才带）> `endTask` > `executeMode` 文案。
/// 只有第一项能表达「显式原地」：老服务端对**所有**任务都发 `endTask: "none"`，客户端
/// 只能把它当成「没指定」再按闭环兜底返回起点，所以不能把 `none` 的语义挂在 `endTask` 上。
fn parse_end_task(payload: &Value) -> (EndTask, bool) {
    if let Some(value) = payload.get("endTaskMode").and_then(Value::as_str) {
        let value = value.trim();
        if !value.is_empty() {
            return (EndTask::parse(Some(value)), true);
        }
    }
    if let Some(value) = payload.get("endTask").and_then(Value::as_str)
        && !value.trim().is_empty()
    {
        let parsed = EndTask::parse(Some(value));
        if parsed != EndTask::None {
            return (parsed, true);
        }
    }
    // executeMode: 单次 / 循环 / 返航 / 返航充电 等前端枚举
    // 注意顺序：「返航充电」包含「返航」，必须先判充电，否则会被误判成返回起点。
    let mode = payload
        .get("executeMode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mode.contains("返充") || mode.contains("返回充电") || mode.contains("充电") {
        (EndTask::ReturnToCharge, true)
    } else if mode.contains("返航") || mode.contains("返回起点") {
        (EndTask::ReturnToStart, true)
    } else {
        (EndTask::None, false)
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
        assert!(plan.end_task_explicit, "显式配置才算 explicit");
        assert_eq!(
            plan.map_path,
            "/ota/alg_data/map/history_map/MAP-ZJ-01_v2.1"
        );
        // 闭环仅在航点数 > 2 时补回首点；本例 2 个点保持原样
        assert_eq!(plan.waypoints.len(), 2);
    }

    #[test]
    fn task_level_none_overrides_closed_loop() {
        // 新版服务端：任务显式配了「原地」→ 载荷带 endTaskMode
        let mut value = payload();
        value["endTaskMode"] = json!("none");
        value["endTask"] = json!("none");
        value["routeData"]["loop"] = json!("闭环");
        let plan = parse_flight_plan(&value, "/m").expect("plan");
        assert_eq!(plan.end_task(), EndTask::None);
        assert!(plan.end_task_explicit, "显式原地不能被闭环兜底");

        // 老服务端（没有 endTaskMode、endTask=none）→ 跟随路线，不算显式
        let mut value = payload();
        value["endTask"] = json!("none");
        value["executeMode"] = json!("周期任务");
        let plan = parse_flight_plan(&value, "/m").expect("plan");
        assert_eq!(plan.end_task(), EndTask::None);
        assert!(!plan.end_task_explicit, "没配置就交给闭环兜底");
    }

    #[test]
    fn start_index_is_parsed_for_breakpoint_resume() {
        // 平台「从断点继续」：载荷带 startIndex = 断点航点下标（0 基）
        let mut value = payload();
        value["startIndex"] = json!(1);
        let plan = parse_flight_plan(&value, "/m").expect("plan");
        assert_eq!(plan.start_index, 1);
        assert_eq!(plan.resume_offset(), Some(1));

        // 缺省 / 0 / 负数都按「从起点跑」，老平台不发这个字段行为不变
        for raw in [None, Some(json!(0)), Some(json!(-2))] {
            let mut value = payload();
            if let Some(raw) = raw {
                value["startIndex"] = raw;
            }
            let plan = parse_flight_plan(&value, "/m").expect("plan");
            assert_eq!(plan.start_index, 0, "缺省 = 从起点跑");
            assert_eq!(plan.resume_offset(), Some(0));
        }

        // 越界（断点 >= 航点数）不能静默夹到最后一个点：交给 runner 拒绝执行
        let mut value = payload();
        value["startIndex"] = json!(9);
        let plan = parse_flight_plan(&value, "/m").expect("plan");
        assert_eq!(plan.resume_offset(), None);
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
