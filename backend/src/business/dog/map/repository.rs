use crate::business::dog::map::domain::{CreateMapRequest, MapPageQuery, UpdateMapRequest};
use crate::business::dog::map::entity::ActiveModel as MapActiveModel;
use crate::business::dog::map::entity::Column as MapColumn;
use crate::business::dog::map::entity::Entity as MapEntity;
use crate::business::dog::map::entity::Model as Map;
use crate::business::dog::map::share_entity::ActiveModel as MapShareActiveModel;
use crate::business::dog::map::share_entity::Column as MapShareColumn;
use crate::business::dog::map::share_entity::Entity as MapShareEntity;
use crate::business::dog::map::share_entity::Model as MapShare;
use crate::common::base::{BaseRepository, order_asc, order_desc};
use crate::common::data_scope::DataScope;
use crate::common::util::{generate_id, parse_beijing_datetime};
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DbBackend,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Statement,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogMapRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogMapRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }

    pub async fn create(
        &self,
        req: &CreateMapRequest,
        id: i64,
        scope: &DataScope,
    ) -> Result<Map, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, scope.user_id(), now);
        MapEntity::insert(active_model).exec(&*self.conn).await?;
        MapEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(
        &self,
        id: i64,
        scope: &DataScope,
    ) -> Result<Option<Map>, sea_orm::DbErr> {
        let mut query = MapEntity::find()
            .filter(MapColumn::Id.eq(id))
            .filter(MapColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            query = query.filter(MapColumn::UserId.eq(owner));
        }
        query.one(&*self.conn).await
    }

    /// 读接口的可见范围：自己的地图 + 自己管辖设备上报的地图 + 分享给自己管辖设备的其他地图。
    ///
    /// 写接口（改 / 删 / 发版 / 擦除 / 覆盖地图包）仍然只认 [`Self::find_by_id`]，
    /// 分享出去的地图不因此变成别人可编辑的。
    pub async fn find_visible_by_id(
        &self,
        id: i64,
        scope: &DataScope,
    ) -> Result<Option<Map>, sea_orm::DbErr> {
        let map = MapEntity::find()
            .filter(MapColumn::Id.eq(id))
            .filter(MapColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;
        let Some(map) = map else {
            return Ok(None);
        };
        if let Some(owner) = scope.owner_filter()
            && !self.is_visible_to_user(&map, owner).await?
        {
            return Ok(None);
        }
        Ok(Some(map))
    }

    /// 普通用户的可见条件：自己的地图，自己管辖设备上报的地图，
    /// 或分享给了分配给自己的设备的地图。
    ///
    /// 「自己管辖设备上报的地图」这一条不能少：地图的 `user_id` 只记一个归属人
    /// （业务平台回传时取第一个被分配的用户），同一台狗绑定多个人时，其他人在
    /// 地图列表里会一张都看不到。地图跟着设备走，绑定关系才是一等公民。
    async fn visibility_condition(&self, owner: i64) -> Result<Condition, sea_orm::DbErr> {
        let mut condition = Condition::any().add(MapColumn::UserId.eq(owner));
        let device_codes = self.find_assigned_device_codes(owner).await?;
        if !device_codes.is_empty() {
            condition = condition.add(MapColumn::DeviceCode.is_in(device_codes));
        }
        let shared = self.find_shared_map_ids_for_user(owner).await?;
        if !shared.is_empty() {
            condition = condition.add(MapColumn::Id.is_in(shared));
        }
        Ok(condition)
    }

    pub async fn find_all_with_page(
        &self,
        query: &MapPageQuery,
        scope: &DataScope,
    ) -> Result<(Vec<Map>, i64), sea_orm::DbErr> {
        let mut base_query = MapEntity::find().filter(MapColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            base_query = base_query.filter(self.visibility_condition(owner).await?);
        }

        let mut cond = Condition::all();
        if let Some(ref map_name) = query.map_name {
            cond = cond.add(MapColumn::MapName.contains(map_name.as_str()));
        }
        if let Some(ref file_name) = query.file_name {
            cond = cond.add(MapColumn::FileName.contains(file_name.as_str()));
        }
        if let Some(ref park_code) = query.park_code
            && !park_code.trim().is_empty()
        {
            cond = cond.add(MapColumn::ParkCode.eq(park_code.trim()));
        }
        if let Some(ref device_code) = query.device_code
            && !device_code.trim().is_empty()
        {
            let code = device_code.trim();
            // 设备编号既匹配「上报设备」，也匹配「分享给这台设备」：
            // 从设备视角看，两者都是这台设备能用的图。
            let shared_ids = self.find_map_ids_shared_to_device(code).await?;
            let mut device_cond = Condition::any().add(MapColumn::DeviceCode.contains(code));
            if !shared_ids.is_empty() {
                device_cond = device_cond.add(MapColumn::Id.is_in(shared_ids));
            }
            cond = cond.add(device_cond);
        }
        if let Some(ref status) = query.map_status
            && !status.trim().is_empty()
        {
            cond = cond.add(MapColumn::MapStatus.eq(status.as_str()));
        }

        if let Some(ref begin) = query.begin_time
            && let Some(utc_dt) = parse_beijing_datetime(begin, false)
        {
            cond = cond.add(MapColumn::CreateTime.gte(utc_dt));
        }
        if let Some(ref end) = query.end_time
            && let Some(utc_dt) = parse_beijing_datetime(end, true)
        {
            cond = cond.add(MapColumn::CreateTime.lte(utc_dt));
        }

        let total = base_query
            .clone()
            .filter(cond.clone())
            .count(&*self.conn)
            .await?;

        let current = query.current.max(1);
        let size = query.size.max(1);
        let offset = (current - 1) * size;

        let records = base_query
            .filter(cond)
            .order_by(MapColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn find_all(&self, scope: &DataScope) -> Result<Vec<Map>, sea_orm::DbErr> {
        let mut query = MapEntity::find().filter(MapColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            query = query.filter(self.visibility_condition(owner).await?);
        }
        query
            .order_by(MapColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateMapRequest,
        scope: &DataScope,
    ) -> Result<Option<Map>, sea_orm::DbErr> {
        let entity = self.find_by_id(id, scope).await?;
        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: MapActiveModel = entity.into();
        if let Some(map_name) = &req.map_name {
            active_model.map_name = ActiveValue::set(map_name.clone());
        }
        if let Some(map_type) = &req.map_type {
            active_model.map_type = ActiveValue::set(Some(map_type.clone()));
        }
        if let Some(park_code) = &req.park_code {
            let trimmed = park_code.trim();
            active_model.park_code =
                ActiveValue::set((!trimmed.is_empty()).then(|| trimmed.to_string()));
        }
        if let Some(park_name) = &req.park_name {
            let trimmed = park_name.trim();
            active_model.park_name =
                ActiveValue::set((!trimmed.is_empty()).then(|| trimmed.to_string()));
        }
        if let Some(device_code) = &req.device_code {
            let trimmed = device_code.trim();
            active_model.device_code =
                ActiveValue::set((!trimmed.is_empty()).then(|| trimmed.to_string()));
        }
        if let Some(map_data) = &req.map_data {
            active_model.map_data = ActiveValue::set(Some(map_data.clone()));
        }
        if let Some(file_name) = &req.file_name {
            active_model.file_name = ActiveValue::set(Some(file_name.clone()));
        }
        if let Some(file_path) = &req.file_path {
            active_model.file_path = ActiveValue::set(Some(file_path.clone()));
        }
        if let Some(file_format) = &req.file_format {
            active_model.file_format = ActiveValue::set(Some(file_format.clone()));
        }
        if let Some(file_size) = req.file_size {
            active_model.file_size = ActiveValue::set(Some(file_size));
        }
        if let Some(remarks) = &req.remarks {
            active_model.remarks = ActiveValue::set(Some(remarks.clone()));
        }
        if let Some(map_version) = &req.map_version {
            active_model.map_version = ActiveValue::set(Some(map_version.clone()));
        }
        if let Some(frame_id) = &req.frame_id {
            active_model.frame_id = ActiveValue::set(Some(frame_id.clone()));
        }
        if let Some(crs) = &req.crs {
            active_model.crs = ActiveValue::set(Some(crs.clone()));
        }
        if let Some(origin) = &req.map_origin {
            active_model.map_origin = ActiveValue::set(Some(origin.clone()));
        }
        if let Some(resolution) = &req.resolution {
            active_model.resolution = ActiveValue::set(Some(resolution.clone()));
        }
        if let Some(floors) = &req.floors {
            active_model.floors = ActiveValue::set(Some(floors.clone()));
        }
        if let Some(road_count) = req.road_count {
            active_model.road_count = ActiveValue::set(Some(road_count));
        }
        if let Some(nogo_count) = req.nogo_count {
            active_model.nogo_count = ActiveValue::set(Some(nogo_count));
        }
        if let Some(map_status) = &req.map_status {
            active_model.map_status = ActiveValue::set(map_status.clone());
        }
        if let Some(map_hash) = &req.map_hash {
            active_model.map_hash = ActiveValue::set(Some(map_hash.clone()));
        }
        if let Some(map_width) = req.map_width {
            active_model.map_width = ActiveValue::set(Some(map_width));
        }
        if let Some(map_height) = req.map_height {
            active_model.map_height = ActiveValue::set(Some(map_height));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        let mut update = MapEntity::update(active_model).filter(MapColumn::Id.eq(id));
        if let Some(owner) = scope.owner_filter() {
            update = update.filter(MapColumn::UserId.eq(owner));
        }
        update.exec(&*self.conn).await?;
        self.find_by_id(id, scope).await
    }

    /// 全量地图版本数量（首页统计，管理员视角）。
    pub async fn count(&self) -> Result<i64, sea_orm::DbErr> {
        MapEntity::find()
            .filter(MapColumn::IsDeleted.eq(0))
            .count(&*self.conn)
            .await
            .map(|c| c as i64)
    }

    /// Creation timestamps at or after `since`, used by dashboard statistics.
    pub async fn find_create_times_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<DateTime<Utc>>, sea_orm::DbErr> {
        MapEntity::find()
            .filter(MapColumn::IsDeleted.eq(0))
            .filter(MapColumn::CreateTime.gte(since))
            .select_only()
            .column(MapColumn::CreateTime)
            .into_tuple::<DateTime<Utc>>()
            .all(&*self.conn)
            .await
    }

    /// 给定 id 中仍存在（未被软删除）的地图 id。
    ///
    /// 业务平台用它判断「业务库里已导入、但中台被删掉」的地图：这类地图不能算
    /// 已同步，否则设备端不会再上传，中台列表里也永远看不到这张图。
    pub async fn find_alive_ids(&self, ids: &[i64]) -> Result<Vec<i64>, sea_orm::DbErr> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        MapEntity::find()
            .filter(MapColumn::Id.is_in(ids.iter().copied()))
            .filter(MapColumn::IsDeleted.eq(0))
            .select_only()
            .column(MapColumn::Id)
            .into_tuple::<i64>()
            .all(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<bool, sea_orm::DbErr> {
        let map = self.find_by_id(id, scope).await?;
        let map = match map {
            Some(m) => m,
            None => return Ok(false),
        };

        let mut active_model: MapActiveModel = map.into();
        active_model.is_deleted = ActiveValue::set(1);
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        let mut update = MapEntity::update(active_model).filter(MapColumn::Id.eq(id));
        if let Some(owner) = scope.owner_filter() {
            update = update.filter(MapColumn::UserId.eq(owner));
        }
        update.exec(&*self.conn).await?;
        // 地图删了，分享关系一并收起（软删；重新建同名 id 不会撞唯一索引）
        self.delete_shares(id).await?;
        Ok(true)
    }

    /// 更新版本状态；`publish_time` 为 `None` 时不改写发布时间。
    pub async fn update_status(
        &self,
        id: i64,
        status: &str,
        publish_time: Option<String>,
        scope: &DataScope,
    ) -> Result<Option<Map>, sea_orm::DbErr> {
        let entity = self.find_by_id(id, scope).await?;
        let Some(entity) = entity else {
            return Ok(None);
        };
        let mut active_model: MapActiveModel = entity.into();
        active_model.map_status = ActiveValue::set(status.to_string());
        if publish_time.is_some() {
            active_model.publish_time = ActiveValue::set(publish_time);
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        let mut update = MapEntity::update(active_model).filter(MapColumn::Id.eq(id));
        if let Some(owner) = scope.owner_filter() {
            update = update.filter(MapColumn::UserId.eq(owner));
        }
        update.exec(&*self.conn).await?;
        self.find_by_id(id, scope).await
    }

    /// 当前分享给了哪些设备（按分享时间排序）。
    pub async fn find_shares(&self, map_id: i64) -> Result<Vec<MapShare>, sea_orm::DbErr> {
        MapShareEntity::find()
            .filter(MapShareColumn::MapId.eq(map_id))
            .filter(MapShareColumn::IsDeleted.eq(0))
            .order_by(MapShareColumn::CreateTime, order_asc())
            .all(&*self.conn)
            .await
    }

    pub async fn find_share_codes(&self, map_id: i64) -> Result<Vec<String>, sea_orm::DbErr> {
        Ok(self
            .find_shares(map_id)
            .await?
            .into_iter()
            .map(|share| share.device_code)
            .collect())
    }

    /// 一次取多张地图的分享设备，避免列表页 N+1。
    pub async fn find_share_codes_for_maps(
        &self,
        ids: &[i64],
    ) -> Result<HashMap<i64, Vec<String>>, sea_orm::DbErr> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = MapShareEntity::find()
            .filter(MapShareColumn::MapId.is_in(ids.iter().copied()))
            .filter(MapShareColumn::IsDeleted.eq(0))
            .order_by(MapShareColumn::CreateTime, order_asc())
            .all(&*self.conn)
            .await?;
        let mut grouped: HashMap<i64, Vec<String>> = HashMap::new();
        for row in rows {
            grouped.entry(row.map_id).or_default().push(row.device_code);
        }
        Ok(grouped)
    }

    /// 分享给一台设备；已分享（含软删除后重新分享）不重复插入。
    ///
    /// 返回 `true` 表示本次真的写入 / 恢复了记录。
    pub async fn add_share(
        &self,
        map_id: i64,
        device_code: &str,
        user_id: i64,
    ) -> Result<bool, sea_orm::DbErr> {
        let existing = MapShareEntity::find()
            .filter(MapShareColumn::MapId.eq(map_id))
            .filter(MapShareColumn::DeviceCode.eq(device_code))
            .one(&*self.conn)
            .await?;
        let now = Utc::now();
        match existing {
            Some(share) if share.is_deleted == 0 => Ok(false),
            Some(share) => {
                let mut active_model: MapShareActiveModel = share.into();
                active_model.user_id = ActiveValue::set(Some(user_id));
                active_model.update_time = ActiveValue::set(Some(now));
                active_model.is_deleted = ActiveValue::set(0);
                MapShareEntity::update(active_model)
                    .exec(&*self.conn)
                    .await?;
                Ok(true)
            }
            None => {
                MapShareEntity::insert(MapShareActiveModel {
                    id: ActiveValue::set(generate_id()),
                    map_id: ActiveValue::set(map_id),
                    device_code: ActiveValue::set(device_code.to_string()),
                    user_id: ActiveValue::set(Some(user_id)),
                    create_time: ActiveValue::set(now),
                    update_time: ActiveValue::set(Some(now)),
                    is_deleted: ActiveValue::set(0),
                })
                .exec(&*self.conn)
                .await?;
                Ok(true)
            }
        }
    }

    /// 取消分享一台设备，返回实际软删除的条数。
    pub async fn remove_share(
        &self,
        map_id: i64,
        device_code: &str,
    ) -> Result<u64, sea_orm::DbErr> {
        let rows = MapShareEntity::find()
            .filter(MapShareColumn::MapId.eq(map_id))
            .filter(MapShareColumn::DeviceCode.eq(device_code))
            .filter(MapShareColumn::IsDeleted.eq(0))
            .all(&*self.conn)
            .await?;
        let mut removed = 0;
        for row in rows {
            let mut active_model: MapShareActiveModel = row.into();
            active_model.is_deleted = ActiveValue::set(1);
            active_model.update_time = ActiveValue::set(Some(Utc::now()));
            MapShareEntity::update(active_model)
                .exec(&*self.conn)
                .await?;
            removed += 1;
        }
        Ok(removed)
    }

    /// `replace` 语义：移除非 `keep` 里的分享。
    pub async fn remove_shares_not_in(
        &self,
        map_id: i64,
        keep: &[String],
    ) -> Result<u64, sea_orm::DbErr> {
        let rows = MapShareEntity::find()
            .filter(MapShareColumn::MapId.eq(map_id))
            .filter(MapShareColumn::IsDeleted.eq(0))
            .all(&*self.conn)
            .await?;
        let mut removed = 0;
        for row in rows {
            if keep.iter().any(|code| code == &row.device_code) {
                continue;
            }
            let mut active_model: MapShareActiveModel = row.into();
            active_model.is_deleted = ActiveValue::set(1);
            active_model.update_time = ActiveValue::set(Some(Utc::now()));
            MapShareEntity::update(active_model)
                .exec(&*self.conn)
                .await?;
            removed += 1;
        }
        Ok(removed)
    }

    /// 清空一张地图的全部分享（地图删除时调用）。
    pub async fn delete_shares(&self, map_id: i64) -> Result<u64, sea_orm::DbErr> {
        self.remove_shares_not_in(map_id, &[]).await
    }

    /// 分享给「分配给该用户」的设备的地图 id（普通用户可见范围）。
    async fn find_shared_map_ids_for_user(&self, user_id: i64) -> Result<Vec<i64>, sea_orm::DbErr> {
        let rows = self
            .conn
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT DISTINCT s.map_id AS map_id FROM dog_map_share s \
                 JOIN dog_device d ON d.device_id = s.device_code AND d.is_deleted = 0 \
                 JOIN dog_user_device ud ON ud.device_id = d.id AND ud.is_deleted = 0 \
                 WHERE s.is_deleted = 0 AND ud.user_id = ?1",
                [user_id.into()],
            ))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row.try_get::<i64>("", "map_id").ok())
            .collect())
    }

    /// 这张地图对 `user_id` 是否可见：
    /// 自己的图 / 自己管辖设备上报的图 / 分享给自己管辖设备的图。
    async fn is_visible_to_user(&self, map: &Map, user_id: i64) -> Result<bool, sea_orm::DbErr> {
        if map.user_id == user_id {
            return Ok(true);
        }
        if let Some(device_code) = map.device_code.as_deref()
            && self
                .is_device_assigned_to_user(device_code, user_id)
                .await?
        {
            return Ok(true);
        }
        self.is_shared_to_user(map.id, user_id).await
    }

    /// 分配给该用户的设备编号（地图跟着设备走：同一台狗绑定多人时，每个绑定人都能看图）。
    async fn find_assigned_device_codes(
        &self,
        user_id: i64,
    ) -> Result<Vec<String>, sea_orm::DbErr> {
        let rows = self
            .conn
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT DISTINCT d.device_id AS device_code FROM dog_device d \
                 JOIN dog_user_device ud ON ud.device_id = d.id AND ud.is_deleted = 0 \
                 WHERE d.is_deleted = 0 AND ud.user_id = ?1",
                [user_id.into()],
            ))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row.try_get::<String>("", "device_code").ok())
            .collect())
    }

    /// 这台设备是否分配给了 `user_id`。
    async fn is_device_assigned_to_user(
        &self,
        device_code: &str,
        user_id: i64,
    ) -> Result<bool, sea_orm::DbErr> {
        let row = self
            .conn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT 1 FROM dog_device d \
                 JOIN dog_user_device ud ON ud.device_id = d.id AND ud.is_deleted = 0 \
                 WHERE d.is_deleted = 0 AND d.device_id = ?1 AND ud.user_id = ?2 LIMIT 1",
                [device_code.into(), user_id.into()],
            ))
            .await?;
        Ok(row.is_some())
    }

    /// 这张地图是否分享给了分配给 `user_id` 的设备。
    async fn is_shared_to_user(&self, map_id: i64, user_id: i64) -> Result<bool, sea_orm::DbErr> {
        let row = self
            .conn
            .query_one(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT 1 FROM dog_map_share s \
                 JOIN dog_device d ON d.device_id = s.device_code AND d.is_deleted = 0 \
                 JOIN dog_user_device ud ON ud.device_id = d.id AND ud.is_deleted = 0 \
                 WHERE s.is_deleted = 0 AND s.map_id = ?1 AND ud.user_id = ?2 LIMIT 1",
                [map_id.into(), user_id.into()],
            ))
            .await?;
        Ok(row.is_some())
    }

    async fn find_map_ids_shared_to_device(
        &self,
        device_code: &str,
    ) -> Result<Vec<i64>, sea_orm::DbErr> {
        let rows = self
            .conn
            .query_all(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "SELECT DISTINCT map_id AS map_id FROM dog_map_share \
                 WHERE is_deleted = 0 AND device_code = ?1",
                [device_code.into()],
            ))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| row.try_get::<i64>("", "map_id").ok())
            .collect())
    }
}

impl BaseRepository for SeaOrmDogMapRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}
