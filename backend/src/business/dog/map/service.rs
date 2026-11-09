use crate::business::dog::map::domain::{
    CreateMapRequest, DogMap, MAP_STATUS_ARCHIVED, MAP_STATUS_DRAFT, MAP_STATUS_PARTIAL,
    MAP_STATUS_PUBLISHED, MapImageVO, MapPageQuery, MapRefsVO, MapRouteRefVO, MapVO,
    UpdateMapRequest, UploadMapRequest, is_valid_map_status,
};
use crate::business::dog::map::package::{parse_map_package, sha256_hex};
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::business::dog::park::repository::SeaOrmDogParkRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::{format_datetime, generate_id};

pub struct DogMapService {
    repo: SeaOrmDogMapRepository,
    park_repo: SeaOrmDogParkRepository,
}

impl DogMapService {
    pub fn new(repo: SeaOrmDogMapRepository, park_repo: SeaOrmDogParkRepository) -> Self {
        Self { repo, park_repo }
    }

    pub async fn create(
        &self,
        req: CreateMapRequest,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        // 地图必须归属某个园区：新增时园区为必填项。
        let park_id = req
            .park_id
            .ok_or_else(|| AppError::BadRequest("请选择所属园区".to_string()))?;
        self.ensure_park_access(park_id, scope).await?;
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
        let map = self
            .repo
            .create(&req, id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        // 首次建图时把园区从「仅台账」推进到「待标定」，发布后才成为可用基线。
        let park = self
            .park_repo
            .find_by_id(park_id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if let Some(park) = park
            && park.map_id.is_none()
            && park.map_status == "未绑定"
        {
            self.park_repo
                .set_baseline(park_id, None, "待标定")
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;
        }

        Ok(map)
    }

    pub async fn get_by_id(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    /// 转成 VO 并补齐「所属园区」名称，供单条详情 / 新增 / 修改接口返回。
    pub async fn to_vo(&self, map: DogMap, scope: &DataScope) -> Result<MapVO, AppError> {
        let mut vo = MapVO::from(map);
        if let Some(park_id) = vo.park_id {
            let park = self
                .park_repo
                .find_by_id(park_id, scope)
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;
            vo.park_name = park.map(|p| p.park_name);
        }
        Ok(vo)
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
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let mut park_ids: Vec<i64> = records.iter().filter_map(|m| m.park_id).collect();
        park_ids.sort_unstable();
        park_ids.dedup();
        let park_names = self
            .repo
            .find_park_names(&park_ids)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let vo_list: Vec<MapVO> = records
            .into_iter()
            .map(|map| {
                let mut vo = MapVO::from(map);
                vo.park_name = vo.park_id.and_then(|id| park_names.get(&id).cloned());
                vo
            })
            .collect();

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
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    /// 园区可用的地图基线版本。`publishable_only` 为 true 时不含草稿 / 已归档，
    /// 用于园区徽章与新建路线的版本下拉。
    pub async fn list_by_park(
        &self,
        park_id: i64,
        publishable_only: bool,
        scope: &DataScope,
    ) -> Result<Vec<MapVO>, AppError> {
        self.ensure_park_access(park_id, scope).await?;
        let maps = self
            .repo
            .find_by_park(park_id, scope, publishable_only)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        let park_name = self
            .park_repo
            .find_by_id(park_id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .map(|p| p.park_name);
        Ok(maps
            .into_iter()
            .map(|map| {
                let mut vo = MapVO::from(map);
                vo.park_name = park_name.clone();
                vo
            })
            .collect())
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateMapRequest,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        let existing = self.get_by_id(id, scope).await?;
        if let Some(park_id) = req.park_id {
            self.ensure_park_access(park_id, scope).await?;
        }
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
                || req.file_path.is_some()
                || (req.park_id.is_some() && req.park_id != existing.park_id))
        {
            return Err(AppError::BadRequest(
                "已发布的地图版本不可修改版本号 / frame_id / 原点 / 分辨率 / 地图包 / 所属园区，请重新建图并发布新版本"
                    .to_string(),
            ));
        }
        self.repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let refs = self.refs(id, scope).await?;
        if !refs.routes.is_empty() {
            return Err(AppError::BadRequest(format!(
                "地图 {} 仍被 {} 条路线引用，需先迁移或删除这些路线",
                refs.map_id,
                refs.routes.len()
            )));
        }
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!("Map with id {} not found", id)));
        }
        self.park_repo
            .clear_baseline_for_map(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(())
    }

    /// 发布地图版本：归档同园区旧版本，把园区指针切到该版本。
    ///
    /// 这一步对应原型里的「发布不可变 MapVersion」。正式系统应由地图管理台
    /// 完成，这里提供受控接口，供建图完成后人工确认发布。
    pub async fn publish(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        let map = self.get_by_id(id, scope).await?;
        let park_id = map
            .park_id
            .ok_or_else(|| AppError::BadRequest("地图必须归属园区才能发布基线".to_string()))?;
        self.ensure_park_access(park_id, scope).await?;
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

        self.repo
            .archive_published_in_park(park_id, id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let status = if map.map_status == MAP_STATUS_PARTIAL {
            MAP_STATUS_PARTIAL
        } else {
            MAP_STATUS_PUBLISHED
        };
        let published = self
            .repo
            .update_status(id, status, Some(format_datetime(chrono::Utc::now())), scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))?;

        let park_status = if status == MAP_STATUS_PARTIAL {
            "部分标定"
        } else {
            "已标定"
        };
        self.park_repo
            .set_baseline(park_id, Some(id), park_status)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(published)
    }

    /// 基于当前版本派生下一个不可变版本并立即发布。
    /// 历史路线仍冻结在旧版本，不会被自动迁移。
    pub async fn new_version(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        let map = self.get_by_id(id, scope).await?;
        let park_id = map
            .park_id
            .ok_or_else(|| AppError::BadRequest("地图必须归属园区才能创建新版本".to_string()))?;
        self.ensure_park_access(park_id, scope).await?;

        let next_version = bump_minor_version(map.map_version.as_deref());
        let req = CreateMapRequest {
            map_name: map.map_name.clone(),
            map_type: map.map_type.clone(),
            map_data: map.map_data.clone(),
            park_id: Some(park_id),
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
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        self.publish(new_id, scope).await
    }

    /// 引用该版本的路线 / 任务清单，供「发布新版本」前做影响分析。
    pub async fn refs(&self, id: i64, scope: &DataScope) -> Result<MapRefsVO, AppError> {
        self.get_by_id(id, scope).await?;
        let routes = self
            .repo
            .find_routes_by_map(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        let route_ids: Vec<i64> = routes.iter().map(|r| r.id).collect();
        let task_count = self
            .repo
            .count_tasks_for_routes(&route_ids)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        let route_refs = routes
            .into_iter()
            .map(|route| MapRouteRefVO {
                id: route.id,
                route_name: route.route_name,
                route_status: route.route_status,
                route_version: None,
                waypoint_count: count_waypoints(route.route_data.as_deref()),
                frozen: true,
            })
            .collect();
        Ok(MapRefsVO {
            map_id: id,
            routes: route_refs,
            task_count,
        })
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

        // 上传前先校验园区：新增必须指定园区，且园区要可访问。
        match info.park_id {
            Some(park_id) => self.ensure_park_access(park_id, scope).await?,
            None if info.id.is_none() => {
                return Err(AppError::BadRequest("请选择所属园区".to_string()));
            }
            None => {}
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
                    map_data: None,
                    park_id: info.park_id,
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
                    map_data: None,
                    park_id: info.park_id,
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
    ///
    /// 与参考实现的 `/dronesRouteLibrary/getRoute/{modelId}` 对应：
    /// 解压`model.pgm` → 转 PNG → base64，并解析 `model.yaml` 的
    /// `resolution` / `origin`。
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
                    "地图 {} 尚未上传地图包，无法用于路线规划",
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

    async fn ensure_park_access(&self, park_id: i64, scope: &DataScope) -> Result<(), AppError> {
        let park = self
            .park_repo
            .find_by_id(park_id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if park.is_none() {
            return Err(AppError::BadRequest(format!(
                "Park with id {} not found or not accessible",
                park_id
            )));
        }
        Ok(())
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

fn count_waypoints(route_data: Option<&str>) -> usize {
    route_data
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value.as_array().map(Vec::len))
        .unwrap_or(0)
}
