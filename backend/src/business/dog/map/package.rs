//! 地图包（`.zip`）解析：pgm → png，并读取 `model.yaml` 的分辨率与原点。
//!
//! 与参考实现（`DronesRouteLibraryServiceImp#getRoute`）保持一致：
//! 地图包解压后包含 ROS map_server 格式的 `model.pgm` 与 `model.yaml`，
//! `yaml` 里的 `resolution` 与 `origin` 用于前端把世界坐标换算成像素坐标。
//!
//! 机器人本体回传的包通常同时带两份栅格：根目录的 `map.pgm`（建图原始地图）
//! 和 `map_edit/map.pgm`（遥控器编辑后的地图）。平台展示、编辑、下发一律以
//! `map_edit/` 为准，原始地图只作为回退——绝不能把平台编辑写到根目录的
//! `map.pgm` 上，否则下次同步会覆盖掉本体上的原始建图。

use std::io::{Cursor, Write};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sha2::{Digest, Sha256};

use crate::business::dog::map::domain::{MapEditPoint, MapEditStroke};
use crate::common::error::AppError;

/// 地图包解析结果（未编码的中间结构）
#[derive(Debug, Clone)]
pub struct ParsedMapPackage {
    pub png: Vec<u8>,
    pub resolution: f64,
    pub origin: Vec<f64>,
    pub width: u32,
    pub height: u32,
    /// 地图包内容的 SHA-256（hex），用于发布基线与设备执行前一致性校验。
    pub sha256: String,
}

impl ParsedMapPackage {
    /// 转成 `data:image/png;base64,...` 形式，便于前端直接放进 `<img src>`
    pub fn png_data_url(&self) -> String {
        format!("data:image/png;base64,{}", BASE64.encode(&self.png))
    }
}

const DEFAULT_RESOLUTION: f64 = 0.05;
const DEFAULT_ORIGIN: [f64; 3] = [0.0, 0.0, 0.0];

/// 单笔擦除的最大半径（像素）。避免一次请求把整张图刷成同一种值。
pub const MAX_ERASE_RADIUS: f64 = 500.0;
/// 单次编辑允许的最大笔画数 / 采样点总数（防止前端异常或恶意请求）。
pub const MAX_EDIT_STROKES: usize = 2_000;
pub const MAX_EDIT_POINTS: usize = 200_000;

/// 橡皮擦写入「可通行」的灰度值（也是 ROS `map_saver` 的自由空间值）。
pub const ERASE_FREE: u8 = 254;
/// 橡皮擦写入「未知 / 不可通行」的灰度值。
///
/// **必须是 128，不能是 205**：本体按地图包的 `map.yaml` 做 trinary 判定
/// （`p = (255 - v) / 255`，`free_thresh: 0.2 / occupied_thresh: 0.5`）：
///
/// - `128 → p = 0.498` → 未知，规划器不走；
/// - `205 → p = 0.1961` → **可通行**（只有 `v > 204` 才算 free）。
///
/// ROS `map_saver` 把未知写成 205，是因为它的默认 `free_thresh` 是 `0.196`；
/// 我们这张图的 yaml 是 `0.2`，差 0.004 就把 205 翻到了自由一侧。
/// 2026-11-24 广汇二期 `TK-1124-17` 就是现场用「擦成未知 (205)」标了一大片
/// 不可通行区，本体照直穿过去、最后报 `13377 无有效绕障轨迹`。
pub const ERASE_UNKNOWN: u8 = 128;
/// 旧版前端写过的「未知」值。
///
/// 读进来时归一到 [`ERASE_UNKNOWN`]（见 [`normalize_erase_value`]），不再原样写回，
/// 否则旧页面缓存 / 旧客户端还会继续把 205（本体眼里的可通行）写进地图。
pub const LEGACY_ERASE_UNKNOWN: u8 = 205;

/// 橡皮擦灰度值规整：只允许「可通行 254 / 未知 128」，旧的 205 转成 128。
pub fn normalize_erase_value(value: u8) -> Result<u8, AppError> {
    match value {
        ERASE_FREE | ERASE_UNKNOWN => Ok(value),
        LEGACY_ERASE_UNKNOWN => Ok(ERASE_UNKNOWN),
        other => Err(AppError::BadRequest(format!(
            "橡皮擦只能写 {ERASE_FREE}（可通行）或 {ERASE_UNKNOWN}（未知），收到 {other}；\
             205 是旧版「未知」值（本体按 map.yaml 会当成可通行），请改用 {ERASE_UNKNOWN}"
        ))),
    }
}

/// 解析地图包 zip：找出栅格 PGM 与 YAML，把栅格地图转成 PNG。
///
/// 选择顺序：`map_edit/` 下的条目 → `model.pgm` → 第一个 `*.pgm`。
/// `map_edit/` 是遥控器编辑后的地图，平台必须优先用它；只有包内没有编辑版时
/// 才退回原始的建图栅格。兼容包内多层目录（例如 `map/model.pgm`）。
pub fn parse_map_package(bytes: &[u8]) -> Result<ParsedMapPackage, AppError> {
    let sha256 = sha256_hex(bytes);
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| AppError::BadRequest(format!("地图包解析失败: {error}")))?;

    let pgm_name = find_entry(&mut archive, "model.pgm", ".pgm").ok_or_else(|| {
        AppError::BadRequest(
            "地图包内未找到可用的 .pgm（map_edit/map.pgm、model.pgm 或 map.pgm）".to_string(),
        )
    })?;
    let pgm = read_entry(&mut archive, &pgm_name)?;

    let yaml_name = find_entry(&mut archive, "model.yaml", ".yaml")
        .or_else(|| find_entry(&mut archive, "model.yml", ".yml"));
    let yaml = match yaml_name {
        Some(name) => read_entry(&mut archive, &name)?,
        None => Vec::new(),
    };

    let image = image::load_from_memory(&pgm)
        .map_err(|error| AppError::BadRequest(format!("地图文件不是有效的 PGM: {error}")))?;
    let width = image.width();
    let height = image.height();

    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| AppError::InternalError(format!("地图转码失败: {error}")))?;

    let yaml_text = String::from_utf8_lossy(&yaml);
    let resolution = parse_resolution(&yaml_text).unwrap_or(DEFAULT_RESOLUTION);
    let origin = parse_origin(&yaml_text).unwrap_or_else(|| DEFAULT_ORIGIN.to_vec());

    Ok(ParsedMapPackage {
        png,
        resolution,
        origin,
        width,
        height,
        sha256,
    })
}

/// 计算字节内容的 SHA-256（小写 hex）。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 可编辑的栅格：统一归一化成 8 位灰度（0 = 占据，128 = 未知，254 = 自由）。
///
/// 注意：平台早期把「未知」写成 205，那是 ROS `map_saver` 配合
/// `free_thresh: 0.196` 的约定；本体这张图的 yaml 是 `0.2`，205 会被判成可通行。
/// 现在统一 `128 = 未知`（见 [`ERASE_UNKNOWN`]），205 只作为读入兼容。
///
/// ROS `map_saver` 产出的是 8 位 `P5`，这里额外兼容 `P2`（ASCII）与 16 位
/// `P5`：16 位在首次编辑时按比例缩放到 8 位（`occupied_thresh` 是比例阈值，
/// 缩放不改变语义）。
#[derive(Debug, Clone)]
pub struct PgmRaster {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl PgmRaster {
    /// 编码成 ROS 通用的 8 位二进制 PGM（`P5` + `maxval 255`）。
    pub fn to_p5(&self) -> Vec<u8> {
        let mut out = format!("P5\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.extend_from_slice(&self.pixels);
        out
    }
}

/// 解析 PGM（`P2` / `P5`）栅格，返回 8 位灰度像素。
///
/// 与 `parse_map_package` 里的 `image` 解码不同，这里保留原始像素值本身，
/// 编辑后再原样写回，避免 254（自由空间）被色彩空间转换改写成 255。
pub fn parse_pgm_raster(bytes: &[u8]) -> Result<PgmRaster, AppError> {
    let mut cursor = HeaderCursor::new(bytes);
    let magic = cursor
        .next_token()
        .ok_or_else(|| AppError::BadRequest("地图文件不是有效的 PGM：缺少文件头".to_string()))?;
    let binary = match magic {
        b"P5" => true,
        b"P2" => false,
        other => {
            return Err(AppError::BadRequest(format!(
                "地图文件不是有效的 PGM：不支持的魔数 {}",
                String::from_utf8_lossy(other)
            )));
        }
    };

    let width = parse_header_number(cursor.next_token(), "宽度")?;
    let height = parse_header_number(cursor.next_token(), "高度")?;
    let maxval = parse_header_number(cursor.next_token(), "最大灰度值")?;
    if width == 0 || height == 0 {
        return Err(AppError::BadRequest(
            "地图栅格尺寸为 0，无法解析".to_string(),
        ));
    }
    if maxval == 0 || maxval > 65_535 {
        return Err(AppError::BadRequest(format!(
            "地图最大灰度值 {maxval} 不受支持"
        )));
    }

    let count = usize::try_from(u64::from(width) * u64::from(height))
        .map_err(|_| AppError::BadRequest("地图栅格过大".to_string()))?;

    let scale = |value: u32| -> u8 {
        if maxval == 255 {
            value as u8
        } else {
            // 四舍五入到 0..=255
            (((value as u64) * 255 + (maxval as u64) / 2) / maxval as u64) as u8
        }
    };

    let mut pixels = Vec::with_capacity(count);
    if binary {
        // `maxval` 之后必须紧跟一个空白字符，其后即为像素数据。
        cursor.skip_one_whitespace();
        let data = cursor.remaining();
        if maxval < 256 {
            let data = data
                .get(..count)
                .ok_or_else(|| AppError::BadRequest("地图像素数据不完整".to_string()))?;
            if maxval == 255 {
                pixels.extend_from_slice(data);
            } else {
                pixels.extend(data.iter().map(|value| scale(u32::from(*value))));
            }
        } else {
            let data = data
                .get(..count * 2)
                .ok_or_else(|| AppError::BadRequest("地图像素数据不完整".to_string()))?;
            pixels.extend(
                data.chunks_exact(2)
                    .map(|pair| scale(u32::from(u16::from_be_bytes([pair[0], pair[1]])))),
            );
        }
    } else {
        for _ in 0..count {
            let token = cursor
                .next_token()
                .ok_or_else(|| AppError::BadRequest("地图像素数据不完整".to_string()))?;
            let value = std::str::from_utf8(token)
                .ok()
                .and_then(|text| text.parse::<u32>().ok())
                .ok_or_else(|| AppError::BadRequest("地图像素数据不是数字".to_string()))?;
            if value > maxval {
                return Err(AppError::BadRequest(format!(
                    "地图像素值 {value} 超过最大灰度值 {maxval}"
                )));
            }
            pixels.push(scale(value));
        }
    }

    Ok(PgmRaster {
        width,
        height,
        pixels,
    })
}

/// 在地图包基础上重写栅格 PGM（橡皮擦），返回新的 `.zip` 字节与改写的像素数。
///
/// 目标与 [`parse_map_package`] 一致：有 `map_edit/` 就改编辑版，否则才改
/// `model.pgm` / `map.pgm`。根目录的原始地图条目原样保留，平台编辑不会污染
/// 本体上的建图原始数据。除被编辑的 PGM 外，其余条目（yaml、自定义标注等）
/// 按原顺序原样复制，因此分辨率 / 原点 / negate 等元数据保持不变。
///
/// `erase_value` 只是兜底：每笔优先用 `stroke.value`（同一次编辑允许混着擦成
/// 可通行 254 / 未知 128；旧的 205 会被归一成 128）。
pub fn rewrite_map_package(
    bytes: &[u8],
    strokes: &[MapEditStroke],
    erase_value: u8,
) -> Result<(Vec<u8>, usize), AppError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| AppError::BadRequest(format!("地图包解析失败: {error}")))?;
    let pgm_name = find_entry(&mut archive, "model.pgm", ".pgm").ok_or_else(|| {
        AppError::BadRequest(
            "地图包内未找到可用的 .pgm（map_edit/map.pgm、model.pgm 或 map.pgm）".to_string(),
        )
    })?;
    let pgm = read_entry(&mut archive, &pgm_name)?;
    let mut raster = parse_pgm_raster(&pgm)?;
    let erased_pixels = apply_erase_strokes(
        &mut raster.pixels,
        raster.width,
        raster.height,
        strokes,
        erase_value,
    )?;
    let edited = raster.to_p5();

    // 橡皮擦的笔画同时记进 `map_edit/edit_info.json`：遥控器下次保存时是
    // 「根图 + edit_info 里的擦除圆」重算栅格，不写进去的话它一保存就把平台
    // 的编辑整体冲掉（现场 11-25 复现四次）。只登记「擦成可通行 254」的笔画——
    // 遥控器的橡皮擦只有这一种落值，`擦成未知 128` 它表达不了。
    let circles = erase_circles(strokes, erase_value)?;
    let edit_info_name = find_entry_by_name(&mut archive, "edit_info.json")
        .filter(|name| is_map_edit_entry(&name.to_lowercase()));
    let merged_edit_info = if circles.is_empty() {
        None
    } else {
        let existing = match edit_info_name.as_deref() {
            Some(name) => Some(read_entry(&mut archive, name)?),
            None => None,
        };
        // 包里连 edit_info.json 都没有（老包 / 本体上没编辑过）：只有本体有
        // `map_edit/` 目录时才补一份，避免在纯建图原图上凭空造编辑记录。
        let has_map_edit_dir = is_map_edit_entry(&pgm_name.to_lowercase());
        if existing.is_none() && !has_map_edit_dir {
            None
        } else {
            merge_edit_info_circles(existing.as_deref(), &circles)
        }
    };

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| AppError::BadRequest(format!("读取地图包失败: {error}")))?;
            let name = entry.name().to_string();
            if entry.is_dir() {
                writer
                    .add_directory(name, options)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
                continue;
            }
            writer
                .start_file(name.clone(), options)
                .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            if name == pgm_name {
                writer
                    .write_all(&edited)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            } else if let Some(merged) = merged_edit_info.as_deref()
                && Some(name.as_str()) == edit_info_name.as_deref()
            {
                writer
                    .write_all(merged)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            } else {
                std::io::copy(&mut entry, &mut writer)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            }
        }
        // 包内原本没有 `edit_info.json`（本体上这张图还没在遥控器里编辑过）：
        // 补一份只含本次擦除的记录，遥控器下次保存时就能带上。
        if let Some(merged) = merged_edit_info.as_deref()
            && edit_info_name.is_none()
        {
            writer
                .start_file("map_edit/edit_info.json".to_string(), options)
                .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            writer
                .write_all(merged)
                .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
        }
        writer
            .finish()
            .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
    }

    Ok((buffer.into_inner(), erased_pixels))
}

/// `edit_info.json` 里登记的擦除圆上限，防止反复下发把文件撑爆。
pub const MAX_EDIT_INFO_CIRCLES: usize = 20_000;
/// `edit_info.json` 的体积上限：超过就放弃合并（下发本身不受影响）。
pub const MAX_EDIT_INFO_BYTES: usize = 8 * 1024 * 1024;

/// 圆的去重精度：保留两位小数后相同的圆视为同一条（同一笔反复下发不会累积）。
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn circle_key(x: f64, y: f64, r: f64) -> (i64, i64, i64) {
    (
        (x * 100.0).round() as i64,
        (y * 100.0).round() as i64,
        (r * 100.0).round() as i64,
    )
}

/// 按「文件名（不含目录）」在包里找条目，大小写不敏感。
fn find_entry_by_name<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let name = name.to_lowercase();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let entry_name = entry.name().to_string();
        let base = entry_name
            .rsplit('/')
            .next()
            .unwrap_or(entry_name.as_str())
            .to_lowercase();
        if base == name {
            return Some(entry_name);
        }
    }
    None
}

/// 把笔画转成遥控器 `edit_info.json` 的擦除圆（`{x, y, r}`，像素坐标）。
///
/// 只有「擦成可通行 `254`」的笔画才登记：遥控器的橡皮擦只有这一种落值，
/// `擦成未知 128` 它表达不了（写进去会被当成 254），那类编辑只能靠写栅格本身。
pub fn erase_circles(
    strokes: &[MapEditStroke],
    erase_value: u8,
) -> Result<Vec<serde_json::Value>, AppError> {
    let erase_value = normalize_erase_value(erase_value)?;
    let mut circles: Vec<serde_json::Value> = Vec::new();
    let mut seen: std::collections::HashSet<(i64, i64, i64)> = std::collections::HashSet::new();
    for stroke in strokes {
        let value = stroke
            .value
            .map_or(Ok(erase_value), normalize_erase_value)?;
        if value != ERASE_FREE {
            continue;
        }
        let radius = stroke.radius.unwrap_or(4.0).clamp(0.5, MAX_ERASE_RADIUS);
        if !radius.is_finite() {
            continue;
        }
        for point in &stroke.points {
            if !point.x.is_finite() || !point.y.is_finite() {
                continue;
            }
            if circles.len() >= MAX_EDIT_INFO_CIRCLES {
                return Ok(circles);
            }
            let (x, y, r) = (round2(point.x), round2(point.y), round2(radius));
            if !seen.insert(circle_key(x, y, r)) {
                continue;
            }
            circles.push(serde_json::json!({ "x": x, "y": y, "r": r }));
        }
    }
    Ok(circles)
}

/// 把本次擦除圆合并进 `map_edit/edit_info.json`。
///
/// 遥控器的编辑记录结构（逆向自它自己写的文件）：
/// `{ erase_areas: [{x,y,r}], forbidden_areas: [], fences: [], edit_ops: [{type,circles}] }`。
/// 规则：
/// - **追加不覆盖**：`erase_areas` 里已有的圆（含遥控器自己的）原样保留，只补新的；
/// - `forbidden_areas` / `fences` 等平台不产生的字段原样保留；
/// - 老文件缺这些键时补空数组，未知字段不动；
/// - 已有文件解析失败、或结果超限 → 返回 `None`（跳过合并，下发照旧）。
pub fn merge_edit_info_circles(
    existing: Option<&[u8]>,
    circles: &[serde_json::Value],
) -> Option<Vec<u8>> {
    if circles.is_empty() {
        return None;
    }
    let mut root: serde_json::Map<String, serde_json::Value> = match existing {
        Some(bytes) if !bytes.is_empty() => {
            let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
            match value {
                serde_json::Value::Object(map) => map,
                // 遥控器的文件永远是对象；不是对象说明我们看不懂，别动它
                _ => return None,
            }
        }
        _ => serde_json::Map::new(),
    };

    for key in ["erase_areas", "forbidden_areas", "fences", "edit_ops"] {
        match root.get(key) {
            None => {
                root.insert(key.to_string(), serde_json::Value::Array(Vec::new()));
            }
            Some(serde_json::Value::Array(_)) => {}
            // 结构对不上就不碰，宁可不同步也不要写坏遥控器的编辑记录
            Some(_) => return None,
        }
    }

    let known: std::collections::HashSet<(i64, i64, i64)> = root
        .get("erase_areas")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let x = item.get("x")?.as_f64()?;
                    let y = item.get("y")?.as_f64()?;
                    let r = item.get("r")?.as_f64()?;
                    Some(circle_key(x, y, r))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut added: Vec<serde_json::Value> = Vec::new();
    for circle in circles {
        let x = circle.get("x").and_then(serde_json::Value::as_f64)?;
        let y = circle.get("y").and_then(serde_json::Value::as_f64)?;
        let r = circle.get("r").and_then(serde_json::Value::as_f64)?;
        if known.contains(&circle_key(x, y, r)) {
            continue;
        }
        if added.len() + known.len() >= MAX_EDIT_INFO_CIRCLES {
            break;
        }
        added.push(circle.clone());
    }
    if added.is_empty() {
        return None;
    }

    if let Some(serde_json::Value::Array(areas)) = root.get_mut("erase_areas") {
        areas.extend(added.iter().cloned());
    }
    if let Some(serde_json::Value::Array(ops)) = root.get_mut("edit_ops") {
        ops.push(serde_json::json!({
            "type": "erase",
            "circles": added,
        }));
    }

    let value = serde_json::Value::Object(root);
    let out = serde_json::to_vec_pretty(&value).ok()?;
    if out.len() > MAX_EDIT_INFO_BYTES {
        return None;
    }
    Some(out)
}

/// 遥控器「没有编辑」时的空 `edit_info.json`（结构与它自己写出来的一致）。
pub fn empty_edit_info() -> Vec<u8> {
    let value = serde_json::json!({
        "erase_areas": [],
        "forbidden_areas": [],
        "fences": [],
        "edit_ops": [],
    });
    serde_json::to_vec_pretty(&value).unwrap_or_else(|_| b"{}".to_vec())
}

/// 把地图包恢复成「建图原图」。
///
/// - `map_edit/map.pgm` ← 根目录的建图原图（`map.pgm` / `model.pgm`）；
/// - `map_edit/edit_info.json` ← 空记录：遥控器保存时是按「根图 + 这些擦除圆」
///   重算栅格的，不清掉它的编辑历史，下次它一保存就把旧擦除又画回来；
/// - 其余条目（`map.pcd` / `model_*.bin` / `nav_paths` / 根目录原图等）原样保留。
///
/// 包内没有 `map_edit/`（手工上传的 ROS 包）直接拒绝：那种图本来就是原图。
pub fn restore_original_package(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| AppError::BadRequest(format!("地图包解析失败: {error}")))?;
    let mut edit_pgm: Option<String> = None;
    let mut root_map: Option<String> = None;
    let mut root_model: Option<String> = None;
    let mut root_other: Option<String> = None;
    let mut edit_info: Option<String> = None;
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let lower = name.to_lowercase();
        let base = lower.rsplit('/').next().unwrap_or(lower.as_str());
        if base == "edit_info.json" {
            if is_map_edit_entry(&lower) {
                edit_info.get_or_insert(name);
            }
            continue;
        }
        if !lower.ends_with(".pgm") {
            continue;
        }
        if is_map_edit_entry(&lower) {
            edit_pgm.get_or_insert(name);
        } else if base == "map.pgm" {
            root_map.get_or_insert(name);
        } else if base == "model.pgm" {
            root_model.get_or_insert(name);
        } else {
            root_other.get_or_insert(name);
        }
    }

    let edit_pgm = edit_pgm.ok_or_else(|| {
        AppError::BadRequest("这张地图没有编辑版（手工上传的地图包），本来就是建图原图".to_string())
    })?;
    let root_name = root_map.or(root_model).or(root_other).ok_or_else(|| {
        AppError::BadRequest("地图包里没有建图原图（根目录 map.pgm），无法恢复".to_string())
    })?;
    let root_bytes = read_entry(&mut archive, &root_name)?;
    let root_pgm = parse_pgm_raster(&root_bytes)?;
    let restored_pgm = root_pgm.to_p5();
    let reset_edit_info = empty_edit_info();

    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| AppError::BadRequest(format!("读取地图包失败: {error}")))?;
            let name = entry.name().to_string();
            if entry.is_dir() {
                writer
                    .add_directory(name, options)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
                continue;
            }
            writer
                .start_file(name.clone(), options)
                .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            if name == edit_pgm {
                writer
                    .write_all(&restored_pgm)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            } else if Some(name.as_str()) == edit_info.as_deref() {
                writer
                    .write_all(&reset_edit_info)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            } else {
                std::io::copy(&mut entry, &mut writer)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            }
        }
        // 包里原本没有编辑记录：补一份空的，遥控器下次保存就不会带回旧擦除
        if edit_info.is_none() {
            writer
                .start_file("map_edit/edit_info.json".to_string(), options)
                .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            writer
                .write_all(&reset_edit_info)
                .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
        }
        writer
            .finish()
            .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
    }
    Ok(buffer.into_inner())
}

/// 把橡皮擦笔画刷进像素缓冲区，返回实际被改写的像素数。
///
/// 每笔写入自己的 `stroke.value`，缺省时用 `erase_value`。两个值都会先过
/// [`normalize_erase_value`]：旧的 205 归一成 128（本体眼里的未知），其它值直接拒绝。
pub fn apply_erase_strokes(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    strokes: &[MapEditStroke],
    erase_value: u8,
) -> Result<usize, AppError> {
    if pixels.len() != width as usize * height as usize {
        return Err(AppError::InternalError(
            "栅格像素数量与尺寸不一致".to_string(),
        ));
    }
    let erase_value = normalize_erase_value(erase_value)?;
    let mut changed = 0usize;
    for stroke in strokes {
        if stroke.points.is_empty() {
            continue;
        }
        let radius = stroke.radius.unwrap_or(4.0).clamp(0.5, MAX_ERASE_RADIUS);
        if !radius.is_finite() {
            return Err(AppError::BadRequest("笔刷半径无效".to_string()));
        }
        // 值跟着笔画走：同一批里可能既擦成可通行（254）又擦成未知（128）。
        // 没有 `value` 的老客户端回退到请求级 `eraseValue`。
        let value = stroke
            .value
            .map_or(Ok(erase_value), normalize_erase_value)?;
        for point in &stroke.points {
            if !point.x.is_finite() || !point.y.is_finite() {
                return Err(AppError::BadRequest("笔画坐标无效".to_string()));
            }
        }

        changed += stamp(pixels, width, height, &stroke.points[0], radius, value);
        for pair in stroke.points.windows(2) {
            changed += stamp_segment(pixels, width, height, &pair[0], &pair[1], radius, value);
        }
    }
    Ok(changed)
}

/// 沿两点连线按 `radius / 2` 的步长补点，避免快速拖动出现断点。
fn stamp_segment(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    from: &MapEditPoint,
    to: &MapEditPoint,
    radius: f64,
    value: u8,
) -> usize {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let distance = (dx * dx + dy * dy).sqrt();
    let step = (radius / 2.0).max(0.5);
    let steps = (distance / step).ceil() as usize;
    let mut changed = 0usize;
    for index in 1..=steps {
        let ratio = index as f64 / steps as f64;
        let point = MapEditPoint {
            x: from.x + dx * ratio,
            y: from.y + dy * ratio,
        };
        changed += stamp(pixels, width, height, &point, radius, value);
    }
    changed
}

/// 以 `(point.x, point.y)` 为圆心绘制实心圆，只统计真正被改写的像素。
fn stamp(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    point: &MapEditPoint,
    radius: f64,
    value: u8,
) -> usize {
    let radius_squared = radius * radius;
    let max_x = width as f64 - 1.0;
    let max_y = height as f64 - 1.0;
    let start_x = (point.x - radius).floor().clamp(0.0, max_x) as u32;
    let end_x = (point.x + radius).ceil().clamp(0.0, max_x) as u32;
    let start_y = (point.y - radius).floor().clamp(0.0, max_y) as u32;
    let end_y = (point.y + radius).ceil().clamp(0.0, max_y) as u32;

    let mut changed = 0usize;
    for y in start_y..=end_y {
        let dy = y as f64 + 0.5 - point.y;
        for x in start_x..=end_x {
            let dx = x as f64 + 0.5 - point.x;
            if dx * dx + dy * dy > radius_squared {
                continue;
            }
            let index = y as usize * width as usize + x as usize;
            if pixels[index] != value {
                pixels[index] = value;
                changed += 1;
            }
        }
    }
    changed
}

fn parse_header_number(token: Option<&[u8]>, label: &str) -> Result<u32, AppError> {
    let token = token.ok_or_else(|| AppError::BadRequest(format!("PGM 文件头缺少{label}")))?;
    std::str::from_utf8(token)
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .ok_or_else(|| AppError::BadRequest(format!("PGM 文件头的{label}不是有效数字")))
}

/// PGM 文件头的逐 token 读取器（跳过空白与 `#` 注释）。
struct HeaderCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> HeaderCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> &'a [u8] {
        &self.bytes[self.pos.min(self.bytes.len())..]
    }

    fn skip_blank(&mut self) {
        loop {
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            if self.pos < self.bytes.len() && self.bytes[self.pos] == b'#' {
                while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            break;
        }
    }

    fn next_token(&mut self) -> Option<&'a [u8]> {
        self.skip_blank();
        let start = self.pos;
        while self.pos < self.bytes.len() && !self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
        (self.pos > start).then(|| &self.bytes[start..self.pos])
    }

    /// `maxval` 之后只消费一个换行/空白；像素数据里的 0x20 / 0x0A 不能被吞掉。
    fn skip_one_whitespace(&mut self) {
        if self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            let carriage_return = self.bytes[self.pos] == b'\r';
            self.pos += 1;
            if carriage_return && self.pos < self.bytes.len() && self.bytes[self.pos] == b'\n' {
                self.pos += 1;
            }
        }
    }
}

/// 条目是否位于 `map_edit/` 目录（遥控器编辑后的地图）。
///
/// 只认完整路径段，避免把 `map_edit.bak` / `map_edited/` 误当成编辑版。
fn is_map_edit_entry(name: &str) -> bool {
    name.split('/')
        .any(|segment| segment.eq_ignore_ascii_case("map_edit"))
}

/// 查找地图包内的栅格 / 元数据条目，优先 `map_edit/`。
///
/// 返回顺序：编辑目录内的精确文件名 → 编辑目录内的同扩展名文件 →
/// 其他目录的精确文件名 → 其他目录的同扩展名文件。这样即使 `map_edit/`
/// 里是 `map.pgm` 而不是 `model.pgm`，也会优先命中编辑版。
fn find_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    preferred: &str,
    extension: &str,
) -> Option<String> {
    let preferred = preferred.to_lowercase();
    let extension = extension.to_lowercase();
    let mut edit_fallback: Option<String> = None;
    let mut preferred_match: Option<String> = None;
    let mut fallback: Option<String> = None;
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let lower = name.to_lowercase();
        let matches_preferred = lower == preferred || lower.ends_with(&format!("/{preferred}"));
        let matches_extension =
            lower.ends_with(&extension) || (extension == ".yaml" && lower.ends_with(".yml"));
        if is_map_edit_entry(&lower) {
            if matches_preferred {
                return Some(name);
            }
            if edit_fallback.is_none() && matches_extension {
                edit_fallback = Some(name);
            }
            continue;
        }
        if preferred_match.is_none() && matches_preferred {
            preferred_match = Some(name.clone());
        }
        if fallback.is_none() && matches_extension {
            fallback = Some(name);
        }
    }
    edit_fallback.or(preferred_match).or(fallback)
}

fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>, AppError> {
    use std::io::Read;

    let mut entry = archive
        .by_name(name)
        .map_err(|error| AppError::BadRequest(format!("读取地图包内 {name} 失败: {error}")))?;
    let mut buffer = Vec::new();
    entry
        .read_to_end(&mut buffer)
        .map_err(|error| AppError::BadRequest(format!("读取地图包内 {name} 失败: {error}")))?;
    Ok(buffer)
}

/// 从 yaml 文本里取 `resolution: 0.05`
fn parse_resolution(yaml: &str) -> Option<f64> {
    yaml.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("resolution:"))
        .and_then(|value| value.trim().parse::<f64>().ok())
}

/// 从 yaml 文本里取 `origin: [-8.7, -17.7, 0.0]`
fn parse_origin(yaml: &str) -> Option<Vec<f64>> {
    let raw = yaml
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("origin:"))?
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');

    let values: Vec<f64> = raw
        .split(',')
        .filter_map(|part| part.trim().parse::<f64>().ok())
        .collect();
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 生成一个 4x3 的 PGM(P5) 栅格图
    fn sample_pgm() -> Vec<u8> {
        let header = b"P5\n4 3\n255\n".to_vec();
        let pixels: Vec<u8> = (0..12u8).map(|v| v.saturating_mul(20)).collect();
        [header, pixels].concat()
    }

    fn build_zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options: zip::write::SimpleFileOptions = Default::default();
            for (name, data) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    #[test]
    fn parses_pgm_and_yaml_from_flat_package() {
        let yaml = b"image: model.pgm\nresolution: 0.05\norigin: [-8.7, -17.7, 0.0]\nnegate: 0\n";
        let zip = build_zip(&[("model.pgm", sample_pgm()), ("model.yaml", yaml.to_vec())]);

        let parsed = parse_map_package(&zip).unwrap();
        assert_eq!(parsed.width, 4);
        assert_eq!(parsed.height, 3);
        assert!((parsed.resolution - 0.05).abs() < f64::EPSILON);
        assert_eq!(parsed.origin, vec![-8.7, -17.7, 0.0]);
        assert_eq!(parsed.sha256.len(), 64);
        assert_eq!(parsed.sha256, sha256_hex(&zip));
        assert!(parsed.png_data_url().starts_with("data:image/png;base64,"));
    }

    #[test]
    fn parses_package_with_nested_directory() {
        let yaml = b"resolution: 0.1\norigin: [1, 2, 0]\n";
        let zip = build_zip(&[
            ("map/model.pgm", sample_pgm()),
            ("map/model.yaml", yaml.to_vec()),
        ]);

        let parsed = parse_map_package(&zip).unwrap();
        assert!((parsed.resolution - 0.1).abs() < f64::EPSILON);
        assert_eq!(parsed.origin, vec![1.0, 2.0, 0.0]);
    }

    #[test]
    fn falls_back_to_first_pgm_when_model_pgm_is_absent() {
        let zip = build_zip(&[("map.pgm", sample_pgm())]);
        let parsed = parse_map_package(&zip).unwrap();
        assert_eq!(parsed.width, 4);
        // 没有 yaml 时使用默认参数
        assert!((parsed.resolution - DEFAULT_RESOLUTION).abs() < f64::EPSILON);
        assert_eq!(parsed.origin, DEFAULT_ORIGIN.to_vec());
    }

    #[test]
    fn prefers_map_edit_over_original_raster() {
        // 根目录：4x3 的原始建图栅格（free=255）；map_edit：2x1 的编辑版（free=254）
        let original_yaml = b"resolution: 0.05\norigin: [-8.7, -17.7, 0.0]\n";
        let edited_yaml = b"resolution: 0.1\norigin: [1, 2, 0]\n";
        let edited_pgm = b"P5\n2 1\n255\n\x00\xfe".to_vec();
        let zip = build_zip(&[
            ("map.pgm", sample_pgm()),
            ("map.yaml", original_yaml.to_vec()),
            ("map_edit/map.pgm", edited_pgm),
            ("map_edit/map.yaml", edited_yaml.to_vec()),
        ]);

        let parsed = parse_map_package(&zip).unwrap();
        // 尺寸 / 元数据都应来自 map_edit，而不是根目录的原始地图
        assert_eq!(parsed.width, 2);
        assert_eq!(parsed.height, 1);
        assert!((parsed.resolution - 0.1).abs() < f64::EPSILON);
        assert_eq!(parsed.origin, vec![1.0, 2.0, 0.0]);

        let image = image::load_from_memory(&parsed.png).unwrap().to_luma8();
        assert_eq!(image.dimensions(), (2, 1));
        assert_eq!(image.get_pixel(0, 0)[0], 0);
        assert_eq!(image.get_pixel(1, 0)[0], 254);
    }

    #[test]
    fn erase_rewrites_map_edit_and_keeps_original() {
        let original_pixels = vec![255u8; 16];
        let original_pgm = [b"P5\n4 4\n255\n".to_vec(), original_pixels.clone()].concat();
        let mut edited_pixels = vec![255u8; 16];
        edited_pixels[0] = 0;
        let edited_pgm = [b"P5\n4 4\n255\n".to_vec(), edited_pixels].concat();
        let zip = build_zip(&[
            ("map.pgm", original_pgm.clone()),
            ("map.yaml", b"image: map.pgm\nresolution: 0.05\n".to_vec()),
            ("map_edit/map.pgm", edited_pgm),
            (
                "map_edit/map.yaml",
                b"image: map.pgm\nresolution: 0.05\n".to_vec(),
            ),
        ]);

        let strokes = vec![MapEditStroke {
            points: vec![MapEditPoint { x: 0.5, y: 0.5 }],
            radius: Some(0.6),
            value: None,
        }];
        let (edited, changed) = rewrite_map_package(&zip, &strokes, 254).unwrap();
        assert_eq!(changed, 1);

        let mut archive = zip::ZipArchive::new(Cursor::new(&edited)).unwrap();
        // 根目录的原始地图一个字节都不能变
        assert_eq!(read_entry(&mut archive, "map.pgm").unwrap(), original_pgm);
        // 编辑只落在 map_edit/map.pgm
        let raster =
            parse_pgm_raster(&read_entry(&mut archive, "map_edit/map.pgm").unwrap()).unwrap();
        assert_eq!(raster.pixels[0], 254);
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn rejects_package_without_pgm() {
        let zip = build_zip(&[("readme.txt", b"nothing here".to_vec())]);
        assert!(parse_map_package(&zip).is_err());
    }

    #[test]
    fn parses_ascii_pgm_with_comments() {
        let pgm = b"P2\n# CREATOR: map_saver\n3 2\n255\n0 205 254\n254 254 0\n";
        let raster = parse_pgm_raster(pgm).unwrap();
        assert_eq!(raster.width, 3);
        assert_eq!(raster.height, 2);
        assert_eq!(raster.pixels, vec![0, 205, 254, 254, 254, 0]);
    }

    #[test]
    fn erases_noise_and_keeps_other_entries() {
        // 8x8 全白（254）地图，中心 (4, 4) 与右上角 (7, 0) 各有一个噪点黑像素
        let mut pixels = vec![254u8; 64];
        pixels[4 * 8 + 4] = 0;
        pixels[7] = 0;
        let pgm = [b"P5\n8 8\n255\n".to_vec(), pixels].concat();
        let yaml = b"image: model.pgm\nresolution: 0.05\norigin: [-1, -2, 0]\n";
        let zip = build_zip(&[("map/model.pgm", pgm), ("map/model.yaml", yaml.to_vec())]);

        let strokes = vec![
            // 中心噪点
            MapEditStroke {
                points: vec![MapEditPoint { x: 4.5, y: 4.5 }],
                radius: Some(0.6),
                value: None,
            },
            // 右上角噪点：半径放大到 2 验证边界裁剪（不能越界 panic）
            MapEditStroke {
                points: vec![MapEditPoint { x: 7.5, y: 0.5 }],
                radius: Some(2.0),
                value: None,
            },
        ];
        let (edited, changed) = rewrite_map_package(&zip, &strokes, 254).unwrap();
        assert!(
            changed >= 2,
            "expected two noise pixels erased, got {changed}"
        );

        // 新包仍能被正常解析（PNG / 分辨率 / 原点不变）
        let parsed = parse_map_package(&edited).unwrap();
        assert_eq!(parsed.width, 8);
        assert_eq!(parsed.height, 8);
        assert!((parsed.resolution - 0.05).abs() < f64::EPSILON);
        assert_eq!(parsed.origin, vec![-1.0, -2.0, 0.0]);
        assert_eq!(parsed.sha256, sha256_hex(&edited));

        // model.yaml 原样保留，PGM 里的噪点被抹掉
        let mut archive = zip::ZipArchive::new(Cursor::new(&edited)).unwrap();
        assert!(archive.by_name("map/model.yaml").is_ok());
        let raster = parse_pgm_raster(&read_entry(&mut archive, "map/model.pgm").unwrap()).unwrap();
        assert_eq!(raster.pixels[4 * 8 + 4], 254);
        assert_eq!(raster.pixels[7], 254);
    }

    #[test]
    fn applies_per_stroke_value_so_mixed_edits_keep_their_own_gray() {
        // 8x1 全是障碍（0）：左半擦成可通行（254）、右半擦成未知（128）。
        // 请求级默认值故意给 254，只有笔画自带的值生效才算对。
        let mut pixels = vec![0u8; 8];
        let strokes = vec![
            MapEditStroke {
                points: vec![MapEditPoint { x: 1.5, y: 0.5 }],
                radius: Some(0.6),
                value: Some(254),
            },
            MapEditStroke {
                points: vec![MapEditPoint { x: 6.5, y: 0.5 }],
                radius: Some(0.6),
                value: Some(128),
            },
        ];
        apply_erase_strokes(&mut pixels, 8, 1, &strokes, 254).unwrap();
        assert_eq!(pixels[1], 254, "第一笔必须写成可通行");
        assert_eq!(pixels[6], 128, "第二笔必须写成未知");
    }

    /// 旧前端还在写「未知 205」：必须归一到 128，否则本体按 `free_thresh: 0.2`
    /// 会把它当成可通行（见 `ERASE_UNKNOWN` 的文档）。
    #[test]
    fn legacy_unknown_205_is_rewritten_as_128() {
        let mut pixels = vec![0u8; 4];
        let strokes = vec![MapEditStroke {
            points: vec![MapEditPoint { x: 2.0, y: 0.5 }],
            radius: Some(1.0),
            value: Some(LEGACY_ERASE_UNKNOWN),
        }];
        apply_erase_strokes(&mut pixels, 4, 1, &strokes, ERASE_FREE).unwrap();
        assert_eq!(pixels[2], ERASE_UNKNOWN, "205 必须落盘成 128");

        // 请求级 eraseValue 是 205（更老的客户端）时同样归一
        let mut pixels = vec![0u8; 4];
        apply_erase_strokes(
            &mut pixels,
            4,
            1,
            &[MapEditStroke {
                points: vec![MapEditPoint { x: 2.0, y: 0.5 }],
                radius: Some(1.0),
                value: None,
            }],
            LEGACY_ERASE_UNKNOWN,
        )
        .unwrap();
        assert_eq!(pixels[2], ERASE_UNKNOWN);
    }

    /// 只允许 254 / 128（+ 旧的 205）：别再造出第三种灰度让本体和平台各说各话。
    #[test]
    fn rejects_unsupported_erase_values() {
        let mut pixels = vec![0u8; 4];
        let strokes = vec![MapEditStroke {
            points: vec![MapEditPoint { x: 2.0, y: 0.5 }],
            radius: Some(1.0),
            value: Some(160),
        }];
        assert!(apply_erase_strokes(&mut pixels, 4, 1, &strokes, ERASE_FREE).is_err());
    }

    #[test]
    fn erases_along_segment_between_samples() {
        let mut pixels = vec![0u8; 20 * 10];
        let strokes = vec![MapEditStroke {
            points: vec![
                MapEditPoint { x: 2.0, y: 5.0 },
                MapEditPoint { x: 18.0, y: 5.0 },
            ],
            radius: Some(1.0),
            value: None,
        }];
        apply_erase_strokes(&mut pixels, 20, 10, &strokes, ERASE_UNKNOWN).unwrap();
        // 中间没有采样点的像素也必须被插值补上
        for x in 2..=18 {
            assert_eq!(pixels[5 * 20 + x], ERASE_UNKNOWN, "x={x} 未被擦除");
        }
        assert_eq!(pixels[0], 0);
    }

    #[test]
    fn normalizes_16bit_pgm_on_edit() {
        // 2x1 的 16 位 P5：0 与 65535 → 0 与 255
        let mut pgm = b"P5\n2 1\n65535\n".to_vec();
        pgm.extend_from_slice(&[0, 0, 0xff, 0xff]);
        let raster = parse_pgm_raster(&pgm).unwrap();
        assert_eq!(raster.pixels, vec![0, 255]);
    }

    #[test]
    fn rejects_malformed_pgm_header() {
        assert!(parse_pgm_raster(b"P5\n3\n").is_err());
        assert!(parse_pgm_raster(b"P3\n1 1\n255\n0 0 0\n").is_err());
        assert!(parse_pgm_raster(b"P5\n2 2\n255\n\x00").is_err());
    }

    // ---- 橡皮擦笔画登记进 edit_info.json（遥控器保存时按「根图 + ops」重算）----

    fn solid_pgm(width: u32, height: u32, value: u8) -> Vec<u8> {
        let header = format!("P5\n{width} {height}\n255\n").into_bytes();
        [header, vec![value; (width * height) as usize]].concat()
    }

    fn read_zip_text(bytes: &[u8], name: &str) -> Option<String> {
        use std::io::Read;
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
        let mut file = archive.by_name(name).ok()?;
        let mut text = String::new();
        file.read_to_string(&mut text).ok()?;
        Some(text)
    }

    fn erase_all_free(xs: &[(f64, f64)], radius: f64, value: u8) -> Vec<MapEditStroke> {
        vec![MapEditStroke {
            points: xs
                .iter()
                .map(|(x, y)| MapEditPoint { x: *x, y: *y })
                .collect(),
            radius: Some(radius),
            value: Some(value),
        }]
    }

    /// 平台擦除要追加进遥控器的编辑记录：已有的圆 / 围栏 / 未知字段一条都不能丢
    #[test]
    fn erase_appends_circles_and_keeps_remote_edit_record() {
        let existing = br#"{
  "erase_areas": [{"x": 1.0, "y": 1.0, "r": 0.5}],
  "forbidden_areas": [],
  "fences": [{"id": "fence-1"}],
  "edit_ops": [],
  "extra": "keep-me"
}"#;
        let zip = build_zip(&[
            ("map_edit/map.pgm", solid_pgm(8, 8, 128)),
            ("map_edit/map.yaml", b"image: map.pgm\n".to_vec()),
            ("map_edit/edit_info.json", existing.to_vec()),
        ]);
        let strokes = erase_all_free(&[(2.0, 2.0), (3.0, 3.0)], 1.0, 254);
        let (out, changed) = rewrite_map_package(&zip, &strokes, 254).unwrap();
        assert!(changed > 0);

        let text = read_zip_text(&out, "map_edit/edit_info.json").unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let areas = value["erase_areas"].as_array().unwrap();
        assert_eq!(areas.len(), 3, "遥控器原有的 1 个圆 + 本次 2 个圆");
        assert_eq!(areas[0]["x"], 1.0);
        assert_eq!(areas[1]["x"], 2.0);
        assert_eq!(areas[2]["y"], 3.0);
        assert_eq!(value["edit_ops"].as_array().unwrap().len(), 1);
        assert_eq!(value["fences"].as_array().unwrap().len(), 1);
        assert_eq!(value["extra"], "keep-me");
    }

    /// 同一笔反复下发不能重复累积（幂等）
    #[test]
    fn repeating_the_same_erase_does_not_duplicate_circles() {
        let zip = build_zip(&[
            ("map_edit/map.pgm", solid_pgm(8, 8, 254)),
            ("map_edit/map.yaml", b"image: map.pgm\n".to_vec()),
        ]);
        let strokes = erase_all_free(&[(2.0, 2.0)], 1.0, 254);
        let (once, _) = rewrite_map_package(&zip, &strokes, 254).unwrap();
        let (twice, _) = rewrite_map_package(&once, &strokes, 254).unwrap();

        let text = read_zip_text(&twice, "map_edit/edit_info.json").unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["erase_areas"].as_array().unwrap().len(), 1);
        assert_eq!(value["edit_ops"].as_array().unwrap().len(), 1);
    }

    /// 「擦成未知 128」遥控器表达不了（它的橡皮擦只会写 254），不登记
    #[test]
    fn erase_to_unknown_is_not_recorded_as_ops() {
        let zip = build_zip(&[
            ("map_edit/map.pgm", solid_pgm(8, 8, 254)),
            ("map_edit/map.yaml", b"image: map.pgm\n".to_vec()),
        ]);
        let strokes = erase_all_free(&[(2.0, 2.0)], 1.0, 128);
        let (out, _) = rewrite_map_package(&zip, &strokes, 128).unwrap();
        assert!(
            read_zip_text(&out, "map_edit/edit_info.json").is_none(),
            "不该凭空造编辑记录"
        );
    }

    /// 根目录栅格（没有 map_edit/）不许造 edit_info.json
    #[test]
    fn erase_on_root_raster_does_not_create_edit_info() {
        let zip = build_zip(&[
            ("map.pgm", solid_pgm(8, 8, 254)),
            ("map.yaml", b"image: map.pgm\n".to_vec()),
        ]);
        let strokes = erase_all_free(&[(2.0, 2.0)], 1.0, 254);
        let (out, _) = rewrite_map_package(&zip, &strokes, 254).unwrap();
        assert!(read_zip_text(&out, "map_edit/edit_info.json").is_none());
    }

    /// 遥控器的记录解析不了就原样保留，栅格照旧编辑（不能把别人的数据写坏）
    #[test]
    fn broken_edit_info_is_left_untouched() {
        let broken = b"{ this is not json".to_vec();
        let zip = build_zip(&[
            ("map_edit/map.pgm", solid_pgm(8, 8, 128)),
            ("map_edit/map.yaml", b"image: map.pgm\n".to_vec()),
            ("map_edit/edit_info.json", broken.clone()),
        ]);
        let strokes = erase_all_free(&[(2.0, 2.0)], 1.0, 254);
        let (out, changed) = rewrite_map_package(&zip, &strokes, 254).unwrap();
        assert!(changed > 0);
        let mut archive = zip::ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut file = archive.by_name("map_edit/edit_info.json").unwrap();
        let mut bytes = Vec::new();
        use std::io::Read;
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, broken);
    }

    // ---- 恢复原图（编辑版栅格换回建图原图 + 清空遥控器编辑记录）----

    fn read_zip_entry(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
        let mut file = archive.by_name(name).ok()?;
        let mut out = Vec::new();
        file.read_to_end(&mut out).ok()?;
        Some(out)
    }

    #[test]
    fn restore_original_swaps_raster_and_clears_edit_record() {
        let original = solid_pgm(8, 8, 255);
        let mut edited_pixels = vec![255u8; 64];
        edited_pixels[0] = 0;
        edited_pixels[1] = 254;
        let edited = [b"P5\n8 8\n255\n".to_vec(), edited_pixels].concat();
        let info = br#"{
  "erase_areas": [{"x": 2.0, "y": 2.0, "r": 8.0}],
  "forbidden_areas": [],
  "fences": [],
  "edit_ops": [{"type": "erase", "circles": [{"x": 2.0, "y": 2.0, "r": 8.0}]}]
}"#;
        let zip = build_zip(&[
            ("map.pgm", original.clone()),
            ("map.yaml", b"image: map.pgm\nresolution: 0.05\n".to_vec()),
            ("map_edit/map.pgm", edited),
            ("map_edit/map.yaml", b"image: map.pgm\n".to_vec()),
            ("map_edit/edit_info.json", info.to_vec()),
            ("nav_paths/rt-1", b"points".to_vec()),
        ]);

        let out = restore_original_package(&zip).unwrap();

        // 编辑版栅格 = 建图原图（按像素比，重新编码过 header）
        let restored = read_zip_entry(&out, "map_edit/map.pgm").unwrap();
        let raster = parse_pgm_raster(&restored).unwrap();
        assert_eq!(raster.pixels, parse_pgm_raster(&original).unwrap().pixels);
        // 遥控器的编辑记录被清空
        let info_text =
            String::from_utf8(read_zip_entry(&out, "map_edit/edit_info.json").unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&info_text).unwrap();
        assert!(value["erase_areas"].as_array().unwrap().is_empty());
        assert!(value["edit_ops"].as_array().unwrap().is_empty());
        assert!(value["forbidden_areas"].as_array().unwrap().is_empty());
        assert!(value["fences"].as_array().unwrap().is_empty());
        // 其它条目原样保留（根目录原图、路网、yaml）
        assert_eq!(
            read_zip_entry(&out, "map.pgm").unwrap(),
            parse_pgm_raster(&original).unwrap().to_p5()
        );
        assert_eq!(read_zip_entry(&out, "nav_paths/rt-1").unwrap(), b"points");
        assert_eq!(
            read_zip_entry(&out, "map_edit/map.yaml").unwrap(),
            b"image: map.pgm\n"
        );
    }

    #[test]
    fn restore_original_creates_empty_edit_info_when_missing() {
        let zip = build_zip(&[
            ("map.pgm", solid_pgm(8, 8, 255)),
            ("map_edit/map.pgm", solid_pgm(8, 8, 254)),
        ]);
        let out = restore_original_package(&zip).unwrap();
        let info = read_zip_entry(&out, "map_edit/edit_info.json").expect("要补一份空记录");
        let value: serde_json::Value = serde_json::from_slice(&info).unwrap();
        assert!(value["erase_areas"].as_array().unwrap().is_empty());
    }

    /// 手工上传的 ROS 包没有 `map_edit/`：本来就是原图，直接拒绝
    #[test]
    fn restore_original_rejects_package_without_edit_raster() {
        let zip = build_zip(&[
            ("model.pgm", solid_pgm(8, 8, 255)),
            ("model.yaml", b"image: model.pgm\n".to_vec()),
        ]);
        let error = restore_original_package(&zip).unwrap_err();
        assert!(format!("{error:?}").contains("没有编辑版"));
    }

    /// 只有 `model.pgm`（没有根 `map.pgm`）时也要能找到原图
    #[test]
    fn restore_original_falls_back_to_model_pgm() {
        let zip = build_zip(&[
            ("model.pgm", solid_pgm(8, 8, 255)),
            ("map_edit/map.pgm", solid_pgm(8, 8, 0)),
        ]);
        let out = restore_original_package(&zip).unwrap();
        let raster = parse_pgm_raster(&read_zip_entry(&out, "map_edit/map.pgm").unwrap()).unwrap();
        assert_eq!(raster.pixels, vec![255u8; 64]);
    }
}
