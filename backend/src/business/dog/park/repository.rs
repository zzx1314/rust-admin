use crate::business::dog::park::domain::{CreateParkRequest, ParkPageQuery, UpdateParkRequest};
use crate::business::dog::park::entity::ActiveModel as ParkActiveModel;
use crate::business::dog::park::entity::Column as ParkColumn;
use crate::business::dog::park::entity::Entity as ParkEntity;
use crate::business::dog::park::entity::Model as Park;
use crate::common::base::{BaseRepository, order_desc};
use crate::common::data_scope::DataScope;
use crate::common::util::parse_beijing_datetime;
use crate::system::sys_user::entity::Column as UserColumn;
use crate::system::sys_user::entity::Entity as UserEntity;
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogParkRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogParkRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }

    pub async fn create(
        &self,
        req: &CreateParkRequest,
        id: i64,
        scope: &DataScope,
    ) -> Result<Park, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, scope.user_id(), now);
        ParkEntity::insert(active_model).exec(&*self.conn).await?;
        ParkEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_user_names(
        &self,
        user_ids: &[i64],
    ) -> Result<HashMap<i64, String>, sea_orm::DbErr> {
        if user_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let users = UserEntity::find()
            .filter(UserColumn::Id.is_in(user_ids.iter().copied()))
            .all(&*self.conn)
            .await?;
        Ok(users.into_iter().map(|u| (u.id, u.username)).collect())
    }

    pub async fn find_by_id(
        &self,
        id: i64,
        scope: &DataScope,
    ) -> Result<Option<Park>, sea_orm::DbErr> {
        let mut query = ParkEntity::find()
            .filter(ParkColumn::Id.eq(id))
            .filter(ParkColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            query = query.filter(ParkColumn::UserId.eq(owner));
        }
        query.one(&*self.conn).await
    }

    pub async fn find_all_with_page(
        &self,
        query: &ParkPageQuery,
        scope: &DataScope,
    ) -> Result<(Vec<Park>, i64), sea_orm::DbErr> {
        let mut base_query = ParkEntity::find().filter(ParkColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            base_query = base_query.filter(ParkColumn::UserId.eq(owner));
        }

        let mut cond = Condition::all();
        if let Some(ref park_name) = query.park_name {
            cond = cond.add(ParkColumn::ParkName.contains(park_name.as_str()));
        }

        if let Some(ref begin) = query.begin_time
            && let Some(utc_dt) = parse_beijing_datetime(begin, false)
        {
            cond = cond.add(ParkColumn::CreateTime.gte(utc_dt));
        }
        if let Some(ref end) = query.end_time
            && let Some(utc_dt) = parse_beijing_datetime(end, true)
        {
            cond = cond.add(ParkColumn::CreateTime.lte(utc_dt));
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
            .order_by(ParkColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn find_all(&self, scope: &DataScope) -> Result<Vec<Park>, sea_orm::DbErr> {
        let mut query = ParkEntity::find().filter(ParkColumn::IsDeleted.eq(0));
        if let Some(owner) = scope.owner_filter() {
            query = query.filter(ParkColumn::UserId.eq(owner));
        }
        query
            .order_by(ParkColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateParkRequest,
        scope: &DataScope,
    ) -> Result<Option<Park>, sea_orm::DbErr> {
        let entity = self.find_by_id(id, scope).await?;
        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: ParkActiveModel = entity.into();
        if let Some(park_name) = &req.park_name {
            active_model.park_name = ActiveValue::set(park_name.clone());
        }
        if let Some(description) = &req.park_description {
            active_model.park_description = ActiveValue::set(Some(description.clone()));
        }
        if let Some(address) = &req.park_address {
            active_model.park_address = ActiveValue::set(Some(address.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        let mut update = ParkEntity::update(active_model).filter(ParkColumn::Id.eq(id));
        if let Some(owner) = scope.owner_filter() {
            update = update.filter(ParkColumn::UserId.eq(owner));
        }
        update.exec(&*self.conn).await?;
        self.find_by_id(id, scope).await
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<bool, sea_orm::DbErr> {
        let park = self.find_by_id(id, scope).await?;
        let park = match park {
            Some(p) => p,
            None => return Ok(false),
        };

        let mut active_model: ParkActiveModel = park.into();
        active_model.is_deleted = ActiveValue::set(1);
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        let mut update = ParkEntity::update(active_model).filter(ParkColumn::Id.eq(id));
        if let Some(owner) = scope.owner_filter() {
            update = update.filter(ParkColumn::UserId.eq(owner));
        }
        update.exec(&*self.conn).await?;
        Ok(true)
    }
}

impl BaseRepository for SeaOrmDogParkRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}
