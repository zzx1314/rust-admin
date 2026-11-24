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

/// 航段里允许的最长连续「占据」长度（米）。超过就拒绝发布。
///
/// 墙线一般 1~2 像素（0.05~0.10 m），直连航点偶尔切到墙角是正常的（本体自己会绕），
/// 但连续 0.5 m 的占据就意味着这条线是从障碍物里穿过去的。
pub(crate) const MAX_LEG_OCCUPIED_RUN_M: f64 = 0.5;

/// 航段里允许的最长连续「未知/不可通行」长度（米）。超过就拒绝发布。
///
/// 2026-11-24 广汇二期 `TK-1124-17`：RT-011 最后一段是 55 m 直线，其中 39.8 m
/// 连续落在平台标的「未知」区（205），本体把这值当可通行，狗直接开了进去，最后
/// 报 `13377 无有效绕障轨迹`。墙边 / 路口零星几格未知不该拦（现场地图动辄大半是
/// unknown），但一整段穿过去一定是「没沿着可走区域画」。
pub(crate) const MAX_LEG_UNKNOWN_RUN_M: f64 = 5.0;

/// 航段允许走出地图范围的长度（米）。只容忍边界取整的零星采样点；
/// 真正的「飞出图外」远大于这个值（端点在图外有 `coords::points_outside` 单独拦）。
pub(crate) const MAX_LEG_OUTSIDE_M: f64 = 0.5;

/// 栅格灰度的通行性（口径与本体 `map.yaml` 的 trinary 判定对齐）：
///
/// | 灰度 | 含义 | 本体 |
/// |---|---|---|
/// | `0..=99` | 占据 | 规划不过去 |
/// | `100..=205` | 未知 / 不可通行 | 128 = ROS 的未知；205 = 平台「未知」画笔 |
/// | `206..=255` | 可通行 | 自由空间 |
///
/// 本体按 `p = (255 - v) / 255`（`free_thresh: 0.2`）判定，**`v > 204` 才算 free**；
/// 205 刚好在自由一侧，所以平台用 205 画的「未知/禁行」本体并不会避开。
/// 这里把 205 也算成未知，是为了让平台自己把这种「本该禁行」的航段报出来，
/// 而不是当成空地放行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CellKind {
    Occupied,
    Unknown,
    Free,
}

pub(crate) fn classify(value: u8) -> CellKind {
    match value {
        0..=99 => CellKind::Occupied,
        100..=205 => CellKind::Unknown,
        _ => CellKind::Free,
    }
}

/// 一张地图的净空距离场
pub(crate) struct ClearanceField {
    resolution: f64,
    origin_x: f64,
    origin_y: f64,
    width: usize,
    height: usize,
    /// 每个栅格到最近障碍的距离（米），行 0 = 图像顶部（与 PGM 存法一致）
    dist_m: Vec<f32>,
    /// 原始灰度，行 0 = 图像顶部；航段校验要看占据 / 未知 / 自由（与 `dist_m` 同序）
    values: Vec<u8>,
    /// 整张图一个占据格都没有（全 free / 全 unknown）：净空没有意义，但航段仍要看未知
    has_obstacles: bool,
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
    /// [`crate::coords::points_outside`] 单独拦，不要在这里重复报）；
    /// 整张图没有占据格时也返回 `None`（没有净空可言）。
    pub(crate) fn clearance_m(&self, x: f64, y: f64) -> Option<f64> {
        if !self.has_obstacles {
            return None;
        }
        self.cell(x, y).map(|index| f64::from(self.dist_m[index]))
    }

    /// 该点的栅格灰度（图外返回 `None`）
    fn value_at(&self, x: f64, y: f64) -> Option<u8> {
        self.cell(x, y).map(|index| self.values[index])
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
                    let (x, y) = point_xy(point)?;
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

/// 平台「未知」画笔的旧值（205）：本体按 `free_thresh: 0.2` 会把它当成可通行，
/// 单独统计一段长度，文案里好告诉运维「这片禁行是 205 画的，本体没拦住」。
const LEGACY_UNKNOWN_PAINT: u8 = 205;

/// 一个航段穿过栅格的情况（`route_data.points` 两两直连）
#[derive(Debug, Clone)]
pub(crate) struct LegCrossing {
    /// 起点航点号（从 1 开始，与校验文案一致）
    pub from_index: usize,
    pub to_index: usize,
    pub length_m: f64,
    /// 落在占据格的累计长度（米）
    pub occupied_m: f64,
    /// 落在未知/不可通行区的累计长度（米）
    pub unknown_m: f64,
    /// 其中由旧「未知 205」画笔造成的长度（米，已含在 `unknown_m` 里）
    pub legacy_205_m: f64,
    /// 航段走出地图范围的累计长度（米）
    pub outside_m: f64,
    pub longest_occupied_run_m: f64,
    pub longest_unknown_run_m: f64,
}

impl LegCrossing {
    fn new(from_index: usize, length_m: f64) -> Self {
        Self {
            from_index,
            to_index: from_index + 1,
            length_m,
            occupied_m: 0.0,
            unknown_m: 0.0,
            legacy_205_m: 0.0,
            outside_m: 0.0,
            longest_occupied_run_m: 0.0,
            longest_unknown_run_m: 0.0,
        }
    }
}

/// 逐段采样栅格：算出每段穿过多少占据 / 未知 / 图外的长度。
///
/// 只检查平台**实际下发**的相邻航点（`P1→P2→…→Pn`）；返航段由本体按 `end_task`
/// 自己规划，平台不下发那一段，检查它只会误报。
pub(crate) fn leg_crossings(field: &ClearanceField, data: &Value) -> Vec<LegCrossing> {
    let Some(points) = data.get("points").and_then(Value::as_array) else {
        return Vec::new();
    };
    // 保留原始下标：某个点缺坐标时只跳过那一段，别让后面的 `第 N→N+1 段` 整体前移
    points
        .windows(2)
        .enumerate()
        .filter_map(|(index, pair)| {
            let from = point_xy(&pair[0])?;
            let to = point_xy(&pair[1])?;
            Some(leg_crossing(field, index + 1, from, to))
        })
        .collect()
}

fn point_xy(point: &Value) -> Option<(f64, f64)> {
    let x = point.get("x").and_then(Value::as_f64)?;
    let y = point.get("y").and_then(Value::as_f64)?;
    Some((x, y))
}

fn leg_crossing(
    field: &ClearanceField,
    from_index: usize,
    from: (f64, f64),
    to: (f64, f64),
) -> LegCrossing {
    let length = (to.0 - from.0).hypot(to.1 - from.1);
    let mut crossing = LegCrossing::new(from_index, length);
    if length <= f64::EPSILON {
        return crossing;
    }
    // 半个栅格一步：墙线一般 1~2 像素，步长取整格会漏掉；上限只是防病态坐标
    let step = (field.resolution * 0.5).max(1e-6);
    let samples = ((length / step).ceil() as usize).clamp(1, 50_000);
    let seg_m = length / samples as f64;
    let mut current: Option<CellKind> = None;
    let mut run_m = 0.0_f64;
    for index in 0..samples {
        let t = (index as f64 + 0.5) / samples as f64;
        let (x, y) = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        let (kind, legacy) = match field.value_at(x, y) {
            Some(value) => (Some(classify(value)), value == LEGACY_UNKNOWN_PAINT),
            None => (None, false),
        };
        match kind {
            Some(CellKind::Occupied) => crossing.occupied_m += seg_m,
            Some(CellKind::Unknown) => {
                crossing.unknown_m += seg_m;
                if legacy {
                    crossing.legacy_205_m += seg_m;
                }
            }
            Some(CellKind::Free) => {}
            None => crossing.outside_m += seg_m,
        }
        if kind != current {
            close_run(current, run_m, &mut crossing);
            current = kind;
            run_m = 0.0;
        }
        run_m += seg_m;
    }
    close_run(current, run_m, &mut crossing);
    crossing
}

fn close_run(kind: Option<CellKind>, run_m: f64, crossing: &mut LegCrossing) {
    match kind {
        Some(CellKind::Occupied) if run_m > crossing.longest_occupied_run_m => {
            crossing.longest_occupied_run_m = run_m;
        }
        Some(CellKind::Unknown) if run_m > crossing.longest_unknown_run_m => {
            crossing.longest_unknown_run_m = run_m;
        }
        _ => {}
    }
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
    let has_obstacles = occupied.iter().any(|value| *value);
    let resolution = frame.resolution;
    // 一个占据格都没有（全 unknown / 新建图）：不算 EDT，只留下栅格供航段校验；
    // `clearance_m` 会返回 None，调用方据此跳过净空项。
    let dist_m = if has_obstacles {
        let dist_sq = squared_edt(&occupied, width, height);
        let max_sq = (MAX_CLEARANCE_M / resolution).powi(2);
        dist_sq
            .into_iter()
            .map(|value| value.min(max_sq).sqrt() as f32 * resolution as f32)
            .collect()
    } else {
        vec![MAX_CLEARANCE_M as f32; width * height]
    };
    Some(ClearanceField {
        resolution,
        origin_x: frame.origin_x,
        origin_y: frame.origin_y,
        width,
        height,
        dist_m,
        values: raster.pixels.clone(),
        has_obstacles,
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
        // 全 unknown（128）：没有占据格 → 没有净空可言，`clearance_m` 返回 None
        // 让调用方跳过净空项；但字段仍然保留灰度，航段校验还要用它。
        let mut pixels = vec![128_u8; 100];
        pixels[0] = 128;
        let raster = RosRaster {
            width: 10,
            height: 10,
            pixels,
        };
        let field = field_from_raster(&raster, &frame(10, 10)).expect("field");
        let (x, y) = map_xy(0, 0, 10);
        assert!(field.clearance_m(x, y).is_none());
    }

    /// 40×10 的栅格，把 `cols` 这些列改成 `value`（其余 254）
    fn raster_with_band(cols: std::ops::Range<u32>, value: u8) -> RosRaster {
        let mut pixels = vec![254_u8; 40 * 10];
        for row in 0..10 {
            for col in cols.clone() {
                pixels[(row * 40 + col) as usize] = value;
            }
        }
        RosRaster {
            width: 40,
            height: 10,
            pixels,
        }
    }

    fn straight_leg(field: &ClearanceField) -> Vec<LegCrossing> {
        let (x0, y0) = map_xy(0, 5, 10);
        let (x1, _) = map_xy(39, 5, 10);
        let data = json!({"points": [{"x": x0, "y": y0}, {"x": x1, "y": y0}]});
        leg_crossings(field, &data)
    }

    /// 航段里连续 20 列（≈1 m）的 205：既是未知，也要单独记下「旧画笔」长度
    #[test]
    fn leg_crossing_measures_unknown_and_legacy_205() {
        let field =
            field_from_raster(&raster_with_band(10..30, 205), &frame(40, 10)).expect("field");
        let crossings = straight_leg(&field);
        assert_eq!(crossings.len(), 1);
        let hit = &crossings[0];
        assert_eq!(hit.from_index, 1);
        assert_eq!(hit.to_index, 2);
        assert!((hit.length_m - 39.0 * RES).abs() < 1e-6);
        assert!(hit.occupied_m.abs() < 1e-6, "205 不是障碍");
        assert!(
            (hit.unknown_m - 20.0 * RES).abs() < 0.06,
            "未知长度应≈1 m，实际 {}",
            hit.unknown_m
        );
        assert!(
            (hit.legacy_205_m - hit.unknown_m).abs() < 1e-6,
            "这段未知全是 205 画的"
        );
        assert!(hit.longest_unknown_run_m >= 0.9);
        assert!(hit.outside_m.abs() < 1e-6);
    }

    /// 128 也是未知（ROS 语义），但不计入 legacy_205
    #[test]
    fn leg_crossing_counts_128_as_unknown_without_legacy_hint() {
        let field =
            field_from_raster(&raster_with_band(10..30, 128), &frame(40, 10)).expect("field");
        let hit = &straight_leg(&field)[0];
        assert!(hit.unknown_m > 0.9);
        assert!(hit.legacy_205_m.abs() < 1e-6);
    }

    /// 中间 4 列（0.2 m）障碍：只报 0.2 m 连续占据，不要按整段长度算
    #[test]
    fn leg_crossing_measures_longest_occupied_run() {
        let field = field_from_raster(&raster_with_band(18..24, 0), &frame(40, 10)).expect("field");
        let hit = &straight_leg(&field)[0];
        assert!(
            (hit.longest_occupied_run_m - 6.0 * RES).abs() < 0.06,
            "连续占据应≈0.2 m，实际 {}",
            hit.longest_occupied_run_m
        );
        assert!(
            hit.occupied_m > 0.25,
            "累计占据应≈0.3 m，实际 {}",
            hit.occupied_m
        );
        assert!(hit.unknown_m.abs() < 1e-6);
    }

    /// 零长度航段（两个航点重合）不报错、全 0
    #[test]
    fn zero_length_leg_has_no_crossings() {
        let field = field_from_raster(&raster_with_band(10..30, 0), &frame(40, 10)).expect("field");
        let (x, y) = map_xy(35, 5, 10);
        let data = json!({"points": [{"x": x, "y": y}, {"x": x, "y": y}]});
        let hit = &leg_crossings(&field, &data)[0];
        assert_eq!(hit.length_m, 0.0);
        assert_eq!(hit.occupied_m, 0.0);
        assert_eq!(hit.unknown_m, 0.0);
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
