use crate::business::dog::map::domain::{
    CreateMapRequest, DogMap, MapPageQuery, MapVO, UpdateMapRequest,
};
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::business::dog::park::repository::SeaOrmDogParkRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::generate_id;

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
        self.ensure_park_access(req.park_id, scope).await?;
        let id = generate_id();
        self.repo
            .create(&req, id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64, scope: &DataScope) -> Result<DogMap, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
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

        Ok(PageResponse::new(
            records.into_iter().map(MapVO::from).collect(),
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

    pub async fn update(
        &self,
        id: i64,
        req: UpdateMapRequest,
        scope: &DataScope,
    ) -> Result<DogMap, AppError> {
        if let Some(park_id) = req.park_id {
            self.ensure_park_access(park_id, scope).await?;
        }
        self.repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Map with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!("Map with id {} not found", id)));
        }
        Ok(())
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
