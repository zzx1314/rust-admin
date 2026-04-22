use crate::common::base::{RepoExt, make_condition, order_asc, order_desc};
use crate::common::traits::{DynFuture, SeaOrmOptResult, SeaOrmResult, SysDictItemRepository};
use crate::impl_repo_conn;
use crate::system::sys_dict_item::domain::{
    CreateSysDictItemRequest, SysDictItem, SysDictItemPageQuery, SysDictItemVO,
    UpdateSysDictItemRequest,
};
use crate::system::sys_dict_item::entity::ActiveModel;
use crate::system::sys_dict_item::entity::Column as SysDictItemColumn;
use crate::system::sys_dict_item::entity::Entity as SysDictItemEntity;
use async_trait::async_trait;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use std::sync::Arc;

pub struct SeaOrmSysDictItemRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmSysDictItemRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl_repo_conn!(SeaOrmSysDictItemRepository);

#[async_trait]
impl SysDictItemRepository for SeaOrmSysDictItemRepository {
    fn create(
        &self,
        req: &CreateSysDictItemRequest,
        id: &i64,
    ) -> DynFuture<SeaOrmResult<SysDictItem>> {
        let req = req.clone();
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let now = chrono::Utc::now();
                let active_model = req.to_active_model(id, now);

                SysDictItemEntity::insert(active_model).exec(&*conn).await?;

                let item = SysDictItemEntity::find_by_id(id).one(&*conn).await?;

                Ok(item.unwrap())
            })
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<SysDictItem>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let item = SysDictItemEntity::find_by_id(id)
                    .filter(SysDictItemColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(item)
            })
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<SysDictItem>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let items = SysDictItemEntity::find()
                    .filter(SysDictItemColumn::IsDeleted.eq(0))
                    .order_by(SysDictItemColumn::Sort, order_asc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(items)
            })
        })
    }

    fn find_by_dict_id(&self, dict_id: &i64) -> DynFuture<SeaOrmResult<Vec<SysDictItem>>> {
        let dict_id = *dict_id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let items = SysDictItemEntity::find()
                    .filter(SysDictItemColumn::DictId.eq(dict_id))
                    .filter(SysDictItemColumn::IsDeleted.eq(0))
                    .order_by(SysDictItemColumn::Sort, order_asc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(items)
            })
        })
    }

    fn find_by_type(&self, r#type: &str) -> DynFuture<SeaOrmResult<Vec<SysDictItem>>> {
        let r#type = r#type.to_string();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let items = SysDictItemEntity::find()
                    .filter(SysDictItemColumn::Type.eq(r#type))
                    .filter(SysDictItemColumn::IsDeleted.eq(0))
                    .order_by(SysDictItemColumn::Sort, order_asc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(items)
            })
        })
    }

    fn find_all_with_page(
        &self,
        req: &SysDictItemPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<SysDictItemVO>, i64)>> {
        let req = req.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let base_query =
                    SysDictItemEntity::find().filter(SysDictItemColumn::IsDeleted.eq(0));

                let mut cond = make_condition();
                let conditions: Vec<_> = [
                    req.dict_id.map(|v| SysDictItemColumn::DictId.eq(v)),
                    req.r#type
                        .as_ref()
                        .map(|v| SysDictItemColumn::Type.contains(v)),
                    req.label
                        .as_ref()
                        .map(|v| SysDictItemColumn::Label.contains(v)),
                ]
                .into_iter()
                .filter_map(|c| c)
                .collect();
                for c in conditions {
                    cond = cond.add(c);
                }

                let total = base_query
                    .clone()
                    .filter(cond.clone())
                    .count(&*conn)
                    .await?;

                let offset = (req.page() - 1) * req.size();
                let records = base_query
                    .filter(cond)
                    .order_by(SysDictItemColumn::Sort, order_asc())
                    .order_by(SysDictItemColumn::CreateTime, order_desc())
                    .offset(Some(offset as u64))
                    .limit(req.size() as u64)
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .map(SysDictItemVO::from)
                    .collect();

                Ok((records, total as i64))
            })
        })
    }

    fn update(
        &self,
        id: &i64,
        req: &UpdateSysDictItemRequest,
    ) -> DynFuture<SeaOrmOptResult<SysDictItem>> {
        let req = req.clone();
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let exists = SysDictItemEntity::find()
                    .filter(SysDictItemColumn::Id.eq(id))
                    .filter(SysDictItemColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if exists.is_none() {
                    return Ok(None);
                }

                let active_model = req.to_active_model(id);
                SysDictItemEntity::update(active_model)
                    .filter(SysDictItemColumn::Id.eq(id))
                    .filter(SysDictItemColumn::IsDeleted.eq(0))
                    .exec(&*conn)
                    .await?;

                let item = SysDictItemEntity::find()
                    .filter(SysDictItemColumn::Id.eq(id))
                    .filter(SysDictItemColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(item)
            })
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let item = SysDictItemEntity::find()
                    .filter(SysDictItemColumn::Id.eq(id))
                    .filter(SysDictItemColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if let Some(mut item) = item {
                    item.is_deleted = 1;
                    item.update_time = Some(chrono::Utc::now());
                    let mut active_model: ActiveModel = item.into();
                    active_model.is_deleted = ActiveValue::Set(1);
                    SysDictItemEntity::update(active_model).exec(&*conn).await?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        })
    }
}
