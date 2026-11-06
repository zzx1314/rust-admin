use crate::business::dog::route_library::domain::{
    CreateRouteRequest, DogRouteLibrary, RoutePageQuery, RouteVO, UpdateRouteRequest,
};
use crate::business::dog::route_library::repository::SeaOrmDogRouteLibraryRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;

pub struct DogRouteLibraryService {
    repo: SeaOrmDogRouteLibraryRepository,
}

impl DogRouteLibraryService {
    pub fn new(repo: SeaOrmDogRouteLibraryRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateRouteRequest) -> Result<DogRouteLibrary, AppError> {
        self.ensure_park_exists(req.park_id).await?;
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogRouteLibrary, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
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
                .map_err(AppError::DatabaseErrorSeaOrm)?;
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
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let mut park_ids: Vec<i64> = records.iter().filter_map(|r| r.park_id).collect();
        park_ids.sort_unstable();
        park_ids.dedup();
        let park_names = self
            .repo
            .find_park_names(&park_ids)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

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
        self.repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateRouteRequest,
    ) -> Result<DogRouteLibrary, AppError> {
        self.ensure_park_exists(req.park_id).await?;
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Route with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Route with id {} not found",
                id
            )));
        }
        Ok(())
    }

    /// 园区是可选关联：指定时必须存在且未删除。
    async fn ensure_park_exists(&self, park_id: Option<i64>) -> Result<(), AppError> {
        let Some(park_id) = park_id else {
            return Ok(());
        };
        let park_names = self
            .repo
            .find_park_names(&[park_id])
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !park_names.contains_key(&park_id) {
            return Err(AppError::BadRequest(format!(
                "Park with id {} not found",
                park_id
            )));
        }
        Ok(())
    }
}

fn generate_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
