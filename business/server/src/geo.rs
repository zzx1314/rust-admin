use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Default, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteMetrics {
    pub point_count: i64,
    pub distance_m: f64,
    pub duration_s: i64,
    pub action_count: i64,
    pub battery_pct: i64,
    pub dwell_s: i64,
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

pub fn route_metrics(data: &Value, default_speed: f64) -> RouteMetrics {
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
    let loop_mode = data.get("loop").and_then(Value::as_str).unwrap_or("闭环");

    let mut pixels = 0.0_f64;
    for pair in points.windows(2) {
        pixels += (number(&pair[1], "x") - number(&pair[0], "x"))
            .hypot(number(&pair[1], "y") - number(&pair[0], "y"));
    }
    if loop_mode == "闭环" && points.len() > 2 {
        let first = &points[0];
        let last = &points[points.len() - 1];
        pixels +=
            (number(last, "x") - number(first, "x")).hypot(number(last, "y") - number(first, "y"));
    }

    let meters = pixels * 1.2;
    let dwell_s: i64 = points
        .iter()
        .map(|point| number(point, "dwell") as i64)
        .sum();
    let mut action_count: i64 = points
        .iter()
        .map(|point| {
            point
                .get("actions")
                .and_then(Value::as_array)
                .map_or(0, |actions| actions.len() as i64)
        })
        .sum();
    action_count += legs
        .iter()
        .map(|leg| {
            leg.get("actions")
                .and_then(Value::as_array)
                .map_or(0, |actions| actions.len() as i64)
        })
        .sum::<i64>();
    if let Some(actions) = data
        .get("returnLeg")
        .and_then(|leg| leg.get("actions"))
        .and_then(Value::as_array)
    {
        action_count += actions.len() as i64;
    }

    let speed = if default_speed > 0.1 {
        default_speed
    } else {
        1.2
    };
    let move_s = (meters / speed).ceil() as i64;
    let duration_s = move_s + dwell_s + action_count * 3;
    let battery_pct = ((meters / 1000.0) * 8.0 + action_count as f64 * 0.4)
        .round()
        .clamp(0.0, 100.0) as i64;

    RouteMetrics {
        point_count: points.len() as i64,
        distance_m: (meters * 10.0).round() / 10.0,
        duration_s,
        action_count,
        battery_pct,
        dwell_s,
    }
}

pub fn nearest_neighbor_order(data: &Value) -> Option<Value> {
    let mut clone = data.clone();
    let points = clone.get_mut("points")?.as_array_mut()?;
    if points.len() < 3 {
        return None;
    }

    let mut remaining: Vec<Value> = points[1..].to_vec();
    let mut ordered: Vec<Value> = vec![points[0].clone()];
    while !remaining.is_empty() {
        let current = ordered.last().expect("ordered is not empty");
        let mut best_index = 0;
        let mut best_distance = f64::MAX;
        for (index, point) in remaining.iter().enumerate() {
            let distance = (number(point, "x") - number(current, "x"))
                .hypot(number(point, "y") - number(current, "y"));
            if distance < best_distance {
                best_distance = distance;
                best_index = index;
            }
        }
        ordered.push(remaining.remove(best_index));
    }

    *points = ordered;
    Some(clone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_route_has_zero_metrics() {
        let data = json!({
            "loop": "闭环",
            "points": [],
            "legs": [],
            "returnLeg": null
        });
        let metrics = route_metrics(&data, 1.2);
        assert_eq!(metrics.point_count, 0);
        assert_eq!(metrics.distance_m, 0.0);
        assert_eq!(metrics.duration_s, 0);
        assert_eq!(metrics.action_count, 0);
        assert_eq!(metrics.battery_pct, 0);
    }

    #[test]
    fn computes_distance_duration_and_actions() {
        let data = json!({
            "loop": "往返",
            "points": [
                {"x": 0, "y": 0, "dwell": 10, "actions": [{"type": "拍照"}]},
                {"x": 100, "y": 0, "dwell": 5, "actions": []}
            ],
            "legs": [
                {"actions": [{"type": "录像"}, {"type": "测温"}]}
            ],
            "returnLeg": null
        });
        let metrics = route_metrics(&data, 1.2);
        assert_eq!(metrics.point_count, 2);
        assert_eq!(metrics.distance_m, 120.0);
        assert_eq!(metrics.dwell_s, 15);
        assert_eq!(metrics.action_count, 3);
        assert_eq!(metrics.duration_s, 100 + 15 + 9);
        assert_eq!(metrics.battery_pct, 2);
    }

    #[test]
    fn closed_loop_includes_return_distance() {
        let data = json!({
            "loop": "闭环",
            "points": [
                {"x": 0, "y": 0},
                {"x": 100, "y": 0},
                {"x": 100, "y": 100}
            ],
            "legs": [],
            "returnLeg": null
        });
        let metrics = route_metrics(&data, 1.0);
        assert!((metrics.distance_m - 409.7).abs() < 0.2);
    }

    #[test]
    fn nearest_neighbor_reorders_points() {
        let data = json!({
            "points": [
                {"x": 0, "y": 0},
                {"x": 100, "y": 0},
                {"x": 10, "y": 0},
                {"x": 200, "y": 0}
            ]
        });
        let optimized = nearest_neighbor_order(&data).expect("order should be produced");
        let xs: Vec<f64> = optimized["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|point| point["x"].as_f64().unwrap())
            .collect();
        assert_eq!(xs, vec![0.0, 10.0, 100.0, 200.0]);
    }

    #[test]
    fn nearest_neighbor_keeps_short_routes_unchanged() {
        let data = json!({"points": [{"x": 0, "y": 0}, {"x": 1, "y": 1}]});
        assert!(nearest_neighbor_order(&data).is_none());
    }
}
