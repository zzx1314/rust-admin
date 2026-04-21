use crate::common::base::{RepoExt, make_condition, order_asc, order_desc};
use crate::common::traits::{DynFuture, OrgRepository, SeaOrmOptResult, SeaOrmResult};
use crate::impl_repo_conn;
use crate::org::domain::{CreateOrgRequest, Org, OrgTreeQuery, UpdateOrgRequest};
use crate::org::entity::ActiveModel as OrgActiveModel;
use crate::org::entity::Column as OrgColumn;
use crate::org::entity::Entity as OrgEntity;
use async_trait::async_trait;
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use std::sync::Arc;

pub struct SeaOrmOrgRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmOrgRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl_repo_conn!(SeaOrmOrgRepository);

#[async_trait]
impl OrgRepository for SeaOrmOrgRepository {
    fn create(&self, req: &CreateOrgRequest, id: &i64) -> DynFuture<SeaOrmResult<Org>> {
        let req = req.clone();
        let id = *id;

        self.with_conn(move |conn| {
            Box::pin(async move {
                let now = chrono::Utc::now();
                let active_model = req.to_active_model(id, now);

                OrgEntity::insert(active_model).exec(&*conn).await?;

                let org = OrgEntity::find_by_id(id).one(&*conn).await?;

                Ok(org.unwrap())
            })
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Org>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let org = OrgEntity::find()
                    .filter(OrgColumn::Id.eq(id))
                    .filter(OrgColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(org)
            })
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Org>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let orgs = OrgEntity::find()
                    .filter(OrgColumn::IsDeleted.eq(0))
                    .order_by(OrgColumn::Sort, order_asc())
                    .order_by(OrgColumn::CreateTime, order_desc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(orgs)
            })
        })
    }

    fn find_by_parent_id(&self, parent_id: Option<i64>) -> DynFuture<SeaOrmResult<Vec<Org>>> {
        self.with_conn(move |conn| {
            Box::pin(async move {
                let mut cond = make_condition().add(OrgColumn::IsDeleted.eq(0));

                if let Some(pid) = parent_id {
                    cond = cond.add(OrgColumn::ParentId.eq(pid));
                } else {
                    cond = cond.add(OrgColumn::ParentId.is_null());
                }

                let orgs = OrgEntity::find()
                    .filter(cond)
                    .order_by(OrgColumn::Sort, order_asc())
                    .order_by(OrgColumn::CreateTime, order_desc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(orgs)
            })
        })
    }

    fn find_tree(&self) -> DynFuture<SeaOrmResult<Vec<Org>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let orgs = OrgEntity::find()
                    .filter(OrgColumn::IsDeleted.eq(0))
                    .order_by(OrgColumn::Sort, order_asc())
                    .order_by(OrgColumn::CreateTime, order_desc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(orgs)
            })
        })
    }

    fn find_tree_with_filter(&self, query: &OrgTreeQuery) -> DynFuture<SeaOrmResult<Vec<Org>>> {
        let query = query.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let base_query = OrgEntity::find().filter(OrgColumn::IsDeleted.eq(0));

                let has_filter = query.name.is_some() || query.r#type.is_some();

                if !has_filter {
                    let orgs = base_query
                        .order_by(OrgColumn::Sort, order_asc())
                        .order_by(OrgColumn::CreateTime, order_desc())
                        .all(&*conn)
                        .await?
                        .into_iter()
                        .collect();
                    return Ok(orgs);
                }

                let mut cond = make_condition();
                if let Some(ref name) = query.name {
                    cond = cond.add(OrgColumn::Name.contains(name));
                }
                if let Some(ref r#type) = query.r#type {
                    cond = cond.add(OrgColumn::Type.eq(r#type));
                }

                let orgs = base_query
                    .filter(cond)
                    .order_by(OrgColumn::Sort, order_asc())
                    .order_by(OrgColumn::CreateTime, order_desc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(orgs)
            })
        })
    }

    fn update(&self, id: &i64, req: &UpdateOrgRequest) -> DynFuture<SeaOrmOptResult<Org>> {
        let id = *id;
        let req = req.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let exists = OrgEntity::find()
                    .filter(OrgColumn::Id.eq(id))
                    .filter(OrgColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if exists.is_none() {
                    return Ok(None);
                }

                let active_model = req.to_active_model(id);
                OrgEntity::update(active_model)
                    .filter(OrgColumn::Id.eq(id))
                    .filter(OrgColumn::IsDeleted.eq(0))
                    .exec(&*conn)
                    .await?;

                let org = OrgEntity::find()
                    .filter(OrgColumn::Id.eq(id))
                    .filter(OrgColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(org)
            })
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let org = OrgEntity::find()
                    .filter(OrgColumn::Id.eq(id))
                    .filter(OrgColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if let Some(mut org) = org {
                    org.is_deleted = 1;
                    org.update_time = chrono::Utc::now();
                    let mut active_model: OrgActiveModel = org.into();
                    active_model.is_deleted = ActiveValue::Set(1);
                    OrgEntity::update(active_model).exec(&*conn).await?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        })
    }
}
