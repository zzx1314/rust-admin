use crate::common::base::{make_condition, order_desc, RepoExt};
use crate::common::traits::{DynFuture, SeaOrmOptResult, SeaOrmResult, SysDictRepository};
use crate::impl_repo_conn;
use crate::sys_dict::domain::{CreateSysDictRequest, SysDict, SysDictPageQuery, SysDictVO, UpdateSysDictRequest};
use crate::sys_dict::entity::ActiveModel;
use crate::sys_dict::entity::Column as SysDictColumn;
use crate::sys_dict::entity::Entity as SysDictEntity;
use async_trait::async_trait;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use std::sync::Arc;

pub struct SeaOrmSysDictRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmSysDictRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl_repo_conn!(SeaOrmSysDictRepository);

#[async_trait]
impl SysDictRepository for SeaOrmSysDictRepository {
    fn create(&self, req: &CreateSysDictRequest, id: &i64) -> DynFuture<SeaOrmResult<SysDict>> {
        let req = req.clone();
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let now = chrono::Utc::now();
                let active_model = req.to_active_model(id, now);

                SysDictEntity::insert(active_model).exec(&*conn).await?;

                let dict = SysDictEntity::find_by_id(id).one(&*conn).await?;

                Ok(dict.unwrap())
            })
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<SysDict>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let dict = SysDictEntity::find_by_id(id)
                    .filter(SysDictColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(dict)
            })
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<SysDict>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let dicts = SysDictEntity::find()
                    .filter(SysDictColumn::IsDeleted.eq(0))
                    .order_by(SysDictColumn::CreateTime, order_desc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(dicts)
            })
        })
    }

    fn find_all_with_page(
        &self,
        req: &SysDictPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<SysDictVO>, i64)>> {
        let req = req.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let base_query = SysDictEntity::find().filter(SysDictColumn::IsDeleted.eq(0));

                let mut cond = make_condition();
                let conditions: Vec<_> = [
                    req.r#type.as_ref().map(|v| SysDictColumn::Type.contains(v)),
                    req.description.as_ref().map(|v| SysDictColumn::Description.contains(v)),
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
                    .order_by(SysDictColumn::CreateTime, order_desc())
                    .offset(Some(offset as u64))
                    .limit(req.size() as u64)
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .map(SysDictVO::from)
                    .collect();

                Ok((records, total as i64))
            })
        })
    }

    fn update(&self, id: &i64, req: &UpdateSysDictRequest) -> DynFuture<SeaOrmOptResult<SysDict>> {
        let req = req.clone();
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let exists = SysDictEntity::find()
                    .filter(SysDictColumn::Id.eq(id))
                    .filter(SysDictColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if exists.is_none() {
                    return Ok(None);
                }

                let active_model = req.to_active_model(id);
                SysDictEntity::update(active_model)
                    .filter(SysDictColumn::Id.eq(id))
                    .filter(SysDictColumn::IsDeleted.eq(0))
                    .exec(&*conn)
                    .await?;

                let dict = SysDictEntity::find()
                    .filter(SysDictColumn::Id.eq(id))
                    .filter(SysDictColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(dict)
            })
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let dict = SysDictEntity::find()
                    .filter(SysDictColumn::Id.eq(id))
                    .filter(SysDictColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if let Some(mut dict) = dict {
                    dict.is_deleted = 1;
                    dict.update_time = Some(chrono::Utc::now());
                    let mut active_model: ActiveModel = dict.into();
                    active_model.is_deleted = ActiveValue::Set(1);
                    SysDictEntity::update(active_model).exec(&*conn).await?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        })
    }
}