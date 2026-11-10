use crate::business::dog::map::domain::{DogMap, is_publishable_map_status};
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::business::dog::park::repository::SeaOrmDogParkRepository;
use crate::business::dog::route_library::domain::{
    CreateRouteRequest, DogRouteLibrary, RoutePageQuery, RouteVO, UpdateRouteRequest,
};
use crate::business::dog::route_library::repository::SeaOrmDogRouteLibraryRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;

pub struct DogRouteLibraryService {
    repo: SeaOrmDogRouteLibraryRepository,
    map_repo: SeaOrmDogMapRepository,
    park_repo: SeaOrmDogParkRepository,
}

impl DogRouteLibraryService {
    pub fn new(
        repo: SeaOrmDogRouteLibraryRepository,
        map_repo: SeaOrmDogMapRepository,
        park_repo: SeaOrmDogParkRepository,
    ) -> Self {
        Self {
            repo,
            map_repo,
            park_repo,
        }
    }

    pub async fn create(&self, req: CreateRouteRequest) -> Result<DogRouteLibrary, AppError> {
        let baseline = self
            .resolve_baseline(req.park_id, req.map_id, false)
            .await?;
        let (map_id, map_version, map_frame) = freeze_baseline(baseline);
        let mut req = req;
        if map_id.is_some() {
            req.map_id = map_id;
        }
        let id = generate_id();
        self.repo
            .create(&req, id, map_version, map_frame)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogRouteLibrary, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Route with id {} not found", id)))
    }

    /// 转成 VO 并补齐「所属园区」名称，供单条详情 / 新增 / 修改接口返回。
    pub async fn to_vo(&self, route: DogRouteLibrary) -> Result<RouteVO, AppError> {
        let mut vo = RouteVO::from(route);
        if let Some(park_id) = vo.park_id {
            let park_names = self
                .repo
                .find_park_names(&[park_id])
                .await
                .map_err(AppError::DatabaseError)?;
            vo.park_name = park_names.get(&park_id).cloned();
        }
        Ok(vo)
    }

    pub async fn list_page(
        &self,
        query: RoutePageQuery,
    ) -> Result<PageResponse<RouteVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseError)?;

        let mut park_ids: Vec<i64> = records.iter().filter_map(|r| r.park_id).collect();
        park_ids.sort_unstable();
        park_ids.dedup();
        let park_names = self
            .repo
            .find_park_names(&park_ids)
            .await
            .map_err(AppError::DatabaseError)?;

        let vo_list: Vec<RouteVO> = records
            .into_iter()
            .map(|route| {
                let mut vo = RouteVO::from(route);
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

    pub async fn list_all(&self) -> Result<Vec<DogRouteLibrary>, AppError> {
        self.repo.find_all().await.map_err(AppError::DatabaseError)
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateRouteRequest,
    ) -> Result<DogRouteLibrary, AppError> {
        let existing = self.get_by_id(id).await?;
        let mut req = req;
        let mut map_version = None;
        let mut map_frame = None;

        let requested_map_id = req.map_id.filter(|map_id| Some(*map_id) != existing.map_id);
        let target_park_id = req.park_id.or(existing.park_id);

        if let Some(map_id) = requested_map_id {
            // 显式换绑：必须换到该园区可用的已发布版本。
            let baseline = self
                .resolve_baseline(target_park_id, Some(map_id), false)
                .await?;
            let (frozen_id, version, frame) = freeze_baseline(baseline);
            req.map_id = frozen_id;
            map_version = version;
            map_frame = frame;
        } else if req.map_id.is_none()
            && req.park_id.is_some()
            && (existing.map_id.is_none() || req.park_id != existing.park_id)
        {
            // 未声明版本的历史路线改挂园区，或换了园区但未显式选版本时，
            // 自动冻结新园区当前基线。
            let baseline = self.resolve_baseline(req.park_id, None, false).await?;
            let (frozen_id, version, frame) = freeze_baseline(baseline);
            if frozen_id.is_some() {
                req.map_id = frozen_id;
                map_version = version;
                map_frame = frame;
            }
        } else if req.park_id.is_some() {
            self.ensure_park_exists(req.park_id).await?;
        }

        self.repo
            .update(id, &req, map_version, map_frame)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("Route with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseError)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Route with id {} not found",
                id
            )));
        }
        Ok(())
    }

    /// 解析路线要冻结的地图基线。
    ///
    /// - 显式传入 `map_id`：校验版本归属园区、且不是草稿 / 已归档；
    /// - 只传 `park_id`：使用园区当前生效基线；园区未绑定时直接拒绝，
    ///   对应原型中「未标定园区禁止编排巡检路线」的约束；
    /// - 两者都没有：兼容历史无园区路线，不冻结版本。
    async fn resolve_baseline(
        &self,
        park_id: Option<i64>,
        map_id: Option<i64>,
        allow_archived: bool,
    ) -> Result<Option<DogMap>, AppError> {
        let (park_id, map_id) = match (park_id, map_id) {
            (None, None) => return Ok(None),
            (Some(park_id), map_id) => (park_id, map_id),
            (None, Some(map_id)) => {
                let map = self.get_map(map_id).await?;
                if !allow_archived && !is_publishable_map_status(&map.map_status) {
                    return Err(AppError::BadRequest(format!(
                        "地图 {} 当前状态为「{}」，只有已发布 / 部分标定的基线可用于路线",
                        map.map_name, map.map_status
                    )));
                }
                return Ok(Some(map));
            }
        };

        let park = self
            .park_repo
            .find_any_by_id(park_id)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::BadRequest(format!("Park with id {park_id} not found")))?;

        let map_id = match map_id {
            Some(map_id) => map_id,
            None => park.map_id.ok_or_else(|| {
                AppError::BadRequest(format!(
                    "园区「{}」尚未绑定地图基线，请先完成建图并发布版本",
                    park.park_name
                ))
            })?,
        };
        let map = self.get_map(map_id).await?;
        if map.park_id != Some(park_id) {
            return Err(AppError::BadRequest(format!(
                "地图 {} 不属于园区「{}」",
                map.map_name, park.park_name
            )));
        }
        if !allow_archived && !is_publishable_map_status(&map.map_status) {
            return Err(AppError::BadRequest(format!(
                "地图 {} 当前状态为「{}」，只有已发布 / 部分标定的基线可用于路线",
                map.map_name, map.map_status
            )));
        }
        Ok(Some(map))
    }

    async fn get_map(&self, map_id: i64) -> Result<DogMap, AppError> {
        self.map_repo
            .find_any_by_id(map_id)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::BadRequest(format!("Map with id {map_id} not found")))
    }

    /// 园区是可选关联：指定时必须存在且未删除。
    async fn ensure_park_exists(&self, park_id: Option<i64>) -> Result<(), AppError> {
        let Some(park_id) = park_id else {
            return Ok(());
        };
        let park = self
            .park_repo
            .find_any_by_id(park_id)
            .await
            .map_err(AppError::DatabaseError)?;
        if park.is_none() {
            return Err(AppError::BadRequest(format!(
                "Park with id {park_id} not found"
            )));
        }
        Ok(())
    }
}

fn freeze_baseline(baseline: Option<DogMap>) -> (Option<i64>, Option<String>, Option<String>) {
    match baseline {
        Some(map) => (Some(map.id), map.map_version, map.frame_id),
        None => (None, None, None),
    }
}

fn generate_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
