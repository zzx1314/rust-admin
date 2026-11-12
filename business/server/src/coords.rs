//! 路线坐标：画布像素 ↔ 地图坐标系（米）。
//!
//! 智航 Pro 导航任务（协议 §2.5.1.4 `5000 kind=2`）的 `goals[].position` 是
//! **地图坐标系下的米**（`frame_id` 由地图版本冻结），而路线编辑器是在
//! 800×460 的 SVG 画布上点击/拖拽的。两套坐标必须显式换算，否则 335 会被
//! 当成 335 米下发出去（记录在案的事故：整条航线落在图外）。
//!
//! 存储约定：`routes.route_data.points[].x/y` **一律是地图米**，并带
//! `coordFrame: "map"` 标记；没有该标记的是历史画布像素数据，由
//! [`migrate_route_coordinates`] 一次性换算。
//!
//! 换算公式（与 `web/src/utils/coords.ts` 必须保持一致）：
//!
//! ```text
//! scale = min(800 / width, 460 / height)          // 地图底图等比缩放后的比例
//! drawW = width * scale, drawH = height * scale
//! rectX = (800 - drawW) / 2, rectY = (460 - drawH) / 2
//!
//! 画布 → 地图：fx = (cx - rectX) / drawW, fy = (cy - rectY) / drawH
//!              x  = originX + fx * width  * resolution
//!              y  = originY + (1 - fy) * height * resolution   // 图像 y 向下，地图 y 向上
//!
//! 地图 → 画布：fx = (x - originX) / (width * resolution)
//!              fy = 1 - (y - originY) / (height * resolution)
//!              cx = rectX + fx * drawW, cy = rectY + fy * drawH
//! ```
//!
//! 没有真实地图包（演示基线 / 未绑定基线）时退回「示意坐标系」：
//! `resolution = 0.1 m/px`、原点 (0, 0)、画布自身 800×460 px，
//! 于是 `x = cx * 0.1`、`y = (460 - cy) * 0.1`，公式完全可逆，
//! 但这类路线**不能下发**（先在地图基线里发布真实 ROS 地图包）。

use serde_json::{Value, json};
use sqlx::SqlitePool;

/// 编辑画布 viewBox 宽度（px）
pub const CANVAS_WIDTH: f64 = 800.0;
/// 编辑画布 viewBox 高度（px）
pub const CANVAS_HEIGHT: f64 = 460.0;
/// 示意坐标系的虚拟比例尺（m/px）
pub const SCHEMATIC_RESOLUTION: f64 = 0.1;
/// 路线数据里的坐标语义标记：`map` = 地图米制
pub const COORD_FRAME_MAP: &str = "map";
/// 坐标语义标记所在的 JSON 字段
pub const COORD_FRAME_KEY: &str = "coordFrame";
/// 米制路线数据的 schema 版本
pub const SCHEMA_VERSION: i64 = 3;

/// 一张地图版本的四至几何（自 `map_versions` 解析而来）
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapFrame {
    /// 米 / 像素
    pub resolution: f64,
    pub origin_x: f64,
    pub origin_y: f64,
    /// 栅格宽高（px）
    pub width: f64,
    pub height: f64,
    /// 是否为示意坐标系（没有真实地图包）
    pub schematic: bool,
}

impl MapFrame {
    /// 示意坐标系：画布自身就是地图，0.1 m/px，原点在左下角
    pub const fn schematic() -> Self {
        Self {
            resolution: SCHEMATIC_RESOLUTION,
            origin_x: 0.0,
            origin_y: 0.0,
            width: CANVAS_WIDTH,
            height: CANVAS_HEIGHT,
            schematic: true,
        }
    }

    /// 从 `map_versions` 的字段组装；缺关键信息（没有 ROS 地图包 / 分辨率 / 原点）时退回示意坐标系
    pub fn from_parts(
        resolution: Option<&str>,
        origin: Option<&str>,
        width: Option<i64>,
        height: Option<i64>,
        file_path: Option<&str>,
    ) -> Self {
        let has_package = file_path.is_some_and(|value| !value.trim().is_empty());
        let resolution = resolution.and_then(parse_resolution);
        let origin = origin.and_then(parse_origin);
        match (has_package, resolution, origin, width, height) {
            (true, Some(resolution), Some((origin_x, origin_y)), Some(width), Some(height))
                if width > 0 && height > 0 =>
            {
                Self {
                    resolution,
                    origin_x,
                    origin_y,
                    width: width as f64,
                    height: height as f64,
                    schematic: false,
                }
            }
            _ => Self::schematic(),
        }
    }

    /// 底图在画布里的铺放几何：`(scale, drawW, drawH, rectX, rectY)`
    fn layout(&self) -> (f64, f64, f64, f64, f64) {
        let scale = (CANVAS_WIDTH / self.width).min(CANVAS_HEIGHT / self.height);
        let draw_w = self.width * scale;
        let draw_h = self.height * scale;
        (scale, draw_w, draw_h, (CANVAS_WIDTH - draw_w) / 2.0, (CANVAS_HEIGHT - draw_h) / 2.0)
    }

    /// 画布像素 → 地图米
    pub fn canvas_to_map(&self, canvas_x: f64, canvas_y: f64) -> (f64, f64) {
        let (_, draw_w, draw_h, rect_x, rect_y) = self.layout();
        let fx = (canvas_x - rect_x) / draw_w;
        let fy = (canvas_y - rect_y) / draw_h;
        (
            self.origin_x + fx * self.width * self.resolution,
            self.origin_y + (1.0 - fy) * self.height * self.resolution,
        )
    }

    /// 地图米 → 画布像素
    pub fn map_to_canvas(&self, x: f64, y: f64) -> (f64, f64) {
        let (_, draw_w, draw_h, rect_x, rect_y) = self.layout();
        let fx = (x - self.origin_x) / (self.width * self.resolution);
        let fy = 1.0 - (y - self.origin_y) / (self.height * self.resolution);
        (rect_x + fx * draw_w, rect_y + fy * draw_h)
    }

    /// 地图覆盖范围（米）：`(min_x, min_y, max_x, max_y)`
    pub fn extents(&self) -> (f64, f64, f64, f64) {
        (
            self.origin_x,
            self.origin_y,
            self.origin_x + self.width * self.resolution,
            self.origin_y + self.height * self.resolution,
        )
    }

    /// 点是否落在地图覆盖范围内（留 1 个像素的容差）
    pub fn contains(&self, x: f64, y: f64) -> bool {
        let (min_x, min_y, max_x, max_y) = self.extents();
        let margin = self.resolution;
        x >= min_x - margin
            && x <= max_x + margin
            && y >= min_y - margin
            && y <= max_y + margin
    }

    /// 冻结进路线数据的地图几何快照（便于离线核对与前端渲染）
    pub fn snapshot(&self) -> Value {
        json!({
            "resolution": self.resolution,
            "origin": [self.origin_x, self.origin_y],
            "width": self.width,
            "height": self.height,
            "schematic": self.schematic,
        })
    }
}

/// 解析 `"0.05 m/px"` / `"0.05"` 形式的分辨率
pub fn parse_resolution(text: &str) -> Option<f64> {
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

/// 解析 `"-14.85, -13.45, 0.0"` 形式的原点（只取 x/y，米）
pub fn parse_origin(text: &str) -> Option<(f64, f64)> {
    let mut parts = text.split(',').map(|part| part.trim().parse::<f64>().ok());
    let x = parts.next()??;
    let y = parts.next()??;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    Some((x, y))
}

/// 取某张地图版本的坐标几何；地图不存在 / 没有 ROS 地图包时退回示意坐标系
pub async fn frame_for_map(
    pool: &SqlitePool,
    map_id: Option<&str>,
) -> Result<MapFrame, sqlx::Error> {
    let Some(map_id) = map_id.filter(|value| !value.trim().is_empty()) else {
        return Ok(MapFrame::schematic());
    };
    let row: Option<MapRow> = sqlx::query_as(
            "SELECT resolution, origin, map_width, map_height, file_path
             FROM map_versions WHERE id = ?1",
        )
        .bind(map_id)
        .fetch_optional(pool)
        .await?;
    Ok(row
        .map(|(resolution, origin, width, height, file_path)| {
            MapFrame::from_parts(
                resolution.as_deref(),
                origin.as_deref(),
                width,
                height,
                file_path.as_deref(),
            )
        })
        .unwrap_or_else(MapFrame::schematic))
}

/// 路线数据是否已经是地图米制（带 `coordFrame: "map"` 标记）
pub fn is_map_frame(data: &Value) -> bool {
    data.get(COORD_FRAME_KEY).and_then(Value::as_str) == Some(COORD_FRAME_MAP)
}

/// 把一条路线的 `points[].x/y` 从画布像素换算成地图米，并打上米制标记。
///
/// 只处理 `points`：`legs` / `returnLeg` 里只有速度与动作，不含坐标。
pub fn apply_canvas_to_map(data: &mut Value, frame: &MapFrame) {
    if let Some(points) = data.get_mut("points").and_then(Value::as_array_mut) {
        for point in points {
            let Some(object) = point.as_object_mut() else {
                continue;
            };
            let canvas_x = object.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let canvas_y = object.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let (x, y) = frame.canvas_to_map(canvas_x, canvas_y);
            object.insert("x".to_string(), json!(round3(x)));
            object.insert("y".to_string(), json!(round3(y)));
        }
    }
    if let Some(object) = data.as_object_mut() {
        object.insert(COORD_FRAME_KEY.to_string(), json!(COORD_FRAME_MAP));
        object.insert("schemaVersion".to_string(), json!(SCHEMA_VERSION));
        object.insert("mapFrame".to_string(), frame.snapshot());
    }
}

/// 落在坐标系可用范围之外的航点：`(序号从 1 开始, x, y)`
pub fn points_outside(data: &Value, frame: &MapFrame) -> Vec<(usize, f64, f64)> {
    data.get("points")
        .and_then(Value::as_array)
        .map(|points| {
            points
                .iter()
                .enumerate()
                .filter_map(|(index, point)| {
                    let x = point.get("x").and_then(Value::as_f64)?;
                    let y = point.get("y").and_then(Value::as_f64)?;
                    (!frame.contains(x, y)).then_some((index + 1, x, y))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `map_versions` 里与坐标换算相关的列
type MapRow = (Option<String>, Option<String>, Option<i64>, Option<i64>, Option<String>);
/// `routes` 里与坐标迁移相关的列
type RouteRow = (i64, Option<String>, Option<String>, String, f64);

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// 历史路线一次性迁移：画布像素 → 地图米。
///
/// 幂等：`route_data` 已带 `coordFrame: "map"` 的路线直接跳过，可以随进程启动反复执行。
/// 迁移后同步重算 `point_count / distance_m / duration_s / battery_pct`
/// （旧口径是「像素 × 1.2 当米」，与地图分辨率无关）。
pub async fn migrate_route_coordinates(pool: &SqlitePool) -> Result<usize, sqlx::Error> {
    let rows: Vec<RouteRow> = sqlx::query_as(
        "SELECT id, map_version_id, route_data, loop_mode, default_speed FROM routes",
    )
    .fetch_all(pool)
    .await?;

    let mut converted = 0_usize;
    for (id, map_id, raw, loop_mode, default_speed) in rows {
        let Some(raw) = raw else { continue };
        let Ok(mut data) = serde_json::from_str::<Value>(&raw) else {
            tracing::warn!("路线 {id} 的 route_data 不是合法 JSON，跳过坐标迁移");
            continue;
        };
        if is_map_frame(&data) {
            continue;
        }
        let frame = frame_for_map(pool, map_id.as_deref()).await?;
        apply_canvas_to_map(&mut data, &frame);
        let metrics = crate::geo::route_metrics(&data, default_speed);
        sqlx::query(
            "UPDATE routes SET route_data = ?1, point_count = ?2, distance_m = ?3,
                duration_s = ?4, battery_pct = ?5, updated_at = ?6 WHERE id = ?7",
        )
        .bind(data.to_string())
        .bind(metrics.point_count)
        .bind(metrics.distance_m)
        .bind(metrics.duration_s)
        .bind(metrics.battery_pct)
        .bind(crate::db::now())
        .bind(id)
        .execute(pool)
        .await?;
        converted += 1;
        tracing::info!(
            route = id,
            schematic = frame.schematic,
            loop_mode = %loop_mode,
            "路线坐标已从画布像素换算为地图米"
        );
    }
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_frame() -> MapFrame {
        // 现场地图 ROS-MAP-1794446794789@v1.0：resolution 0.05、origin(-14.85,-13.45)、1088×940
        MapFrame::from_parts(
            Some("0.05 m/px"),
            Some("-14.85, -13.45, 0.0"),
            Some(1088),
            Some(940),
            Some("maps/1794446794779-2026_11_10_16_15_06.zip"),
        )
    }

    #[test]
    fn fake_baseline_without_package_falls_back_to_schematic() {
        // 演示基线：origin 是经纬度锚点、没有 file_path
        let frame = MapFrame::from_parts(
            Some("0.05 m/px"),
            Some("31.2014, 121.6023"),
            Some(1600),
            Some(860),
            None,
        );
        assert!(frame.schematic);
        assert_eq!(frame, MapFrame::schematic());
    }

    #[test]
    fn canvas_point_maps_to_expected_meters() {
        let frame = real_frame();
        assert!(!frame.schematic);
        let (x, y) = frame.canvas_to_map(335.0, 120.0);
        assert!((x - 5.71).abs() < 0.02, "x={x}");
        assert!((y - 21.29).abs() < 0.02, "y={y}");
        assert!(frame.contains(x, y));
    }

    #[test]
    fn canvas_roundtrip_is_stable() {
        let frame = real_frame();
        for (cx, cy) in [(135.0, 0.0), (400.0, 230.0), (666.0, 460.0)] {
            let (x, y) = frame.canvas_to_map(cx, cy);
            let (back_x, back_y) = frame.map_to_canvas(x, y);
            assert!((back_x - cx).abs() < 1e-6, "cx {cx} -> {back_x}");
            assert!((back_y - cy).abs() < 1e-6, "cy {cy} -> {back_y}");
        }
    }

    #[test]
    fn schematic_frame_is_identity_with_y_flip() {
        let frame = MapFrame::schematic();
        let (x, y) = frame.canvas_to_map(320.0, 335.0);
        assert!((x - 32.0).abs() < 1e-9);
        assert!((y - 12.5).abs() < 1e-9);
    }

    #[test]
    fn out_of_bounds_points_are_reported() {
        let frame = real_frame();
        let data = json!({
            "points": [
                {"name": "在图上", "x": 5.7, "y": 21.3},
                {"name": "画布像素", "x": 335, "y": 120}
            ]
        });
        let outside = points_outside(&data, &frame);
        assert_eq!(outside.len(), 1);
        assert_eq!(outside[0].0, 2);
    }

    #[test]
    fn apply_marks_coord_frame_and_rounds() {
        let mut data = json!({
            "schemaVersion": 2,
            "loop": "闭环",
            "points": [{"name": "P1", "x": 335, "y": 120}],
            "legs": [],
            "returnLeg": null
        });
        apply_canvas_to_map(&mut data, &real_frame());
        assert_eq!(data["coordFrame"], "map");
        assert_eq!(data["schemaVersion"], 3);
        assert!(data["mapFrame"]["resolution"].as_f64().unwrap() > 0.0);
        assert!(is_map_frame(&data));
    }

    #[test]
    fn resolution_and_origin_parsing() {
        assert_eq!(parse_resolution("0.05 m/px"), Some(0.05));
        assert_eq!(parse_resolution("0.05"), Some(0.05));
        assert_eq!(parse_resolution("—"), None);
        assert_eq!(parse_origin("-14.85, -13.45, 0.0"), Some((-14.85, -13.45)));
        assert_eq!(parse_origin("31.2014, 121.6023"), Some((31.2014, 121.6023)));
        assert_eq!(parse_origin("—"), None);
    }
}
