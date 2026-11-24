//! 航点净空（离最近障碍的距离）：判断「最后一个航点能不能当停车位」。
//!
//! 为什么需要它：`endTaskMode = none`（原地待命）时，**最后一个航点就是停车位姿**——
//! 本体 `5000 kind=4` 的 `end_task = 0` 就是「停在最后一点」。而本体规划器要求目标点
//! 在 free 栅格上、离障碍留出膨胀余量，现场实测净空 < 0.5 m 就会反复报
//! `13377 无有效绕障轨迹` / `13446 全局规划失败`（2026-11-14 复盘，判读见
//! `deploy/robodog-body.md` §10.8）：任务于是永远到不了终态，平台一直显示「执行中」。
//!
//! 这里直接用地图纸里的 `map.pgm` 算**欧氏距离场**（Felzenszwalb 精确 EDT），给路线
//! 校验 / 发布判定「这个点离最近障碍多远」。坐标口径与 [`crate::coords::MapFrame`]
//! 一致（米制、原点取 `map.yaml`、图像 y 向下）。
//!
//! **只认占据格（灰度 < 100）**：未知区域（128）不算障碍——现场地图动辄 8 成是
//! unknown，把未知算障碍会让每个航点都报净空不足，校验就没人看了。

// 与同模块其它文件一致：词汇表（`AppState` / 错误类型）由 `super::*` 带入
#![allow(clippy::wildcard_imports)]

use super::raster::{RosRaster, read_ros_raster};
use super::*;
use crate::coords::MapFrame;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

/// 「原地待命」停车位的建议净空（米）。
///
/// 与 `deploy/robodog-body.md` §10.8 的现场实测阈值一致：0.50 m 的航点本体可达，
/// 再近就开始吃 `13377`。
pub(crate) const MIN_STOP_CLEARANCE_M: f64 = 0.5;

/// 硬门槛（米）：低于它说明点基本落在障碍格 / 膨胀层里，**任何**结束动作都规划不过去。
pub(crate) const MIN_FREE_CLEARANCE_M: f64 = 0.15;

/// 距离场的上限（米）：整张图一个障碍都没有时（全 unknown / 全 free）距离是无穷，
/// 掐到 60 m 免得消息里出现 `inf`。
const MAX_CLEARANCE_M: f64 = 60.0;

/// 一张地图的净空距离场
pub(crate) struct ClearanceField {
    resolution: f64,
    origin_x: f64,
    origin_y: f64,
    width: usize,
    height: usize,
    /// 每个栅格到最近障碍的距离（米），行 0 = 图像顶部（与 PGM 存法一致）
    dist_m: Vec<f32>,
}

impl ClearanceField {
    /// 地图米制坐标 → 栅格下标（图像行 0 在顶部，所以要翻 y）
    fn cell(&self, x: f64, y: f64) -> Option<usize> {
        let col = ((x - self.origin_x) / self.resolution).round();
        let row_from_bottom = ((y - self.origin_y) / self.resolution).round();
        if !col.is_finite() || !row_from_bottom.is_finite() || col < 0.0 || row_from_bottom < 0.0 {
            return None;
        }
        let (col, row_from_bottom) = (col as usize, row_from_bottom as usize);
        if col >= self.width || row_from_bottom >= self.height {
            return None;
        }
        Some((self.height - 1 - row_from_bottom) * self.width + col)
    }

    /// 该点离最近障碍的距离（米）。图外返回 `None`（图外的点由
    /// [`crate::coords::points_outside`] 单独拦，不要在这里重复报）。
    pub(crate) fn clearance_m(&self, x: f64, y: f64) -> Option<f64> {
        self.cell(x, y).map(|index| f64::from(self.dist_m[index]))
    }
}

/// `route_data.points` 里一个点的净空
#[derive(Debug, Clone)]
pub(crate) struct PointClearance {
    /// 航点序号（从 1 开始，与路线校验的其它文案一致）
    pub index: usize,
    pub x: f64,
    pub y: f64,
    pub clearance_m: f64,
}

/// 逐点算净空；坐标缺失 / 落在图外的点跳过（它们已经有别的校验项在管）
pub(crate) fn point_clearances(field: &ClearanceField, data: &Value) -> Vec<PointClearance> {
    data.get("points")
        .and_then(Value::as_array)
        .map(|points| {
            points
                .iter()
                .enumerate()
                .filter_map(|(index, point)| {
                    let x = point.get("x").and_then(Value::as_f64)?;
                    let y = point.get("y").and_then(Value::as_f64)?;
                    let clearance_m = field.clearance_m(x, y)?;
                    Some(PointClearance {
                        index: index + 1,
                        x,
                        y,
                        clearance_m,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 净空距离场缓存：`map_id|file_path → ClearanceField`
///
/// 校验 / 发布一次只用一次，但 UI 上「校验路线」是连点的，一张 2000×2000 的图
/// 解码 + EDT 要几十毫秒，缓存比每次重算划算（地图包变了 `file_path` 会变，
/// key 里带上它就不会拿到旧图）。
static CACHE: OnceLock<StdMutex<HashMap<String, Arc<ClearanceField>>>> = OnceLock::new();

fn cache() -> &'static StdMutex<HashMap<String, Arc<ClearanceField>>> {
    CACHE.get_or_init(|| StdMutex::new(HashMap::new()))
}

/// `map_versions` 里与净空计算相关的列：`(file_path, resolution, origin, map_width, map_height)`
type MapRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

/// 取某张地图版本的净空距离场；没有真实地图包 / 图与几何对不上时返回 `None`
/// （调用方据此**跳过**净空相关校验项，而不是判失败）。
pub(crate) async fn field_for_map(
    db: &SqlitePool,
    backend_db: &std::path::Path,
    map_id: Option<&str>,
) -> Option<Arc<ClearanceField>> {
    let map_id = map_id.map(str::trim).filter(|value| !value.is_empty())?;
    let row: Option<MapRow> = sqlx::query_as(
        "SELECT file_path, resolution, origin, map_width, map_height
               FROM map_versions WHERE id = ?1",
    )
    .bind(map_id)
    .fetch_optional(db)
    .await
    .ok()?;
    let (file_path, resolution, origin, width, height) = row?;
    let file_path = file_path.filter(|value| !value.trim().is_empty())?;
    let frame = MapFrame::from_parts(
        resolution.as_deref(),
        origin.as_deref(),
        width,
        height,
        Some(file_path.as_str()),
    );
    if frame.schematic {
        return None;
    }
    let key = format!("{map_id}|{file_path}");
    if let Ok(guard) = cache().lock()
        && let Some(field) = guard.get(&key)
    {
        return Some(field.clone());
    }
    let raster = read_ros_raster(backend_db, &file_path)?;
    let field = Arc::new(field_from_raster(&raster, &frame)?);
    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, field.clone());
    }
    Some(field)
}

/// 栅格 + 地图几何 → 距离场。
///
/// 栅格尺寸与 `map_versions` 记住的 `map_width/map_height` 允许差 1 个格（yaml 取整），
/// 差得更多说明图与几何不是同一份，宁可不用（`None`）也不能给出错的净空。
fn field_from_raster(raster: &RosRaster, frame: &MapFrame) -> Option<ClearanceField> {
    let (width, height) = (raster.width as usize, raster.height as usize);
    if width == 0 || height == 0 || width.checked_mul(height).is_none() {
        return None;
    }
    let expected = (frame.width as usize, frame.height as usize);
    if width.abs_diff(expected.0) > 1 || height.abs_diff(expected.1) > 1 {
        tracing::warn!(
            "地图栅格 {}x{} 与地图几何 {}x{} 对不上，跳过净空计算",
            width,
            height,
            expected.0,
            expected.1
        );
        return None;
    }
    let occupied: Vec<bool> = raster.pixels.iter().map(|value| *value < 100).collect();
    if occupied.iter().all(|value| !*value) {
        // 一个占据格都没有（全 unknown / 新建图）：算不出净空，当成「没有信息」
        return None;
    }
    let dist_sq = squared_edt(&occupied, width, height);
    let resolution = frame.resolution;
    let max_sq = (MAX_CLEARANCE_M / resolution).powi(2);
    let dist_m = dist_sq
        .into_iter()
        .map(|value| value.min(max_sq).sqrt() as f32 * resolution as f32)
        .collect();
    Some(ClearanceField {
        resolution,
        origin_x: frame.origin_x,
        origin_y: frame.origin_y,
        width,
        height,
        dist_m,
    })
}

/// 精确欧氏距离变换（Felzenszwalb & Huttenlocher 2012）：返回每格到最近 `true` 格的
/// **平方**距离（单位：格）。两趟一维变换（先列后行），与 §10.8 里那把多源 BFS
/// 等价，但给的是欧氏距离——BFS 的曼哈顿距离会把斜向净空高估 41%，正好是
/// 「消息说 0.7 m、本体还是进不去」的那种误判方向。
fn squared_edt(mask: &[bool], width: usize, height: usize) -> Vec<f64> {
    const INF: f64 = 1e20;
    let mut grid: Vec<f64> = mask
        .iter()
        .map(|occupied| if *occupied { 0.0 } else { INF })
        .collect();
    let longest = width.max(height);
    let mut f = vec![0.0_f64; longest];
    let mut d = vec![0.0_f64; longest];
    let mut v = vec![0_usize; longest];
    let mut z = vec![0.0_f64; longest + 1];

    // 列方向
    for col in 0..width {
        for row in 0..height {
            f[row] = grid[row * width + col];
        }
        edt_1d(&f[..height], height, &mut d[..height], &mut v, &mut z);
        for row in 0..height {
            grid[row * width + col] = d[row];
        }
    }
    // 行方向
    for row in 0..height {
        let base = row * width;
        f[..width].copy_from_slice(&grid[base..base + width]);
        edt_1d(&f[..width], width, &mut d[..width], &mut v, &mut z);
        grid[base..base + width].copy_from_slice(&d[..width]);
    }
    grid
}

/// 一维平方距离变换：`f` 是上一步的平方距离，`d` 是结果
fn edt_1d(f: &[f64], n: usize, d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    const INF: f64 = 1e20;
    if n == 0 {
        return;
    }
    let mut k = 0_usize;
    v[0] = 0;
    z[0] = -INF;
    z[1] = INF;
    for q in 1..n {
        // 抛物线 (q, f[q]) 与当前最右抛物线 (v[k], f[v[k]]) 的交点
        let mut s = intersection(f, q, v[k]);
        while s <= z[k] {
            k -= 1;
            s = intersection(f, q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = INF;
    }
    k = 0;
    // 三个数组（`d` / `v` / `z`）按同一个下标走，用迭代器反而看不出算法在干什么
    #[allow(clippy::needless_range_loop)]
    for q in 0..n {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let dq = q as f64 - v[k] as f64;
        d[q] = dq * dq + f[v[k]];
    }
}

fn intersection(f: &[f64], q: usize, previous: usize) -> f64 {
    let qf = q as f64;
    let pf = previous as f64;
    ((f[q] + qf * qf) - (f[previous] + pf * pf)) / (2.0 * qf - 2.0 * pf)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RES: f64 = 0.05;

    /// 构造一张 `width×height` 的栅格，`occupied` 给出占据格的 (col, row)（row 从图像顶部数）
    fn raster(width: u32, height: u32, occupied: &[(u32, u32)]) -> RosRaster {
        let mut pixels = vec![254_u8; (width * height) as usize];
        for (col, row) in occupied {
            pixels[(row * width + col) as usize] = 0;
        }
        RosRaster {
            width,
            height,
            pixels,
        }
    }

    fn frame(width: u32, height: u32) -> MapFrame {
        MapFrame {
            resolution: RES,
            origin_x: -1.0,
            origin_y: -2.0,
            width: f64::from(width),
            height: f64::from(height),
            schematic: false,
        }
    }

    /// 图像坐标（col, row 从顶部数）→ 地图米制。PGM 的 row 0 在地图上方，
    /// 所以地图 y 要翻过来：`y = origin_y + (height-1-row) * res`。
    fn map_xy(col: usize, row_from_top: usize, height: usize) -> (f64, f64) {
        (
            -1.0 + col as f64 * RES,
            -2.0 + (height - 1 - row_from_top) as f64 * RES,
        )
    }

    #[test]
    fn clearance_is_zero_on_obstacle_and_grows_with_distance() {
        let field = field_from_raster(&raster(20, 10, &[(5, 5)]), &frame(20, 10)).expect("field");
        let (x, y) = map_xy(5, 5, 10);
        assert!(
            field.clearance_m(x, y).unwrap() < 1e-6,
            "障碍格自身净空为 0"
        );
        assert!(
            (field.clearance_m(x + RES, y).unwrap() - RES).abs() < 1e-6,
            "邻格净空应为一个格距"
        );
        assert!((field.clearance_m(x + 3.0 * RES, y).unwrap() - 3.0 * RES).abs() < 1e-6);
    }

    #[test]
    fn diagonal_distance_is_euclidean_not_manhattan() {
        let field = field_from_raster(&raster(20, 20, &[(0, 0)]), &frame(20, 20)).expect("field");
        // 障碍在图像左上角；往右 3 格、往下 4 格：欧氏 5 格 = 0.25 m，
        // 曼哈顿会给 7 格 = 0.35 m（高估 41%，正是「消息说 0.7 m、本体还是进不去」的方向）
        let (x, y) = map_xy(3, 4, 20);
        let clearance = field.clearance_m(x, y).unwrap();
        assert!(
            (clearance - 5.0 * RES).abs() < 1e-6,
            "斜向净空必须按欧氏距离算，实际 {clearance}"
        );
    }

    #[test]
    fn unknown_cells_are_not_obstacles() {
        // 全 unknown（128）：没有占据格 → 判不出净空，返回 None 让调用方跳过校验项
        let mut pixels = vec![128_u8; 100];
        pixels[0] = 128;
        let raster = RosRaster {
            width: 10,
            height: 10,
            pixels,
        };
        assert!(field_from_raster(&raster, &frame(10, 10)).is_none());
    }

    #[test]
    fn mismatched_raster_and_frame_is_rejected() {
        // 图与 map.yaml 不是同一份：宁可不给净空，也不能给错的
        assert!(field_from_raster(&raster(20, 10, &[(1, 1)]), &frame(40, 10)).is_none());
    }

    #[test]
    fn out_of_bounds_points_have_no_clearance() {
        let field = field_from_raster(&raster(10, 10, &[(0, 0)]), &frame(10, 10)).expect("field");
        assert!(field.clearance_m(-99.0, 0.0).is_none());
        assert!(field.clearance_m(-1.0, 99.0).is_none());
    }

    #[test]
    fn route_points_get_one_based_index_and_clearance() {
        let field = field_from_raster(&raster(20, 20, &[(0, 0)]), &frame(20, 20)).expect("field");
        let (far_x, far_y) = map_xy(10, 10, 20);
        let (near_x, near_y) = map_xy(1, 0, 20);
        let data = json!({
            "points": [
                {"x": far_x, "y": far_y},
                {"x": near_x, "y": near_y},
            ]
        });
        let clearances = point_clearances(&field, &data);
        assert_eq!(clearances.len(), 2);
        assert_eq!(clearances[0].index, 1);
        assert_eq!(clearances[1].index, 2);
        assert!(
            clearances[1].clearance_m < clearances[0].clearance_m,
            "贴着障碍的点净空必须更小"
        );
    }
}
