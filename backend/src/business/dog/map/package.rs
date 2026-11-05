//! 地图包（`.zip`）解析：pgm → png，并读取 `model.yaml` 的分辨率与原点。
//!
//! 与参考实现（`DronesRouteLibraryServiceImp#getRoute`）保持一致：
//! 地图包解压后包含 ROS map_server 格式的 `model.pgm` 与 `model.yaml`，
//! `yaml` 里的 `resolution` 与 `origin` 用于前端把世界坐标换算成像素坐标。

use std::io::Cursor;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use crate::common::error::AppError;

/// 地图包解析结果（未编码的中间结构）
#[derive(Debug, Clone)]
pub struct ParsedMapPackage {
    pub png: Vec<u8>,
    pub resolution: f64,
    pub origin: Vec<f64>,
    pub width: u32,
    pub height: u32,
}

impl ParsedMapPackage {
    /// 转成 `data:image/png;base64,...` 形式，便于前端直接放进 `<img src>`
    pub fn png_data_url(&self) -> String {
        format!("data:image/png;base64,{}", BASE64.encode(&self.png))
    }
}

const DEFAULT_RESOLUTION: f64 = 0.05;
const DEFAULT_ORIGIN: [f64; 3] = [0.0, 0.0, 0.0];

/// 解析地图包 zip：找出 `*.pgm` 与 `*.yaml`，把栅格地图转成 PNG。
///
/// 兼容包内多层目录（例如 `map/model.pgm`），先找 `model.pgm`，找不到时退回
/// 第一个 `*.pgm`。
pub fn parse_map_package(bytes: &[u8]) -> Result<ParsedMapPackage, AppError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| AppError::BadRequest(format!("地图包解析失败: {error}")))?;

    let pgm_name = find_entry(&mut archive, "model.pgm", ".pgm");
    let pgm_name = pgm_name.ok_or_else(|| {
        AppError::BadRequest("地图包内未找到 model.pgm，请检查地图包内容".to_string())
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
    })
}

/// 按 `preferred`（精确文件名）→ `extension`（后缀）顺序查找包内条目。
fn find_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    preferred: &str,
    extension: &str,
) -> Option<String> {
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
        if lower.ends_with(&format!("/{preferred}")) || lower == preferred {
            return Some(name);
        }
        if fallback.is_none() && lower.ends_with(extension) {
            fallback = Some(name);
        }
    }
    fallback
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
    if values.is_empty() { None } else { Some(values) }
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
        let zip = build_zip(&[
            ("model.pgm", sample_pgm()),
            ("model.yaml", yaml.to_vec()),
        ]);

        let parsed = parse_map_package(&zip).unwrap();
        assert_eq!(parsed.width, 4);
        assert_eq!(parsed.height, 3);
        assert!((parsed.resolution - 0.05).abs() < f64::EPSILON);
        assert_eq!(parsed.origin, vec![-8.7, -17.7, 0.0]);
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
    fn rejects_package_without_pgm() {
        let zip = build_zip(&[("readme.txt", b"nothing here".to_vec())]);
        assert!(parse_map_package(&zip).is_err());
    }
}
