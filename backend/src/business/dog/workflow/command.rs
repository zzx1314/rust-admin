//! 把流程设计（nodes / edges）转换成下发给设备的指令 JSON。
//!
//! 结构与 mgmt_platform 的 `DronesCommandWebsocket` 保持一致：
//!
//! ```json
//! {
//!   "type": "toros",
//!   "deviceId": "<设备编号>",
//!   "route": [{ "wpId": 1, "lat": 0.0, "lon": 0.0, "alt": 0.5 }],
//!   "tasks": [
//!     {
//!       "taskId": "node_x",
//!       "fromWp": { ... },
//!       "toWp": { ... },
//!       "event": "init_x",
//!       "actions": [
//!         { "actionId": "node_y", "after": "init_x", "type": "TAKEOFF", "params": { ... } }
//!       ]
//!     }
//!   ],
//!   "services": [{ "type": "YOLO", "params": { ... } }],
//!   "onErrorActions": []
//! }
//! ```

use std::collections::{HashMap, HashSet};

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::business::dog::device::entity::{Column as DeviceColumn, Entity as DeviceEntity};
use crate::business::dog::route_library::entity::{Column as RouteColumn, Entity as RouteEntity};
use crate::business::dog::services::entity::{Column as ServiceColumn, Entity as ServiceEntity};
use crate::common::error::AppError;

#[derive(Debug, Deserialize)]
struct FlowNode {
    id: String,
    #[serde(rename = "type")]
    node_type: Option<String>,
    #[serde(default)]
    data: Value,
}

#[derive(Debug, Deserialize)]
struct FlowEdge {
    source: String,
    target: String,
}

#[derive(Debug, Clone)]
struct RoutePoint {
    wp_id: i64,
    lat: f64,
    lon: f64,
    alt: f64,
}

impl RoutePoint {
    fn to_value(&self) -> Value {
        json!({
            "wpId": self.wp_id,
            "lat": self.lat,
            "lon": self.lon,
            "alt": self.alt,
        })
    }
}

/// 生成指令 JSON。流程里没有任务节点时返回 `None`（保存仍然允许）。
pub async fn build_command_json(
    conn: &DatabaseConnection,
    nodes_json: Option<&str>,
    edges_json: Option<&str>,
) -> Result<Option<String>, AppError> {
    let nodes: Vec<FlowNode> = parse_nodes(nodes_json);
    let edges: Vec<FlowEdge> = parse_edges(edges_json);
    if nodes.is_empty() {
        return Ok(None);
    }

    let node_map: HashMap<&str, &FlowNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut edge_map: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in &edges {
        edge_map
            .entry(edge.source.as_str())
            .or_default()
            .push(edge.target.as_str());
    }

    let Some(start_id) = nodes
        .iter()
        .find(|n| n.node_type.as_deref() == Some("start"))
        .map(|n| n.id.as_str())
    else {
        return Ok(None);
    };

    let task_ids = collect_task_ids(&node_map, &edge_map, start_id);
    if task_ids.is_empty() {
        return Ok(None);
    }

    // 首个任务节点决定设备与航线
    let first_task = node_map
        .get(task_ids[0])
        .copied()
        .expect("task node must exist");
    let device_id = first_task.data.get("deviceId").and_then(Value::as_i64);
    let route_id = first_task.data.get("routeId").and_then(Value::as_i64);

    let device_code = match device_id {
        Some(id) => find_device_code(conn, id).await?,
        None => String::new(),
    };

    let route = match route_id {
        Some(id) if id > 0 => find_route_points(conn, id).await?,
        _ => Vec::new(),
    };

    let mut tasks = Vec::new();
    let mut error_actions: Vec<Value> = Vec::new();
    let mut service_types: HashSet<String> = HashSet::new();
    let mut action_values: Vec<Value> = Vec::new();

    for task_id in &task_ids {
        let Some(task_node) = node_map.get(task_id) else {
            continue;
        };
        let task_info = task_node.data.get("taskInfo");
        let event = task_info
            .and_then(|info| info.get("event"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let from_wp = resolve_waypoint(task_info.and_then(|i| i.get("fromWp")), &route);
        let to_wp = resolve_waypoint(task_info.and_then(|i| i.get("toWp")), &route);

        let mut actions: Vec<Value> = Vec::new();
        let first_ids = edge_map.get(*task_id).cloned().unwrap_or_default();
        for next_id in first_ids {
            add_action(
                &node_map,
                &edge_map,
                next_id,
                Some(&event),
                &route,
                &mut actions,
                &mut error_actions,
                &mut service_types,
                &mut HashSet::new(),
            );
        }
        action_values.extend(actions.iter().cloned());

        tasks.push(json!({
            "taskId": task_id,
            "fromWp": from_wp,
            "toWp": to_wp,
            "event": event,
            "actions": actions,
        }));
    }

    let services = find_services(conn, &service_types).await?;
    apply_service_params(&mut tasks, &services);

    let payload = json!({
        "type": "toros",
        "deviceId": device_code,
        "route": route.iter().map(RoutePoint::to_value).collect::<Vec<_>>(),
        "tasks": tasks,
        "services": services
            .iter()
            .map(|(service_type, params)| json!({ "type": service_type, "params": params }))
            .collect::<Vec<_>>(),
        "onErrorActions": error_actions,
    });

    Ok(Some(payload.to_string()))
}

fn parse_nodes(raw: Option<&str>) -> Vec<FlowNode> {
    raw.and_then(|text| serde_json::from_str::<Vec<FlowNode>>(text).ok())
        .unwrap_or_default()
}

fn parse_edges(raw: Option<&str>) -> Vec<FlowEdge> {
    raw.and_then(|text| serde_json::from_str::<Vec<FlowEdge>>(text).ok())
        .unwrap_or_default()
}

/// 从 start 节点沿边依次收集任务节点（与 mgmt_platform 的 findTasknodes 一致）。
fn collect_task_ids<'a>(
    node_map: &HashMap<&'a str, &'a FlowNode>,
    edge_map: &HashMap<&'a str, Vec<&'a str>>,
    start_id: &'a str,
) -> Vec<&'a str> {
    let mut task_ids = Vec::new();
    let mut visited: HashSet<&str> = HashSet::new();
    let mut current = start_id;

    while visited.insert(current) {
        let Some(next_ids) = edge_map.get(current) else {
            break;
        };
        let mut advanced = false;
        for next_id in next_ids {
            let Some(next_node) = node_map.get(*next_id) else {
                continue;
            };
            match next_node.node_type.as_deref() {
                Some("task") => {
                    task_ids.push(next_node.id.as_str());
                    current = next_node.id.as_str();
                    advanced = true;
                    break;
                }
                Some("end") => {
                    current = next_node.id.as_str();
                    advanced = true;
                    break;
                }
                _ => {}
            }
        }
        if !advanced {
            break;
        }
    }

    task_ids
}

fn node_action(node: &FlowNode) -> Option<Value> {
    node.data.get("action").filter(|a| a.is_object()).cloned()
}

/// 遍历动作链：action 节点串起来，error 节点进入 onErrorActions。
#[allow(clippy::too_many_arguments)]
fn add_action(
    node_map: &HashMap<&str, &FlowNode>,
    edge_map: &HashMap<&str, Vec<&str>>,
    node_id: &str,
    after: Option<&str>,
    route: &[RoutePoint],
    actions: &mut Vec<Value>,
    error_actions: &mut Vec<Value>,
    service_types: &mut HashSet<String>,
    visited: &mut HashSet<String>,
) {
    if !visited.insert(node_id.to_string()) {
        return;
    }
    let Some(node) = node_map.get(node_id) else {
        return;
    };

    let Some(mut action) = node_action(node) else {
        return;
    };
    let action_type = action
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if let Some(action_obj) = action.as_object_mut() {
        action_obj.insert("actionId".to_string(), json!(node_id));
        action_obj.insert("after".to_string(), json!(after.unwrap_or_default()));
        fill_target_wp(action_obj, route);
    }

    if action_type.contains("SERVICE")
        && let Some(service_type) = action
            .pointer("/params/type")
            .and_then(Value::as_str)
            .map(str::to_string)
    {
        service_types.insert(service_type);
    }

    let on_complete = action
        .pointer("/params/event/onComplete")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let on_fail = action
        .pointer("/params/event/onFail")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    actions.push(action);

    let Some(next_ids) = edge_map.get(node_id) else {
        return;
    };
    for next_id in next_ids {
        let Some(next_node) = node_map.get(*next_id) else {
            continue;
        };
        match next_node.node_type.as_deref() {
            Some("action") => add_action(
                node_map,
                edge_map,
                next_node.id.as_str(),
                Some(&on_complete),
                route,
                actions,
                error_actions,
                service_types,
                visited,
            ),
            Some("error") => {
                if let Some(mut error_action) = node_action(next_node) {
                    if let Some(obj) = error_action.as_object_mut() {
                        obj.insert("after".to_string(), json!(on_fail));
                    }
                    error_actions.push(error_action);
                }
            }
            _ => {}
        }
    }
}

/// GOTO 动作里带上航点坐标（与 mgmt_platform 的 fillDronesRouteItem 一致）。
fn fill_target_wp(action: &mut Map<String, Value>, route: &[RoutePoint]) {
    let Some(params) = action.get_mut("params").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(target) = params.get("targetWp").and_then(Value::as_object) else {
        return;
    };
    let wp_id = target.get("wpId").and_then(Value::as_i64);
    let Some(wp_id) = wp_id else {
        return;
    };
    if let Some(point) = route.iter().find(|p| p.wp_id == wp_id) {
        params.insert("targetWp".to_string(), point.to_value());
    }
}

fn resolve_waypoint(info: Option<&Value>, route: &[RoutePoint]) -> Value {
    let wp_id = info
        .and_then(|i| i.get("wpId"))
        .and_then(Value::as_i64)
        .unwrap_or_default();
    match route.iter().find(|p| p.wp_id == wp_id) {
        Some(point) => point.to_value(),
        None => json!({ "wpId": wp_id, "alt": 0.5 }),
    }
}

async fn find_device_code(conn: &DatabaseConnection, device_id: i64) -> Result<String, AppError> {
    let device = DeviceEntity::find()
        .filter(DeviceColumn::Id.eq(device_id))
        .filter(DeviceColumn::IsDeleted.eq(0))
        .one(conn)
        .await
        .map_err(AppError::DatabaseErrorSeaOrm)?;
    Ok(device
        .map(|d| d.device_id)
        .unwrap_or_else(|| device_id.to_string()))
}

/// 读取路线 waypoints。地图世界坐标：x 为东西向（lon），y 为南北向（lat）。
async fn find_route_points(
    conn: &DatabaseConnection,
    route_id: i64,
) -> Result<Vec<RoutePoint>, AppError> {
    let route = RouteEntity::find()
        .filter(RouteColumn::Id.eq(route_id))
        .filter(RouteColumn::IsDeleted.eq(0))
        .one(conn)
        .await
        .map_err(AppError::DatabaseErrorSeaOrm)?;
    let Some(route) = route else {
        return Ok(Vec::new());
    };
    let Some(raw) = route.route_data else {
        return Ok(Vec::new());
    };
    let Ok(points) = serde_json::from_str::<Vec<Value>>(&raw) else {
        return Ok(Vec::new());
    };

    Ok(points
        .into_iter()
        .enumerate()
        .filter_map(|(index, point)| {
            let (x, y) = match &point {
                Value::Array(values) if values.len() >= 2 => {
                    (values[0].as_f64()?, values[1].as_f64()?)
                }
                Value::Object(map) => {
                    let x = map
                        .get("x")
                        .and_then(Value::as_f64)
                        .or_else(|| map.get("lon").and_then(Value::as_f64))?;
                    let y = map
                        .get("y")
                        .and_then(Value::as_f64)
                        .or_else(|| map.get("lat").and_then(Value::as_f64))?;
                    (x, y)
                }
                _ => return None,
            };
            Some(RoutePoint {
                wp_id: index as i64 + 1,
                lat: y,
                lon: x,
                alt: 0.5,
            })
        })
        .collect())
}

/// 读取服务参数：`type` → `params`（JSON）。
async fn find_services(
    conn: &DatabaseConnection,
    service_types: &HashSet<String>,
) -> Result<Vec<(String, Value)>, AppError> {
    if service_types.is_empty() {
        return Ok(Vec::new());
    }
    let services = ServiceEntity::find()
        .filter(ServiceColumn::Type.is_in(service_types.iter().cloned()))
        .filter(ServiceColumn::IsDeleted.eq(0))
        .all(conn)
        .await
        .map_err(AppError::DatabaseErrorSeaOrm)?;

    Ok(services
        .into_iter()
        .filter_map(|service| {
            let service_type = service.r#type?;
            let params = service
                .params
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .unwrap_or_else(|| json!({}));
            Some((service_type, params))
        })
        .collect())
}

/// 与 mgmt_platform 的 setActionService 一致：把服务参数合并进动作参数与事件。
fn apply_service_params(tasks: &mut [Value], services: &[(String, Value)]) {
    if services.is_empty() {
        return;
    }
    let service_map: HashMap<&str, &Value> = services
        .iter()
        .map(|(service_type, params)| (service_type.as_str(), params))
        .collect();

    for task in tasks.iter_mut() {
        let Some(actions) = task.get_mut("actions").and_then(Value::as_array_mut) else {
            continue;
        };
        for action in actions.iter_mut() {
            let Some(action_type) = action.get("type").and_then(Value::as_str) else {
                continue;
            };
            let Some(service_type) = service_type_for_action(action_type) else {
                continue;
            };
            let Some(service_params) = service_map.get(service_type) else {
                continue;
            };
            let Some(params) = action.get_mut("params").and_then(Value::as_object_mut) else {
                continue;
            };
            for key in ["model", "input", "max_age", "url", "bitrate"] {
                if let Some(value) = service_params.get(key) {
                    params.insert(key.to_string(), value.clone());
                }
            }
            if let Some(events) = service_params.get("event").and_then(Value::as_object) {
                let action_events = params
                    .entry("event")
                    .or_insert_with(|| json!({}))
                    .as_object_mut();
                if let Some(action_events) = action_events {
                    for key in ["onStart", "onStop", "onLost", "onTracked"] {
                        if let Some(value) = events.get(key) {
                            action_events.insert(key.to_string(), value.clone());
                        }
                    }
                }
            }
        }
    }
}

fn service_type_for_action(action_type: &str) -> Option<&'static str> {
    match action_type {
        "SERVICE_START_YOLO" | "SERVICE_STOP_YOLO" => Some("YOLO"),
        "SERVICE_START_DEEPSORT" | "SERVICE_STOP_DEEPSORT" => Some("DEEPSORT"),
        "SERVICE_START_RTSP" | "SERVICE_STOP_RTSP" => Some("RTSP"),
        _ => None,
    }
}
