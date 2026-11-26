use crate::business::dog::map::domain::{
    CreateMapRequest, DogMap, MAP_STATUS_ARCHIVED, MAP_STATUS_DRAFT, MAP_STATUS_PARTIAL,
    MAP_STATUS_PUBLISHED, MapEditRequest, MapEditStroke, MapImageVO, MapPageQuery, MapShareResult,
    MapShareVO, MapVO, ShareMapRequest, UpdateMapRequest, UploadMapRequest, is_valid_map_status,
};
use crate::business::dog::map::package::{
    MAX_EDIT_POINTS, MAX_EDIT_STROKES, ParsedMapPackage, normalize_erase_value, parse_map_package,
    restore_original_package, rewrite_map_package, sha256_hex,
};
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
            .find_visible_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    /// 只认「自己的地图」：改 / 删 / 发版 / 擦除 / 覆盖地图包 / 管理分享都走这里。
    async fn get_owned_by_id(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    /// 转成 VO（带分享设备清单），供单条详情 / 新增 / 修改接口返回。
    pub async fn to_vo(&self, map: DogMap) -> Result<MapVO, AppError> {
        self.to_vos(vec![map]).await.map(|mut vos| vos.remove(0))
    }

    /// 批量转 VO：一次查出所有分享关系，列表页不会 N+1。
    async fn to_vos(&self, maps: Vec<DogMap>) -> Result<Vec<MapVO>, AppError> {
        let ids: Vec<i64> = maps.iter().map(|map| map.id).collect();
        let mut shares = self
            .repo
            .find_share_codes_for_maps(&ids)
            .await
            .map_err(AppError::DatabaseError)?;
        Ok(maps
            .into_iter()
            .map(|map| {
                let mut vo = MapVO::from(map);
                vo.shared_device_codes = shares.remove(&vo.id).unwrap_or_default();
                vo
            })
            .collect())
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

        let vo_list = self.to_vos(records).await?;

        Ok(PageResponse::new(
            vo_list,
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self, scope: &DataScope) -> Result<Vec<MapVO>, AppError> {
        let maps = self
            .repo
            .find_all(scope)
            .await
            .map_err(AppError::DatabaseError)?;
        self.to_vos(maps).await
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateMapRequest,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        let existing = self.get_owned_by_id(id, scope).await?;
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

    /// 返回给定 id 中仍然存在（未被软删除）的地图 id。
    ///
    /// 服务令牌身份调用，不做数据范围过滤：调用方（业务平台）只需要
    /// 「中台还认不认这张图」，看不到任何地图字段。
    pub async fn alive_ids(&self, ids: &[i64]) -> Result<Vec<i64>, AppError> {
        self.repo
            .find_alive_ids(ids)
            .await
            .map_err(AppError::DatabaseError)
    }

    /// 发布地图版本：把状态推进到「已发布」并记录发布时间。
    ///
    /// 「部分标定」只是**入库时的标定完整度**，发布是人工确认「这张图可以当基线用」，
    /// 所以草稿 / 部分标定发布后都是「已发布」（2026-11-24 起；以前部分标定发布会
    /// 保留部分标定，现场点了发布看到页面还是部分标定，以为发布没生效）。
    /// 完整度信息仍保留在 `publish_time` 与发布前的历史状态里，
    /// 需要重新标定就「新版本」派生。
    /// 已发布版本视为不可变；归档由发布新版本或人工操作完成。
    pub async fn publish(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        let map = self.get_owned_by_id(id, scope).await?;
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
            .update_status(
                id,
                MAP_STATUS_PUBLISHED,
                Some(format_datetime(chrono::Utc::now())),
                scope,
            )
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    /// 基于当前版本派生下一个不可变版本并立即发布。
    pub async fn new_version(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        let map = self.get_owned_by_id(id, scope).await?;
        // 派生版本要继承分享设备：否则发一次新版，被分享的设备立刻看不到图了
        let shared_device_codes = self.share_codes(id).await?;
        let next_version = bump_minor_version(map.map_version.as_deref());
        let req = CreateMapRequest {
            map_name: map.map_name.clone(),
            map_type: map.map_type.clone(),
            // 派生新版本时保留园区归属与上报设备
            park_code: map.park_code.clone(),
            park_name: map.park_name.clone(),
            device_code: map.device_code.clone(),
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
        for device_code in shared_device_codes {
            self.repo
                .add_share(new_id, &device_code, scope.user_id())
                .await
                .map_err(AppError::DatabaseError)?;
        }
        self.publish(new_id, scope).await
    }

    /// 把地图分享给其他设备使用（不复制地图包，也不重新建图）。
    ///
    /// 只有地图归属人（或管理员）能分享；上报设备本身不算分享，会被忽略。
    pub async fn share(
        &self,
        id: i64,
        req: ShareMapRequest,
        scope: &DataScope,
    ) -> Result<MapShareResult, AppError> {
        let map = self.get_owned_by_id(id, scope).await?;
        let replace = req
            .mode
            .as_deref()
            .map(|mode| mode.trim().eq_ignore_ascii_case("replace"))
            .unwrap_or(false);

        let mut codes: Vec<String> = Vec::new();
        for code in &req.device_codes {
            let code = code.trim();
            if code.is_empty() || Some(code) == map.device_code.as_deref() {
                continue;
            }
            if !codes.iter().any(|kept| kept == code) {
                codes.push(code.to_string());
            }
        }
        if !replace && codes.is_empty() {
            return Err(AppError::BadRequest(
                "请选择要分享的设备（上报设备本身就是可用设备，无需分享）".to_string(),
            ));
        }

        let mut removed = 0;
        if replace {
            removed = self
                .repo
                .remove_shares_not_in(id, &codes)
                .await
                .map_err(AppError::DatabaseError)?;
        }
        let mut added = 0;
        for code in &codes {
            if self
                .repo
                .add_share(id, code, scope.user_id())
                .await
                .map_err(AppError::DatabaseError)?
            {
                added += 1;
            }
        }

        let shared_device_codes = self.share_codes(id).await?;
        Ok(MapShareResult {
            map_id: id,
            shared_device_codes,
            added,
            removed,
        })
    }

    /// 取消分享（幂等：本来没分享也返回成功）。
    pub async fn unshare(
        &self,
        id: i64,
        device_code: &str,
        scope: &DataScope,
    ) -> Result<MapShareResult, AppError> {
        self.get_owned_by_id(id, scope).await?;
        let device_code = device_code.trim();
        if device_code.is_empty() {
            return Err(AppError::BadRequest("缺少设备编号".to_string()));
        }
        let removed = self
            .repo
            .remove_share(id, device_code)
            .await
            .map_err(AppError::DatabaseError)?;
        let shared_device_codes = self.share_codes(id).await?;
        Ok(MapShareResult {
            map_id: id,
            shared_device_codes,
            added: 0,
            removed,
        })
    }

    /// 当前分享给了哪些设备。
    pub async fn list_shares(
        &self,
        id: i64,
        scope: &DataScope,
    ) -> Result<Vec<MapShareVO>, AppError> {
        // 读权限：自己的地图，或分享给了自己管辖设备的地图
        self.get_by_id(id, scope).await?;
        Ok(self
            .repo
            .find_shares(id)
            .await
            .map_err(AppError::DatabaseError)?
            .into_iter()
            .map(|share| MapShareVO {
                map_id: share.map_id,
                device_code: share.device_code,
                create_time: format_datetime(share.create_time),
            })
            .collect())
    }

    /// 分享设备清单（下发地图的目标候选）。
    pub async fn share_codes(&self, id: i64) -> Result<Vec<String>, AppError> {
        self.repo
            .find_share_codes(id)
            .await
            .map_err(AppError::DatabaseError)
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

        // 业务平台以服务账号回传设备地图时，用 ownerUserId 把地图归属到设备所有人；
        // 普通用户不能改归属，始终记在自己名下。
        let owner_scope = info
            .owner_user_id
            .filter(|owner| *owner > 0 && scope.is_admin())
            .map(|owner| DataScope::new(owner, false));
        let write_scope = owner_scope.as_ref().unwrap_or(scope);

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

        // 原地覆盖（业务平台「同步地图」刷新）时先校验状态，再落盘：
        // 已发布 / 已归档不可改包，与橡皮擦同一套规则（草稿 + 部分标定可编辑）。
        if let Some(id) = info.id {
            let existing = self.get_owned_by_id(id, write_scope).await?;
            if !matches!(
                existing.map_status.as_str(),
                MAP_STATUS_DRAFT | MAP_STATUS_PARTIAL
            ) {
                return Err(AppError::BadRequest(format!(
                    "地图「{}」当前状态为 {}，不可原地覆盖地图包；请先派生新版本（新版本为草稿）再刷新",
                    existing.map_name, existing.map_status
                )));
            }
        }

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
                    device_code: info.device_code,
                    map_data: None,
                    file_name: Some(file_name),
                    file_path: Some(file_path),
                    file_format: Some(file_format),
                    file_size: Some(file_size),
                    remarks: info.remarks,
                    // 版本号 / 状态由发布流程维护：刷新只换栅格内容，
                    // 否则一张 v1.1 的图会被业务平台回传的 v1.0 打回旧版本号。
                    map_version: None,
                    frame_id: info.frame_id,
                    crs: info.crs,
                    map_origin: Some(canonical_origin),
                    resolution: Some(canonical_resolution),
                    floors: info.floors,
                    road_count: info.road_count,
                    nogo_count: info.nogo_count,
                    map_status: None,
                    map_hash: Some(map_hash),
                    map_width,
                    map_height,
                };
                // 走 repo 而不是 `update()`：`update()` 会拦住「已发布/部分标定不可改包」，
                // 而刷新语义正是只替换栅格内容（状态阈值已在上面校验过）。
                self.repo
                    .update(id, &req, write_scope)
                    .await
                    .map_err(AppError::DatabaseError)?
                    .ok_or_else(|| AppError::NotFound(format!("Map with id {id} not found")))
            }
            None => {
                let req = CreateMapRequest {
                    map_name: info.map_name,
                    map_type: info.map_type,
                    park_code: info.park_code,
                    park_name: info.park_name,
                    device_code: info.device_code,
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
                self.create(req, write_scope).await
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
        let bytes = self.read_package(&map, upload_root).await?;
        let parsed = parse_map_package(&bytes)?;

        Ok(MapImageVO {
            image: parsed.png_data_url(),
            resolution: parsed.resolution,
            origin: parsed.origin,
            width: parsed.width,
            height: parsed.height,
            map_hash: None,
            erased_pixels: None,
        })
    }

    /// 橡皮擦：擦除栅格噪点并落盘为新的地图包。
    ///
    /// - 只允许「草稿 / 部分标定」版本编辑，已发布 / 已归档版本必须派生新版本；
    /// - 优先改写 `map_edit/map.pgm`（遥控器编辑版），根目录原始 `map.pgm` 原样保留；
    ///   包里没有 `map_edit/` 时才退回旧的 `model.pgm` / `map.pgm`；
    /// - 其余条目（yaml 等）原样保留，`map_hash` 按新包重算；
    /// - 新包写新文件，旧包留在磁盘上便于回溯（与 `upload` 覆盖旧包的行为一致）。
    pub async fn erase(
        &self,
        id: i64,
        req: MapEditRequest,
        upload_root: &std::path::Path,
        scope: &DataScope,
    ) -> Result<MapImageVO, AppError> {
        let strokes = validate_edit_request(&req)?;
        let erase_value = req.erase_value.unwrap_or(254);

        let map = self.get_owned_by_id(id, scope).await?;
        if !matches!(
            map.map_status.as_str(),
            MAP_STATUS_DRAFT | MAP_STATUS_PARTIAL
        ) {
            return Err(AppError::BadRequest(format!(
                "地图「{}」当前状态为 {}，不可直接编辑；请先派生新版本（新版本为草稿）再擦除噪点",
                map.map_name, map.map_status
            )));
        }

        let bytes = self.read_package(&map, upload_root).await?;
        let (edited, erased_pixels) = rewrite_map_package(&bytes, strokes, erase_value)?;
        let parsed = parse_map_package(&edited)?;

        self.store_edited_package(id, &map, &edited, &parsed, upload_root, scope)
            .await?;

        Ok(MapImageVO {
            image: parsed.png_data_url(),
            resolution: parsed.resolution,
            origin: parsed.origin,
            width: parsed.width,
            height: parsed.height,
            map_hash: Some(parsed.sha256),
            erased_pixels: Some(erased_pixels),
        })
    }

    /// 恢复原图：把编辑版栅格换回建图原图，并清空遥控器的编辑记录。
    ///
    /// 和橡皮擦同一套状态约束（只有 `草稿` / `部分标定` 可改）：已发布 / 已归档
    /// 版本请先 `newVersion` 派生。恢复后需要再「下发到狗体」才会写回本体。
    pub async fn restore_original(
        &self,
        id: i64,
        upload_root: &std::path::Path,
        scope: &DataScope,
    ) -> Result<MapImageVO, AppError> {
        let map = self.get_owned_by_id(id, scope).await?;
        if !matches!(
            map.map_status.as_str(),
            MAP_STATUS_DRAFT | MAP_STATUS_PARTIAL
        ) {
            return Err(AppError::BadRequest(format!(
                "地图「{}」当前状态为 {}，不可直接编辑；请先派生新版本（新版本为草稿）再恢复原图",
                map.map_name, map.map_status
            )));
        }

        let bytes = self.read_package(&map, upload_root).await?;
        let restored = restore_original_package(&bytes)?;
        let parsed = parse_map_package(&restored)?;
        self.store_edited_package(id, &map, &restored, &parsed, upload_root, scope)
            .await?;

        Ok(MapImageVO {
            image: parsed.png_data_url(),
            resolution: parsed.resolution,
            origin: parsed.origin,
            width: parsed.width,
            height: parsed.height,
            map_hash: Some(parsed.sha256),
            erased_pixels: None,
        })
    }

    /// 把「改过栅格的新包」落盘并回写地图行（橡皮擦 / 恢复原图共用）。
    async fn store_edited_package(
        &self,
        id: i64,
        map: &DogMap,
        bytes: &[u8],
        parsed: &ParsedMapPackage,
        upload_root: &std::path::Path,
        scope: &DataScope,
    ) -> Result<(), AppError> {
        let original_name = map
            .file_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("map-{id}.zip"));
        let dir = upload_root.join("maps");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|error| AppError::InternalError(format!("创建地图目录失败: {error}")))?;
        let stored_name = format!("{}-{}", generate_id(), original_name);
        tokio::fs::write(dir.join(&stored_name), bytes)
            .await
            .map_err(|error| AppError::InternalError(format!("保存地图包失败: {error}")))?;

        let req = UpdateMapRequest {
            map_name: None,
            map_type: None,
            park_code: None,
            park_name: None,
            device_code: None,
            map_data: None,
            file_name: Some(original_name),
            file_path: Some(format!("maps/{stored_name}")),
            file_format: Some("zip".to_string()),
            file_size: Some(i64::try_from(bytes.len()).unwrap_or(i64::MAX)),
            remarks: None,
            map_version: None,
            frame_id: None,
            crs: None,
            map_origin: None,
            resolution: None,
            floors: None,
            road_count: None,
            nogo_count: None,
            map_status: None,
            map_hash: Some(parsed.sha256.clone()),
            map_width: i32::try_from(parsed.width).ok(),
            map_height: i32::try_from(parsed.height).ok(),
        };
        // 绕过 `update` 的「已发布不可改坐标」约束：这里改的是栅格内容，
        // 分辨率 / 原点 / 版本号均未变化，状态校验由调用方完成。
        self.repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {id} not found")))?;
        Ok(())
    }

    /// 读取地图包字节并校验 `map_hash`（发布后内容不可被偷改）。
    async fn read_package(
        &self,
        map: &DogMap,
        upload_root: &std::path::Path,
    ) -> Result<Vec<u8>, AppError> {
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

        Ok(bytes)
    }
}

/// 校验橡皮擦请求的规模，返回待应用的笔画。
fn validate_edit_request(req: &MapEditRequest) -> Result<&[MapEditStroke], AppError> {
    if req.strokes.is_empty() {
        return Err(AppError::BadRequest(
            "没有可提交的擦除笔画，请在预览画布上先用橡皮擦涂抹噪点".to_string(),
        ));
    }
    if req.strokes.len() > MAX_EDIT_STROKES {
        return Err(AppError::BadRequest(format!(
            "一次最多提交 {MAX_EDIT_STROKES} 笔擦除，请分批保存"
        )));
    }
    let points: usize = req.strokes.iter().map(|stroke| stroke.points.len()).sum();
    if points == 0 {
        return Err(AppError::BadRequest("擦除笔画为空".to_string()));
    }
    if points > MAX_EDIT_POINTS {
        return Err(AppError::BadRequest(format!(
            "一次最多提交 {MAX_EDIT_POINTS} 个采样点，请分批保存"
        )));
    }
    for stroke in &req.strokes {
        // 灰度值先拦一道：只允许 254 / 128（旧的 205 归一成 128），
        // 否则「未知」又会被写成 205——本体按 free_thresh: 0.2 会当成可通行。
        if let Some(value) = stroke.value {
            normalize_erase_value(value)?;
        }
        if let Some(radius) = stroke.radius
            && !radius.is_finite()
        {
            return Err(AppError::BadRequest("笔刷半径无效".to_string()));
        }
    }
    if let Some(value) = req.erase_value {
        normalize_erase_value(value)?;
    }
    Ok(&req.strokes)
}

/// 校验前端手填的 resolution / origin 是否与地图包内容一致。
fn validate_package_metadata(
    info: &UploadMapRequest,
    parsed: &ParsedMapPackage,
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
