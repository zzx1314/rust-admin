use crate::business::dog::device::domain::{
    AssignUsersRequest, AssignableUserVO, CreateDeviceRequest, DeviceFilter, DevicePageQuery,
    DeviceVO, DogDevice, UnassignUsersRequest, UpdateDeviceRequest,
};
use crate::business::dog::device::audit::{self, CommandAudit, NewCommandAudit};
use crate::business::dog::device::repository::SeaOrmDogDeviceRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::generate_id;

pub struct DogDeviceService {
    repo: SeaOrmDogDeviceRepository,
}

impl DogDeviceService {
    pub fn new(repo: SeaOrmDogDeviceRepository) -> Self {
        Self { repo }
    }

    pub async fn create(
        &self,
        req: CreateDeviceRequest,
        scope: &DataScope,
    ) -> Result<DeviceVO, AppError> {
        let code = req.device_id.trim();
        if code.is_empty() {
            return Err(AppError::BadRequest("设备编号不能为空".to_string()));
        }
        let existing = self
            .repo
            .find_raw_by_code(code)
            .await
            .map_err(AppError::DatabaseError)?;
        let device = match existing {
            Some(existing) if existing.is_deleted == 0 => {
                return Err(AppError::BadRequest(format!("设备编号 {code} 已存在")));
            }
            // 软删除过的设备重新接入：恢复台账并覆盖业务字段。
            Some(existing) => self.revive(existing.id, &req, scope).await?,
            None => self
                .repo
                .create(&req, generate_id(), scope.user_id())
                .await
                .map_err(AppError::DatabaseError)?,
        };

        // 管理员可在同步时直接带上分配；普通用户创建的设备默认归创建者管辖。
        if let Some(user_ids) = req.user_ids.as_ref().filter(|ids| !ids.is_empty()) {
            if scope.is_admin() {
                self.repo
                    .assign_users(&[device.id], user_ids, false)
                    .await
                    .map_err(AppError::DatabaseError)?;
            }
        } else if !scope.is_admin() {
            self.repo
                .assign_users(&[device.id], &[scope.user_id()], false)
                .await
                .map_err(AppError::DatabaseError)?;
        }

        self.to_vo(device).await
    }

    /// 业务平台同步入口：按 `deviceId` 幂等写入（存在则更新，不存在则创建）。
    pub async fn upsert(
        &self,
        req: CreateDeviceRequest,
        scope: &DataScope,
    ) -> Result<DeviceVO, AppError> {
        let code = req.device_id.trim();
        if code.is_empty() {
            return Err(AppError::BadRequest("设备编号不能为空".to_string()));
        }
        let Some(existing) = self
            .repo
            .find_raw_by_code(code)
            .await
            .map_err(AppError::DatabaseError)?
        else {
            return self.create(req, scope).await;
        };

        let update = UpdateDeviceRequest {
            device_name: req.device_name.clone(),
            model: req.model.clone(),
            status: req.status.clone(),
            firmware: req.firmware.clone(),
            device_ip: req.device_ip.clone(),
            park_code: req.park_code.clone(),
            park_name: req.park_name.clone(),
            remarks: req.remarks.clone(),
        };
        let device = if existing.is_deleted == 0 {
            self.repo
                .update(existing.id, &update, scope)
                .await
                .map_err(AppError::DatabaseError)?
                .ok_or_else(|| AppError::NotFound(format!("设备 {code} 不存在或无权访问")))?
        } else {
            self.revive(existing.id, &req, scope).await?
        };

        if let Some(user_ids) = req.user_ids.as_ref().filter(|ids| !ids.is_empty())
            && scope.is_admin()
        {
            self.repo
                .assign_users(&[device.id], user_ids, false)
                .await
                .map_err(AppError::DatabaseError)?;
        }
        self.to_vo(device).await
    }

    async fn revive(
        &self,
        id: i64,
        req: &CreateDeviceRequest,
        scope: &DataScope,
    ) -> Result<DogDevice, AppError> {
        let mut active_model: crate::business::dog::device::entity::ActiveModel = self
            .repo
            .find_raw_by_code(req.device_id.trim())
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound("设备不存在".to_string()))?
            .into();
        active_model.id = sea_orm::ActiveValue::Unchanged(id);
        active_model.device_name = sea_orm::ActiveValue::set(req.device_name.clone());
        active_model.model = sea_orm::ActiveValue::set(req.model.clone());
        active_model.status =
            sea_orm::ActiveValue::set(req.status.clone().unwrap_or_else(|| "离线".to_string()));
        active_model.firmware = sea_orm::ActiveValue::set(req.firmware.clone());
        active_model.device_ip = sea_orm::ActiveValue::set(req.device_ip.clone());
        active_model.park_code = sea_orm::ActiveValue::set(req.park_code.clone());
        active_model.park_name = sea_orm::ActiveValue::set(req.park_name.clone());
        active_model.remarks = sea_orm::ActiveValue::set(req.remarks.clone());
        active_model.user_id = sea_orm::ActiveValue::set(Some(scope.user_id()));
        active_model.update_time = sea_orm::ActiveValue::set(Some(chrono::Utc::now()));
        active_model.is_deleted = sea_orm::ActiveValue::set(0);
        self.repo
            .restore(active_model)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn get(&self, id: i64, scope: &DataScope) -> Result<DogDevice, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("设备 {id} 不存在或无权访问")))
    }

    pub async fn get_vo(&self, id: i64, scope: &DataScope) -> Result<DeviceVO, AppError> {
        let device = self.get(id, scope).await?;
        self.to_vo(device).await
    }

    pub async fn get_vo_by_code(&self, code: &str) -> Result<Option<DeviceVO>, AppError> {
        let Some(device) = self
            .repo
            .find_by_code(code)
            .await
            .map_err(AppError::DatabaseError)?
        else {
            return Ok(None);
        };
        self.to_vo(device).await.map(Some)
    }

    async fn to_vo(&self, device: DogDevice) -> Result<DeviceVO, AppError> {
        let mut users = self
            .repo
            .users_for_devices(&[device.id])
            .await
            .map_err(AppError::DatabaseError)?;
        let users = users.remove(&device.id).unwrap_or_default();
        Ok(DeviceVO::from_model(device, users))
    }

    pub async fn list_page(
        &self,
        query: DevicePageQuery,
        scope: &DataScope,
    ) -> Result<PageResponse<DeviceVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query, scope)
            .await
            .map_err(AppError::DatabaseError)?;
        let ids: Vec<i64> = records.iter().map(|record| record.id).collect();
        let mut users = self
            .repo
            .users_for_devices(&ids)
            .await
            .map_err(AppError::DatabaseError)?;
        let vo: Vec<DeviceVO> = records
            .into_iter()
            .map(|record| {
                let assigned = users.remove(&record.id).unwrap_or_default();
                DeviceVO::from_model(record, assigned)
            })
            .collect();
        Ok(PageResponse::new(
            vo,
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(
        &self,
        filter: DeviceFilter,
        scope: &DataScope,
    ) -> Result<Vec<DeviceVO>, AppError> {
        let records = self
            .repo
            .find_all(&filter, scope)
            .await
            .map_err(AppError::DatabaseError)?;
        let ids: Vec<i64> = records.iter().map(|record| record.id).collect();
        let mut users = self
            .repo
            .users_for_devices(&ids)
            .await
            .map_err(AppError::DatabaseError)?;
        Ok(records
            .into_iter()
            .map(|record| {
                let assigned = users.remove(&record.id).unwrap_or_default();
                DeviceVO::from_model(record, assigned)
            })
            .collect())
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateDeviceRequest,
        scope: &DataScope,
    ) -> Result<DeviceVO, AppError> {
        let device = self
            .repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseError)?
            .ok_or_else(|| AppError::NotFound(format!("设备 {id} 不存在或无权访问")))?;
        self.to_vo(device).await
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseError)?;
        if !deleted {
            return Err(AppError::NotFound(format!("设备 {id} 不存在或无权访问")));
        }
        Ok(())
    }

    pub async fn assign_users(&self, req: AssignUsersRequest) -> Result<u64, AppError> {
        if req.device_ids.is_empty() {
            return Err(AppError::BadRequest("请选择要分配的设备".to_string()));
        }
        if req.user_ids.is_empty() && req.mode.as_deref() != Some("replace") {
            return Err(AppError::BadRequest("请选择要分配的用户".to_string()));
        }
        let replace = req.mode.as_deref() == Some("replace");
        self.repo
            .assign_users(&req.device_ids, &req.user_ids, replace)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn unassign_users(&self, req: UnassignUsersRequest) -> Result<u64, AppError> {
        if req.device_ids.is_empty() || req.user_ids.is_empty() {
            return Err(AppError::BadRequest(
                "请选择要解除分配的设备与用户".to_string(),
            ));
        }
        self.repo
            .unassign_users(&req.device_ids, &req.user_ids)
            .await
            .map_err(AppError::DatabaseError)
    }

    pub async fn assignable_users(&self) -> Result<Vec<AssignableUserVO>, AppError> {
        self.repo
            .assignable_users()
            .await
            .map_err(AppError::DatabaseError)
    }

    /// 按幂等键查已下发的指令；命中说明这是一次重试。
    pub async fn find_command(&self, request_id: &str) -> Result<Option<CommandAudit>, AppError> {
        audit::find_by_request(self.repo.conn(), request_id)
            .await
            .map_err(AppError::DatabaseError)
    }

    /// 下发前落一条 pending 审计，返回审计主键。
    pub async fn begin_command(&self, entry: NewCommandAudit<'_>) -> Result<i64, AppError> {
        audit::insert_pending(self.repo.conn(), &entry)
            .await
            .map_err(AppError::DatabaseError)
    }

    /// 回填下发结果。审计写失败不应该让已经成功的指令变成失败，
    /// 因此这里只记日志不向上抛错。
    pub async fn finish_command(
        &self,
        id: i64,
        status: &str,
        result: Option<&str>,
        message: Option<&str>,
        latency_ms: i64,
    ) {
        if let Err(error) = audit::finish(self.repo.conn(), id, status, result, message, latency_ms)
            .await
        {
            tracing::error!(audit_id = id, %error, "回填指令审计失败");
        }
    }
}
