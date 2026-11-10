use crate::business::dog::map::domain::{CreateMapRequest, MapPageQuery, UpdateMapRequest};
use crate::business::dog::map::entity::ActiveModel as MapActiveModel;
use crate::business::dog::map::entity::Column as MapColumn;
use crate::business::dog::map::entity::Entity as MapEntity;
use crate::business::dog::map::entity::Model as Map;
use crate::common::base::{BaseRepository, order_desc};
use crate::common::data_scope::DataScope;
use crate::common::util::parse_beijing_datetime;
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
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

    pub async fn find_all_with_page(
        &self,
        query: &MapPageQuery,
        scope: &DataScope,
    ) -> Result<(Vec<Map>, i64), sea_orm::DbErr> {
        let mut base_query = MapEntity::find().filter(MapColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            base_query = base_query.filter(MapColumn::UserId.eq(owner));
        }

        let mut cond = Condition::all();
        if let Some(ref map_name) = query.map_name {
            cond = cond.add(MapColumn::MapName.contains(map_name.as_str()));
        }
        if let Some(ref file_name) = query.file_name {
            cond = cond.add(MapColumn::FileName.contains(file_name.as_str()));
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
            query = query.filter(MapColumn::UserId.eq(owner));
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
}

impl BaseRepository for SeaOrmDogMapRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}
