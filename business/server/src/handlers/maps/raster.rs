//! 地图栅格预览：读 ROS 的 `pgm`、降采样、编 PNG。

// 同一模块拆出来的几块共用一个词汇表（`MAP_SELECT` / helper / `AppState`），
// 逐个列名只会在每次加函数时漏一个编译错误。
#[allow(clippy::wildcard_imports)]
use super::*;

/// 渲染 backend 上传的 ROS PGM 栅格地图，返回 PNG data URL。
pub async fn map_raster(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<MapRaster>>, AppError> {
    let map = fetch_map(&state, &id).await?;
    let file_path = map
        .file_path
        .clone()
        .ok_or_else(|| AppError::BadRequest("该地图没有关联的 ROS 地图包".to_string()))?;
    let raster = read_ros_raster(&state.config.backend_db_path, &file_path).ok_or_else(|| {
        AppError::BadRequest("无法从地图包中解析栅格（map_edit/map.pgm 或 map.pgm）".to_string())
    })?;
    let raster = downscale_raster(raster, 1600);
    let png = encode_png(raster.width, raster.height, &raster.pixels)
        .ok_or_else(|| AppError::Internal("PNG 编码失败".to_string()))?;
    Ok(ApiResponse::ok(MapRaster {
        map_id: map.id,
        image: format!("data:image/png;base64,{}", BASE64.encode(png)),
        resolution: map.resolution,
        origin: map.origin,
        width: raster.width,
        height: raster.height,
    }))
}
pub(crate) struct RosRaster {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
}

/// 从根 backend 上传的 `.zip` 中读取栅格 PGM 并解码为 8 位灰度像素。
///
/// 优先 `map_edit/map.pgm`（遥控器编辑后的地图），没有编辑版时才用根目录
/// `model.pgm` / `map.pgm`（建图原始地图）。
pub(crate) fn read_ros_raster(backend_db: &std::path::Path, relative: &str) -> Option<RosRaster> {
    let upload_root = backend_db.parent()?.join("device_uploads");
    let path = upload_root.join(relative);
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut names: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if !entry.is_dir() {
            names.push(entry.name().to_string());
        }
    }
    let name = ingest::preferred_map_entry(names.iter().map(String::as_str), &[".pgm"])?;
    let mut entry = archive.by_name(name).ok()?;
    let mut data = Vec::new();
    entry.read_to_end(&mut data).ok()?;
    decode_pgm(&data)
}

pub(crate) fn decode_pgm(bytes: &[u8]) -> Option<RosRaster> {
    let (magic, width, height, maxval, data_start) = pgm_header(bytes)?;
    let count = (width as usize).checked_mul(height as usize)?;
    let pixels = match magic {
        "P5" if maxval <= 255 => {
            let end = data_start.checked_add(count)?;
            bytes.get(data_start..end)?.to_vec()
        }
        "P5" => {
            let end = data_start.checked_add(count.checked_mul(2)?)?;
            let data = bytes.get(data_start..end)?;
            (0..count)
                .map(|index| ((data[index * 2] as u16) << 8 | data[index * 2 + 1] as u16) >> 8)
                .map(|value| value as u8)
                .collect()
        }
        "P2" => {
            let text = std::str::from_utf8(bytes.get(data_start..)?).ok()?;
            let mut values = Vec::with_capacity(count);
            for token in text.split_ascii_whitespace() {
                if values.len() >= count {
                    break;
                }
                let value: u32 = token.parse().ok()?;
                values.push(if maxval <= 255 {
                    value as u8
                } else {
                    (value >> 8) as u8
                });
            }
            if values.len() < count {
                return None;
            }
            values
        }
        _ => return None,
    };
    Some(RosRaster {
        width,
        height,
        pixels,
    })
}

pub(crate) fn pgm_header(bytes: &[u8]) -> Option<(&str, u32, u32, u32, usize)> {
    let mut tokens: Vec<(usize, usize)> = Vec::new();
    let mut index = 0;
    while index < bytes.len() && tokens.len() < 4 {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            break;
        }
        if bytes[index] == b'#' {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        let start = index;
        while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        tokens.push((start, index));
    }
    if tokens.len() < 4 {
        return None;
    }
    let magic = std::str::from_utf8(&bytes[tokens[0].0..tokens[0].1]).ok()?;
    let width = std::str::from_utf8(&bytes[tokens[1].0..tokens[1].1])
        .ok()?
        .parse()
        .ok()?;
    let height = std::str::from_utf8(&bytes[tokens[2].0..tokens[2].1])
        .ok()?
        .parse()
        .ok()?;
    let maxval = std::str::from_utf8(&bytes[tokens[3].0..tokens[3].1])
        .ok()?
        .parse()
        .ok()?;
    let mut data_start = tokens[3].1;
    if data_start < bytes.len() && bytes[data_start] == b'\r' {
        data_start += 1;
    }
    if data_start < bytes.len() && bytes[data_start].is_ascii_whitespace() {
        data_start += 1;
    }
    Some((magic, width, height, maxval, data_start))
}

pub(crate) fn downscale_raster(raster: RosRaster, max_dimension: u32) -> RosRaster {
    let longest = raster.width.max(raster.height);
    if longest <= max_dimension || longest == 0 {
        return raster;
    }
    let scale = max_dimension as f64 / longest as f64;
    let width = ((raster.width as f64 * scale).round() as u32).max(1);
    let height = ((raster.height as f64 * scale).round() as u32).max(1);
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        let source_y = (y as f64 / scale) as u32;
        for x in 0..width {
            let source_x = (x as f64 / scale) as u32;
            let source_index = (source_y.min(raster.height - 1) * raster.width
                + source_x.min(raster.width - 1)) as usize;
            pixels.push(raster.pixels[source_index]);
        }
    }
    RosRaster {
        width,
        height,
        pixels,
    }
}

pub(crate) fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, width, height);
        encoder.set_color(png::ColorType::Grayscale);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(pixels).ok()?;
    }
    Some(output)
}
