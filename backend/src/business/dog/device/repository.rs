use crate::business::dog::device::domain::{
    AssignableUserVO, CreateDeviceRequest, DeviceFilter, DevicePageQuery, DeviceUserVO, DogDevice,
    UpdateDeviceRequest,
};
use crate::business::dog::device::entity::ActiveModel as DeviceActiveModel;
use crate::business::dog::device::entity::Column as DeviceColumn;
use crate::business::dog::device::entity::Entity as DeviceEntity;
use crate::common::base::order_desc;
use crate::common::data_scope::DataScope;
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbBackend, DbErr,
    EntityTrait, FromQueryResult, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Statement,
    Value,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, FromQueryResult)]
struct DeviceUserRow {
    device_id: i64,
    id: i64,
    username: String,
    real_name: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct IdRow {
    id: i64,
}

#[derive(Clone)]
pub struct SeaOrmDogDeviceRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogDeviceRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }

    /// 指令审计台账直接用 SQL 访问连接（不是设备实体的 ORM 查询）。
    pub fn conn(&self) -> &Arc<DatabaseConnection> {
        &self.conn
    }

    pub async fn create(
        &self,
        req: &CreateDeviceRequest,
        id: i64,
        user_id: i64,
    ) -> Result<DogDevice, DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, user_id, now);
        DeviceEntity::insert(active_model).exec(&*self.conn).await?;
        DeviceEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|record| record.expect("device must exist after insert"))
    }

    /// 包含软删除记录地按设备编号查找，供「重新接入时恢复台账」使用。
    pub async fn find_raw_by_code(&self, device_code: &str) -> Result<Option<DogDevice>, DbErr> {
        DeviceEntity::find()
            .filter(DeviceColumn::DeviceId.eq(device_code.trim()))
            .one(&*self.conn)
            .await
    }

    pub async fn find_by_id(&self, id: i64, scope: &DataScope) -> Result<Option<DogDevice>, DbErr> {
        let device = DeviceEntity::find()
            .filter(DeviceColumn::Id.eq(id))
            .filter(DeviceColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;
        let Some(device) = device else {
            return Ok(None);
        };
        if let Some(owner) = scope.owner_filter()
            && !self.is_assigned(device.id, owner).await?
        {
            return Ok(None);
        }
        Ok(Some(device))
    }

    pub async fn find_by_code(&self, code: &str) -> Result<Option<DogDevice>, DbErr> {
        DeviceEntity::find()
            .filter(DeviceColumn::DeviceId.eq(code.trim()))
            .filter(DeviceColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn is_assigned(&self, device_id: i64, user_id: i64) -> Result<bool, DbErr> {
        let count = self
            .count_assignments(
                "device_id = ?1 AND user_id = ?2",
                vec![device_id.into(), user_id.into()],
            )
            .await?;
        Ok(count > 0)
    }

    async fn count_assignments(&self, condition: &str, values: Vec<Value>) -> Result<i64, DbErr> {
        let sql = format!(
            "SELECT COUNT(*) AS count FROM dog_user_device WHERE is_deleted = 0 AND {condition}"
        );
        let row: Option<CountRow> = CountRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            values,
        ))
        .one(&*self.conn)
        .await?;
        Ok(row.map(|row| row.count).unwrap_or(0))
    }

    /// 返回当前数据范围内可见的设备 id。
    ///
    /// `None` 表示管理员且未指定用户过滤，即不限制；`Some(vec![])` 表示不可见任何设备。
    async fn visible_device_ids(
        &self,
        scope: &DataScope,
        requested_user: Option<i64>,
    ) -> Result<Option<Vec<i64>>, DbErr> {
        let Some(user_id) = scope.owner_filter().or(requested_user) else {
            return Ok(None);
        };
        self.assigned_device_ids(user_id).await.map(Some)
    }

    async fn assigned_device_ids(&self, user_id: i64) -> Result<Vec<i64>, DbErr> {
        let sql = "SELECT device_id AS id FROM dog_user_device
                   WHERE user_id = ?1 AND is_deleted = 0";
        let rows = IdRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            vec![user_id.into()],
        ))
        .all(&*self.conn)
        .await?;
        Ok(rows.into_iter().map(|row| row.id).collect())
    }

    fn apply_filters(mut condition: Condition, filter: &DeviceFilter) -> Condition {
        if let Some(device_id) = filter.device_id.as_deref()
            && !device_id.trim().is_empty()
        {
            condition = condition.add(DeviceColumn::DeviceId.contains(device_id.trim()));
        }
        if let Some(device_name) = filter.device_name.as_deref()
            && !device_name.trim().is_empty()
        {
            condition = condition.add(DeviceColumn::DeviceName.contains(device_name.trim()));
        }
        if let Some(status) = filter.status.as_deref()
            && !status.trim().is_empty()
            && status != "全部"
        {
            condition = condition.add(DeviceColumn::Status.eq(status.trim()));
        }
        if let Some(park_code) = filter.park_code.as_deref()
            && !park_code.trim().is_empty()
        {
            condition = condition.add(DeviceColumn::ParkCode.eq(park_code.trim()));
        }
        condition
    }

    pub async fn find_all_with_page(
        &self,
        query: &DevicePageQuery,
        scope: &DataScope,
    ) -> Result<(Vec<DogDevice>, i64), DbErr> {
        let visible = self.visible_device_ids(scope, query.user_id).await?;
        if matches!(&visible, Some(ids) if ids.is_empty()) {
            return Ok((Vec::new(), 0));
        }

        let filter = DeviceFilter {
            device_id: query.device_id.clone(),
            device_name: query.device_name.clone(),
            status: query.status.clone(),
            park_code: query.park_code.clone(),
            user_id: query.user_id,
        };
        let mut condition = Self::apply_filters(Condition::all(), &filter);
        if let Some(ids) = &visible {
            condition = condition.add(DeviceColumn::Id.is_in(ids.clone()));
        }

        let base = DeviceEntity::find()
            .filter(DeviceColumn::IsDeleted.eq(0))
            .filter(condition.clone());
        let total = base.clone().count(&*self.conn).await?;
        let current = query.current.max(1);
        let size = query.size.max(1);
        let offset = (current - 1) * size;
        let records = base
            .order_by(DeviceColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;
        Ok((records, total as i64))
    }

    pub async fn find_all(
        &self,
        filter: &DeviceFilter,
        scope: &DataScope,
    ) -> Result<Vec<DogDevice>, DbErr> {
        let visible = self.visible_device_ids(scope, filter.user_id).await?;
        if matches!(&visible, Some(ids) if ids.is_empty()) {
            return Ok(Vec::new());
        }
        let mut condition = Self::apply_filters(Condition::all(), filter);
        if let Some(ids) = &visible {
            condition = condition.add(DeviceColumn::Id.is_in(ids.clone()));
        }
        DeviceEntity::find()
            .filter(DeviceColumn::IsDeleted.eq(0))
            .filter(condition)
            .order_by(DeviceColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn restore(&self, active_model: DeviceActiveModel) -> Result<DogDevice, DbErr> {
        let id = match active_model.id.clone() {
            ActiveValue::Set(id) | ActiveValue::Unchanged(id) => id,
            ActiveValue::NotSet => {
                return Err(DbErr::Custom("missing device id".to_string()));
            }
        };
        DeviceEntity::update(active_model).exec(&*self.conn).await?;
        DeviceEntity::find_by_id(id)
            .one(&*self.conn)
            .await?
            .ok_or_else(|| DbErr::RecordNotFound(format!("device {id}")))
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateDeviceRequest,
        scope: &DataScope,
    ) -> Result<Option<DogDevice>, DbErr> {
        let Some(existing) = self.find_by_id(id, scope).await? else {
            return Ok(None);
        };
        let mut active_model: DeviceActiveModel = existing.into();
        if let Some(value) = &req.device_name {
            active_model.device_name = ActiveValue::set(Some(value.clone()));
        }
        if let Some(value) = &req.model {
            active_model.model = ActiveValue::set(Some(value.clone()));
        }
        if let Some(value) = &req.status {
            active_model.status = ActiveValue::set(value.clone());
        }
        if let Some(value) = &req.firmware {
            active_model.firmware = ActiveValue::set(Some(value.clone()));
        }
        if let Some(value) = &req.device_ip {
            active_model.device_ip = ActiveValue::set(Some(value.clone()));
        }
        if let Some(value) = &req.park_code {
            active_model.park_code = ActiveValue::set(Some(value.clone()));
        }
        if let Some(value) = &req.park_name {
            active_model.park_name = ActiveValue::set(Some(value.clone()));
        }
        if let Some(value) = &req.remarks {
            active_model.remarks = ActiveValue::set(Some(value.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));
        DeviceEntity::update(active_model).exec(&*self.conn).await?;
        self.find_by_id(id, scope).await
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<bool, DbErr> {
        let Some(existing) = self.find_by_id(id, scope).await? else {
            return Ok(false);
        };
        let mut active_model: DeviceActiveModel = existing.into();
        active_model.is_deleted = ActiveValue::set(1);
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));
        DeviceEntity::update(active_model).exec(&*self.conn).await?;
        Ok(true)
    }

    /// 批量分配：`replace=true` 先清空设备已有分配，再写入新用户。
    pub async fn assign_users(
        &self,
        device_ids: &[i64],
        user_ids: &[i64],
        replace: bool,
    ) -> Result<u64, DbErr> {
        let now = chrono::Utc::now().to_rfc3339();
        if replace && !device_ids.is_empty() {
            let marks = placeholders(2, device_ids.len());
            let sql = format!(
                "UPDATE dog_user_device SET is_deleted = 1, update_time = ?1
                 WHERE device_id IN ({marks}) AND is_deleted = 0"
            );
            let mut values: Vec<Value> = vec![now.clone().into()];
            values.extend(device_ids.iter().map(|id| (*id).into()));
            self.conn
                .execute(Statement::from_sql_and_values(
                    DbBackend::Sqlite,
                    sql,
                    values,
                ))
                .await?;
        }

        let mut affected = 0u64;
        for device_id in device_ids {
            for user_id in user_ids {
                let result = self
                    .conn
                    .execute(Statement::from_sql_and_values(
                        DbBackend::Sqlite,
                        "INSERT INTO dog_user_device (user_id, device_id, create_time, update_time, is_deleted)
                         VALUES (?1, ?2, ?3, ?3, 0)
                         ON CONFLICT(user_id, device_id) DO UPDATE
                         SET is_deleted = 0, update_time = excluded.update_time",
                        vec![(*user_id).into(), (*device_id).into(), now.clone().into()],
                    ))
                    .await?;
                affected += result.rows_affected();
            }
        }
        Ok(affected)
    }

    pub async fn unassign_users(&self, device_ids: &[i64], user_ids: &[i64]) -> Result<u64, DbErr> {
        if device_ids.is_empty() || user_ids.is_empty() {
            return Ok(0);
        }
        let device_marks = placeholders(2, device_ids.len());
        let user_marks = placeholders(2 + device_ids.len(), user_ids.len());
        let sql = format!(
            "UPDATE dog_user_device SET is_deleted = 1, update_time = ?1
             WHERE device_id IN ({device_marks}) AND user_id IN ({user_marks}) AND is_deleted = 0"
        );
        let mut values: Vec<Value> = vec![chrono::Utc::now().to_rfc3339().into()];
        values.extend(device_ids.iter().map(|id| (*id).into()));
        values.extend(user_ids.iter().map(|id| (*id).into()));
        let result = self
            .conn
            .execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                sql,
                values,
            ))
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn users_for_devices(
        &self,
        device_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<DeviceUserVO>>, DbErr> {
        let mut result: HashMap<i64, Vec<DeviceUserVO>> = HashMap::new();
        if device_ids.is_empty() {
            return Ok(result);
        }
        let marks = placeholders(1, device_ids.len());
        let sql = format!(
            "SELECT ud.device_id AS device_id, u.id AS id, u.username AS username, u.real_name AS real_name
             FROM dog_user_device ud
             JOIN p_sys_user u ON u.id = ud.user_id
             WHERE ud.is_deleted = 0 AND (u.is_deleted IS NULL OR u.is_deleted = 0)
               AND ud.device_id IN ({marks})
             ORDER BY u.id ASC"
        );
        let values: Vec<Value> = device_ids.iter().map(|id| (*id).into()).collect();
        let rows = DeviceUserRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            sql,
            values,
        ))
        .all(&*self.conn)
        .await?;
        for row in rows {
            result.entry(row.device_id).or_default().push(DeviceUserVO {
                id: row.id,
                username: row.username,
                real_name: row.real_name,
            });
        }
        Ok(result)
    }

    /// 可分配用户：启用且未删除的中台账号。
    pub async fn assignable_users(&self) -> Result<Vec<AssignableUserVO>, DbErr> {
        #[derive(Debug, FromQueryResult)]
        struct UserRow {
            id: i64,
            username: String,
            real_name: Option<String>,
        }
        let rows = UserRow::find_by_statement(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT id, username, real_name FROM p_sys_user
             WHERE (is_deleted IS NULL OR is_deleted = 0)
               AND (enable IS NULL OR enable = 1)
             ORDER BY id ASC",
        ))
        .all(&*self.conn)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| AssignableUserVO {
                id: row.id,
                username: row.username,
                real_name: row.real_name,
            })
            .collect())
    }

    pub async fn count_all(&self) -> Result<i64, DbErr> {
        DeviceEntity::find()
            .filter(DeviceColumn::IsDeleted.eq(0))
            .count(&*self.conn)
            .await
            .map(|count| count as i64)
    }
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    count: i64,
}

fn placeholders(start: usize, count: usize) -> String {
    (start..start + count)
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ")
}
