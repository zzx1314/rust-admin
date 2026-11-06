use crate::business::dog::route_library::domain::{
    CreateRouteRequest, RoutePageQuery, UpdateRouteRequest,
};
use crate::business::dog::route_library::entity::ActiveModel as RouteActiveModel;
use crate::business::dog::route_library::entity::Column as RouteColumn;
use crate::business::dog::route_library::entity::Entity as RouteEntity;
use crate::business::dog::route_library::entity::Model as DogRoute;
use crate::business::dog::park::entity::Column as ParkColumn;
use crate::business::dog::park::entity::Entity as ParkEntity;
use crate::common::base::{BaseRepository, order_desc};
use crate::common::util::parse_beijing_datetime;
use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogRouteLibraryRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogRouteLibraryRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl BaseRepository for SeaOrmDogRouteLibraryRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmDogRouteLibraryRepository {
    pub async fn create(
        &self,
        req: &CreateRouteRequest,
        id: i64,
    ) -> Result<DogRoute, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        RouteEntity::insert(active_model).exec(&*self.conn).await?;
        RouteEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<DogRoute>, sea_orm::DbErr> {
        RouteEntity::find()
            .filter(RouteColumn::Id.eq(id))
            .filter(RouteColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    /// 按路线引用的 `park_id` 批量取园区名称，用于展示「所属园区」。
    pub async fn find_park_names(
        &self,
        park_ids: &[i64],
    ) -> Result<HashMap<i64, String>, sea_orm::DbErr> {
        if park_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let parks = ParkEntity::find()
            .filter(ParkColumn::Id.is_in(park_ids.iter().copied()))
            .filter(ParkColumn::IsDeleted.eq(0))
            .all(&*self.conn)
            .await?;
        Ok(parks
            .into_iter()
            .map(|park| (park.id, park.park_name))
            .collect())
    }

    pub async fn find_all_with_page(
        &self,
        query: &RoutePageQuery,
    ) -> Result<(Vec<DogRoute>, i64), sea_orm::DbErr> {
        let base_query = RouteEntity::find().filter(RouteColumn::IsDeleted.eq(0));
        let mut cond = Condition::all();
        if let Some(ref name) = query.route_name {
            cond = cond.add(RouteColumn::RouteName.contains(name.as_str()));
        }
        if let Some(ref rtype) = query.route_type {
            cond = cond.add(RouteColumn::RouteType.eq(rtype.as_str()));
        }
        if let Some(ref status) = query.route_status {
            cond = cond.add(RouteColumn::RouteStatus.eq(status.as_str()));
        }
        if let Some(park_id) = query.park_id {
            cond = cond.add(RouteColumn::ParkId.eq(park_id));
        }

        if let Some(ref begin) = query.begin_time
            && let Some(utc_dt) = parse_beijing_datetime(begin, false)
        {
            cond = cond.add(RouteColumn::CreateTime.gte(utc_dt));
        }
        if let Some(ref end) = query.end_time
            && let Some(utc_dt) = parse_beijing_datetime(end, true)
        {
            cond = cond.add(RouteColumn::CreateTime.lte(utc_dt));
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
            .order_by(RouteColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateRouteRequest,
    ) -> Result<Option<DogRoute>, sea_orm::DbErr> {
        let entity = RouteEntity::find()
            .filter(RouteColumn::Id.eq(id))
            .filter(RouteColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: RouteActiveModel = entity.into();
        if let Some(name) = &req.route_name {
            active_model.route_name = ActiveValue::set(name.clone());
        }
        if let Some(rtype) = &req.route_type {
            active_model.route_type = ActiveValue::set(Some(rtype.clone()));
        }
        if let Some(status) = &req.route_status {
            active_model.route_status = ActiveValue::set(Some(status.clone()));
        }
        if let Some(desc) = &req.route_description {
            active_model.route_description = ActiveValue::set(Some(desc.clone()));
        }
        if let Some(park_id) = req.park_id {
            active_model.park_id = ActiveValue::set(Some(park_id));
        }
        if let Some(model_id) = req.model_id {
            active_model.model_id = ActiveValue::set(model_id);
        }
        if let Some(data) = &req.route_data {
            active_model.route_data = ActiveValue::set(Some(data.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        RouteEntity::update(active_model).exec(&*self.conn).await?;
        self.find_by_id(id).await
    }

    pub async fn find_all(&self) -> Result<Vec<DogRoute>, sea_orm::DbErr> {
        RouteEntity::find()
            .filter(RouteColumn::IsDeleted.eq(0))
            .order_by(RouteColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn count(&self) -> Result<i64, sea_orm::DbErr> {
        RouteEntity::find()
            .filter(RouteColumn::IsDeleted.eq(0))
            .count(&*self.conn)
            .await
            .map(|c| c as i64)
    }

    /// Creation timestamps at or after `since`, used by dashboard statistics.
    pub async fn find_create_times_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<Vec<DateTime<Utc>>, sea_orm::DbErr> {
        RouteEntity::find()
            .filter(RouteColumn::IsDeleted.eq(0))
            .filter(RouteColumn::CreateTime.gte(since))
            .select_only()
            .column(RouteColumn::CreateTime)
            .into_tuple::<DateTime<Utc>>()
            .all(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let route = self.find_by_id(id).await?;
        if let Some(mut route) = route {
            route.is_deleted = 1;
            route.update_time = Some(chrono::Utc::now());
            let mut active_model: RouteActiveModel = route.into();
            active_model.is_deleted = ActiveValue::Set(1);
            RouteEntity::update(active_model).exec(&*self.conn).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
