use std::collections::HashMap;

use crate::business::dog::device::repository::SeaOrmDogDeviceRepository;
use crate::business::dog::user_device::domain::{
    CreateUserDeviceRequest, DogUserDevice, UserDevicePageQuery, UserDeviceVO,
};
use crate::business::dog::user_device::repository::SeaOrmDogUserDeviceRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::generate_id;

pub struct DogUserDeviceService {
    repo: SeaOrmDogUserDeviceRepository,
    device_repo: SeaOrmDogDeviceRepository,
}

impl DogUserDeviceService {
    pub fn new(
        repo: SeaOrmDogUserDeviceRepository,
        device_repo: SeaOrmDogDeviceRepository,
    ) -> Self {
        Self { repo, device_repo }
    }

    /// Binds a device to a user.
    ///
    /// Administrators may bind any user (`userId` in the body); regular users
    /// can only claim a device that is not bound to anybody yet.
    pub async fn bind(
        &self,
        req: CreateUserDeviceRequest,
        scope: &DataScope,
    ) -> Result<UserDeviceVO, AppError> {
        let target_user = if scope.is_admin() {
            req.user_id.unwrap_or(scope.user_id())
        } else {
            scope.user_id()
        };

        let device = self
            .device_repo
            .find_by_id_unscoped(req.device_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| {
                AppError::BadRequest(format!("Device with id {} not found", req.device_id))
            })?;

        if !scope.is_admin() {
            let bound = self
                .repo
                .count_active_by_device(req.device_id)
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;
            if bound > 0 {
                return Err(AppError::Conflict(format!(
                    "Device with id {} is already bound to another user",
                    req.device_id
                )));
            }
        }

        if let Some(existing) = self
            .repo
            .find_any_by_user_and_device(target_user, req.device_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
        {
            if existing.is_deleted == 0 {
                return Err(AppError::Conflict(format!(
                    "Device with id {} is already bound to user {}",
                    req.device_id, target_user
                )));
            }
            self.repo
                .restore(existing.id)
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;
            let restored = self
                .repo
                .find_by_id(existing.id, scope)
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?
                .ok_or_else(|| {
                    AppError::NotFound(format!("Binding with id {} not found", existing.id))
                })?;
            return Ok(UserDeviceVO::from_model(restored, Some(device.device_id)));
        }

        let binding = self
            .repo
            .create(&req, target_user, generate_id())
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(UserDeviceVO::from_model(binding, Some(device.device_id)))
    }

    pub async fn get_by_id(&self, id: i64, scope: &DataScope) -> Result<UserDeviceVO, AppError> {
        let binding = self
            .repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Binding with id {} not found", id)))?;
        Ok(self.enrich(vec![binding]).await?.remove(0))
    }

    pub async fn list_page(
        &self,
        query: UserDevicePageQuery,
        scope: &DataScope,
    ) -> Result<PageResponse<UserDeviceVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            self.enrich(records).await?,
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self, scope: &DataScope) -> Result<Vec<UserDeviceVO>, AppError> {
        let records = self
            .repo
            .find_all(scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        self.enrich(records).await
    }

    pub async fn list_by_user(
        &self,
        user_id: i64,
        scope: &DataScope,
    ) -> Result<Vec<UserDeviceVO>, AppError> {
        let records = self
            .repo
            .find_all_by_user(user_id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        self.enrich(records).await
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Binding with id {} not found",
                id
            )));
        }
        Ok(())
    }

    /// Assignment entry point used by the device gateway: resolves the device
    /// by its hardware id (the value reported during registration) and binds
    /// it to the user.
    pub async fn bind_by_device_code(
        &self,
        device_code: &str,
        user_id: Option<i64>,
        scope: &DataScope,
    ) -> Result<UserDeviceVO, AppError> {
        let device = self.resolve_device(device_code).await?;
        self.bind(
            CreateUserDeviceRequest {
                user_id,
                device_id: device.id,
            },
            scope,
        )
        .await
    }

    pub async fn list_by_device_code(
        &self,
        device_code: &str,
        scope: &DataScope,
    ) -> Result<Vec<UserDeviceVO>, AppError> {
        let device = self.resolve_device(device_code).await?;
        let records = self
            .repo
            .find_all_by_device(device.id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        self.enrich(records).await
    }

    pub async fn unbind_by_device_code(
        &self,
        device_code: &str,
        user_id: Option<i64>,
        scope: &DataScope,
    ) -> Result<(), AppError> {
        let device = self.resolve_device(device_code).await?;
        let target_user = if scope.is_admin() {
            user_id.unwrap_or(scope.user_id())
        } else {
            scope.user_id()
        };
        let binding = self
            .repo
            .find_active_by_user_and_device(target_user, device.id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "Device '{}' is not bound to user {}",
                    device_code, target_user
                ))
            })?;
        self.delete(binding.id, scope).await
    }

    async fn resolve_device(
        &self,
        device_code: &str,
    ) -> Result<crate::business::dog::device::domain::DogDevice, AppError> {
        let code = device_code.trim();
        if code.is_empty() {
            return Err(AppError::BadRequest(
                "deviceId must not be empty".to_string(),
            ));
        }
        self.device_repo
            .find_by_device_code(code)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "Device '{}' is not registered yet; register it before assigning",
                    code
                ))
            })
    }

    async fn enrich(&self, bindings: Vec<DogUserDevice>) -> Result<Vec<UserDeviceVO>, AppError> {
        let device_ids: Vec<i64> = bindings.iter().map(|b| b.device_id).collect();
        let devices = self
            .device_repo
            .find_by_ids(&device_ids)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        let codes: HashMap<i64, String> = devices
            .into_iter()
            .map(|device| (device.id, device.device_id))
            .collect();

        Ok(bindings
            .into_iter()
            .map(|binding| {
                let code = codes.get(&binding.device_id).cloned();
                UserDeviceVO::from_model(binding, code)
            })
            .collect())
    }
}
