use crate::business::dog::map::domain::{
    CreateMapRequest, DogMap, MAP_STATUS_ARCHIVED, MAP_STATUS_DRAFT, MAP_STATUS_PARTIAL,
    MAP_STATUS_PUBLISHED, MapImageVO, MapPageQuery, MapVO, UpdateMapRequest, UploadMapRequest,
    is_valid_map_status,
};
use crate::business::dog::map::package::{parse_map_package, sha256_hex};
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::{format_datetime, generate_id};

pub struct DogMapService {
    repo: SeaOrmDogMapRepository,
}

impl DogMapService {
    pub fn new(repo: SeaOrmDogMapRepository) -> Self {
        Self { repo }
    }

    pub async fn create(
        &self,
        req: CreateMapRequest,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        if let Some(status) = req.map_status.as_deref()
            && !is_valid_map_status(status)
        {
            return Err(AppError::BadRequest(format!("无效的地图状态: {status}")));
        }
        if matches!(
            req.map_status.as_deref(),
            Some(MAP_STATUS_PUBLISHED) | Some(MAP_STATUS_ARCHIVED)
        ) {
            return Err(AppError::BadRequest(
                "新建地图只能保存为草稿或部分标定，发布请使用「发布基线」".to_string(),
            ));
        }
        let id = generate_id();
        self.repo
            .create(&req, id, scope)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn get_by_id(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    /// 转成 VO，供单条详情 / 新增 / 修改接口返回。
    pub fn to_vo(&self, map: DogMap) -> MapVO {
        MapVO::from(map)
    }

    pub async fn list_page(
        &self,
        query: MapPageQuery,
        scope: &DataScope,
    ) -> Result<PageResponse<MapVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query, scope)
            .await
            .map_err(AppError::DatabaseError)?;

        let vo_list: Vec<MapVO> = records.into_iter().map(MapVO::from).collect();

        Ok(PageResponse::new(
            vo_list,
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self, scope: &DataScope) -> Result<Vec<DogMap>, AppError> {
        self.repo
            .find_all(scope)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateMapRequest,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        let existing = self.get_by_id(id, scope).await?;
        if let Some(status) = req.map_status.as_deref()
            && !is_valid_map_status(status)
        {
            return Err(AppError::BadRequest(format!("无效的地图状态: {status}")));
        }
        if let Some(status) = req.map_status.as_deref()
            && matches!(status, MAP_STATUS_PUBLISHED | MAP_STATUS_ARCHIVED)
            && status != existing.map_status
        {
            return Err(AppError::BadRequest(
                "已发布 / 已归档状态由发布流程维护，不可直接修改".to_string(),
            ));
        }
        // 已发布的版本是不可变坐标系：坐标定义字段不可再改，只能重新建图发版。
        if existing.map_status != MAP_STATUS_DRAFT
            && (req.map_version.is_some()
                || req.frame_id.is_some()
                || req.map_origin.is_some()
                || req.resolution.is_some()
                || req.file_name.is_some()
                || req.file_path.is_some())
        {
            return Err(AppError::BadRequest(
                "已发布的地图版本不可修改版本号 / frame_id / 原点 / 分辨率 / 地图包，请重新建图并发布新版本"
                    .to_string(),
            ));
        }
        self.repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseError)?;
        if !deleted {
            return Err(AppError::NotFound(format!("Map with id {} not found", id)));
        }
        Ok(())
    }

    /// 发布地图版本：把状态推进到「已发布」/「部分标定」并记录发布时间。
    ///
    /// 已发布版本视为不可变；归档由发布新版本或人工操作完成。
    pub async fn publish(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        let map = self.get_by_id(id, scope).await?;
        if map.map_status == MAP_STATUS_ARCHIVED {
            return Err(AppError::BadRequest(
                "已归档的地图版本不可再次发布，请创建新版本".to_string(),
            ));
        }
        if !is_valid_map_status(&map.map_status) {
            return Err(AppError::BadRequest(format!(
                "无效的地图状态: {}",
                map.map_status
            )));
        }

        let status = if map.map_status == MAP_STATUS_PARTIAL {
            MAP_STATUS_PARTIAL
        } else {
            MAP_STATUS_PUBLISHED
        };
        self.repo
            .update_status(id, status, Some(format_datetime(chrono::Utc::now())), scope)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    /// 基于当前版本派生下一个不可变版本并立即发布。
    pub async fn new_version(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        let map = self.get_by_id(id, scope).await?;
        let next_version = bump_minor_version(map.map_version.as_deref());
        let req = CreateMapRequest {
            map_name: map.map_name.clone(),
            map_type: map.map_type.clone(),
            // 派生新版本时保留园区归属
            park_code: map.park_code.clone(),
            park_name: map.park_name.clone(),
            map_data: map.map_data.clone(),
            file_name: map.file_name.clone(),
            file_path: map.file_path.clone(),
            file_format: map.file_format.clone(),
            file_size: map.file_size,
            remarks: map.remarks.clone(),
            map_version: Some(next_version),
            frame_id: map.frame_id.clone(),
            crs: map.crs.clone(),
            map_origin: map.map_origin.clone(),
            resolution: map.resolution.clone(),
            floors: map.floors.clone(),
            road_count: map.road_count,
            nogo_count: map.nogo_count,
            map_status: Some(MAP_STATUS_DRAFT.to_string()),
            map_hash: map.map_hash.clone(),
            map_width: map.map_width,
            map_height: map.map_height,
        };
        let new_id = generate_id();
        self.repo
            .create(&req, new_id, scope)
            .await
            .map_err(AppError::DatabaseError)?;
        self.publish(new_id, scope).await
    }

    /// Stores an uploaded `.zip` map package and creates/updates the map row.
    pub async fn save_upload(
        &self,
        info: UploadMapRequest,
        original_name: &str,
        bytes: &[u8],
        upload_root: &std::path::Path,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        let file_name = std::path::Path::new(original_name)
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "map.zip".to_string());
        let file_format = std::path::Path::new(&file_name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_lowercase();
        if file_format != "zip" {
            return Err(AppError::BadRequest("地图包仅支持 .zip 格式".to_string()));
        }
        if info.map_name.trim().is_empty() {
            return Err(AppError::BadRequest("地图名称不能为空".to_string()));
        }

        // 以地图包内容为准解析坐标系与哈希；前端手填值只用于校验，不覆盖包内数据。
        let parsed = parse_map_package(bytes)?;
        validate_package_metadata(&info, &parsed)?;
        let canonical_resolution = format_number(parsed.resolution);
        let canonical_origin = parsed
            .origin
            .iter()
            .map(|value| format_number(*value))
            .collect::<Vec<_>>()
            .join(", ");
        let map_hash = parsed.sha256.clone();
        let map_width = i32::try_from(parsed.width).ok();
        let map_height = i32::try_from(parsed.height).ok();

        let dir = upload_root.join("maps");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| AppError::InternalError(format!("创建地图目录失败: {error}")))?;
        let stored_name = format!("{}-{}", generate_id(), file_name);
        tokio::fs::write(dir.join(&stored_name), bytes)
            .await
            .map_err(|error| AppError::InternalError(format!("保存地图包失败: {error}")))?;

        let file_path = format!("maps/{stored_name}");
        let file_size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);

        match info.id {
            Some(id) => {
                let req = UpdateMapRequest {
                    map_name: Some(info.map_name),
                    map_type: info.map_type,
                    park_code: info.park_code,
                    park_name: info.park_name,
                    map_data: None,
                    file_name: Some(file_name),
                    file_path: Some(file_path),
                    file_format: Some(file_format),
                    file_size: Some(file_size),
                    remarks: info.remarks,
                    map_version: info.map_version,
                    frame_id: info.frame_id,
                    crs: info.crs,
                    map_origin: Some(canonical_origin),
                    resolution: Some(canonical_resolution),
                    floors: info.floors,
                    road_count: info.road_count,
                    nogo_count: info.nogo_count,
                    map_status: info.map_status,
                    map_hash: Some(map_hash),
                    map_width,
                    map_height,
                };
                self.update(id, req, scope).await
            }
            None => {
                let req = CreateMapRequest {
                    map_name: info.map_name,
                    map_type: info.map_type,
                    park_code: info.park_code,
                    park_name: info.park_name,
                    map_data: None,
                    file_name: Some(file_name),
                    file_path: Some(file_path),
                    file_format: Some(file_format),
                    file_size: Some(file_size),
                    remarks: info.remarks,
                    map_version: info.map_version,
                    frame_id: info.frame_id,
                    crs: info.crs,
                    map_origin: Some(canonical_origin),
                    resolution: Some(canonical_resolution),
                    floors: info.floors,
                    road_count: info.road_count,
                    nogo_count: info.nogo_count,
                    map_status: info.map_status,
                    map_hash: Some(map_hash),
                    map_width,
                    map_height,
                };
                self.create(req, scope).await
            }
        }
    }

    /// 读取地图包并转成前端可直接绘制的 PNG（base64），附带栅格分辨率与原点。
    pub async fn load_image(
        &self,
        id: i64,
        upload_root: &std::path::Path,
        scope: &DataScope,
    ) -> Result<MapImageVO, AppError> {
        let map = self.get_by_id(id, scope).await?;
        let file_path = map
            .file_path
            .clone()
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "地图 {} 尚未上传地图包，无法读取栅格图",
                    map.map_name
                ))
            })?;

        let bytes = tokio::fs::read(upload_root.join(&file_path))
            .await
            .map_err(|error| AppError::InternalError(format!("读取地图包失败: {error}")))?;

        if let Some(expected) = map
            .map_hash
            .as_deref()
            .filter(|hash| !hash.trim().is_empty())
        {
            let actual = sha256_hex(&bytes);
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(AppError::BadRequest(format!(
                    "地图包内容与发布时的 map_hash 不一致（期望 {}，实际 {}），请重新发布基线",
                    expected, actual
                )));
            }
        }

        let parsed = parse_map_package(&bytes)?;

        Ok(MapImageVO {
            image: parsed.png_data_url(),
            resolution: parsed.resolution,
            origin: parsed.origin,
            width: parsed.width,
            height: parsed.height,
        })
    }
}

/// 校验前端手填的 resolution / origin 是否与地图包内容一致。
fn validate_package_metadata(
    info: &UploadMapRequest,
    parsed: &crate::business::dog::map::package::ParsedMapPackage,
) -> Result<(), AppError> {
    if let Some(requested) = info
        .resolution
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let requested = first_number(requested).ok_or_else(|| {
            AppError::BadRequest(format!("无法解析分辨率「{requested}」，请留空或填写数字"))
        })?;
        if (requested - parsed.resolution).abs() > 1e-6 {
            return Err(AppError::BadRequest(format!(
                "分辨率与地图包不一致：表单 {}，地图包 {}。请留空由服务端解析",
                requested, parsed.resolution
            )));
        }
    }

    if let Some(requested) = info
        .map_origin
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let values: Vec<f64> = requested
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| {
                part.parse::<f64>().map_err(|_| {
                    AppError::BadRequest(format!(
                        "无法解析原点「{requested}」，请按 x, y, yaw 填写或留空"
                    ))
                })
            })
            .collect::<Result<_, _>>()?;
        if values.len() > parsed.origin.len() {
            return Err(AppError::BadRequest(format!(
                "原点维度与地图包不一致：表单 {} 个值，地图包 {} 个值",
                values.len(),
                parsed.origin.len()
            )));
        }
        for (index, value) in values.iter().enumerate() {
            if (value - parsed.origin[index]).abs() > 1e-6 {
                return Err(AppError::BadRequest(format!(
                    "原点与地图包不一致：表单第 {} 个值 {}，地图包 {}。请留空由服务端解析",
                    index + 1,
                    value,
                    parsed.origin[index]
                )));
            }
        }
    }

    Ok(())
}

fn first_number(value: &str) -> Option<f64> {
    value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|part| !part.is_empty())
        .find_map(|part| part.parse::<f64>().ok())
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}

/// v1.2 → v1.3；无版本号时从 v1.0 开始。
fn bump_minor_version(current: Option<&str>) -> String {
    let numbers: Vec<u32> = current
        .unwrap_or_default()
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u32>().ok())
        .collect();
    let major = numbers.first().copied().unwrap_or(1);
    let mut minor = numbers.get(1).copied().unwrap_or(0) + 1;
    let mut major = major;
    if minor > 9 {
        major += 1;
        minor = 0;
    }
    format!("v{major}.{minor}")
}
