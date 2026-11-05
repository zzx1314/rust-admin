use crate::business::dog::park::domain::{
    CreateParkRequest, DogPark, ParkPageQuery, ParkVO, UpdateParkRequest,
};
use crate::business::dog::park::repository::SeaOrmDogParkRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::generate_id;

pub struct DogParkService {
    repo: SeaOrmDogParkRepository,
}

impl DogParkService {
    pub fn new(repo: SeaOrmDogParkRepository) -> Self {
        Self { repo }
    }

    pub async fn create(
        &self,
        req: CreateParkRequest,
        scope: &DataScope,
    ) -> Result<DogPark, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64, scope: &DataScope) -> Result<DogPark, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Park with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: ParkPageQuery,
        scope: &DataScope,
    ) -> Result<PageResponse<ParkVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(ParkVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self, scope: &DataScope) -> Result<Vec<DogPark>, AppError> {
        self.repo
            .find_all(scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateParkRequest,
        scope: &DataScope,
    ) -> Result<DogPark, AppError> {
        self.repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Park with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!("Park with id {} not found", id)));
        }
        Ok(())
    }
}
