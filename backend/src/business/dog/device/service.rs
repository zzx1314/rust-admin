use crate::business::dog::device::domain::{
    CreateDeviceRequest, DevicePageQuery, DeviceVO, DogDevice, UpdateDeviceRequest,
};
use crate::business::dog::device::repository::SeaOrmDogDeviceRepository;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;

pub struct DogDeviceService {
    repo: SeaOrmDogDeviceRepository,
}

impl DogDeviceService {
    pub fn new(repo: SeaOrmDogDeviceRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateDeviceRequest) -> Result<DogDevice, AppError> {
        let id = generate_id();
        self.repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogDevice, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Device with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: DevicePageQuery,
    ) -> Result<PageResponse<DeviceVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records.into_iter().map(DeviceVO::from).collect(),
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self) -> Result<Vec<DogDevice>, AppError> {
        self.repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(&self, id: i64, req: UpdateDeviceRequest) -> Result<DogDevice, AppError> {
        self.repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Device with id {} not found", id)))
    }

    pub async fn update_status(&self, id: i64, status: &str) -> Result<DogDevice, AppError> {
        self.repo
            .update_status(id, status)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Device with id {} not found", id)))
    }

    pub async fn update_comm_time(&self, id: i64) -> Result<DogDevice, AppError> {
        self.repo
            .update_comm_time(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Device with id {} not found", id)))
    }

    pub async fn count(&self) -> Result<i64, AppError> {
        self.repo
            .count()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Device with id {} not found",
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
