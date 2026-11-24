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
            } else {
                std::io::copy(&mut entry, &mut writer)
                    .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
            }
        }
        writer
            .finish()
            .map_err(|error| AppError::InternalError(format!("写入地图包失败: {error}")))?;
    }

    Ok((buffer.into_inner(), erased_pixels))
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
}
