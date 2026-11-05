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

    pub async fn list_page(
        &self,
        query: RoutePageQuery,
    ) -> Result<PageResponse<RouteVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(RouteVO::from).collect(),
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
}

fn generate_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
