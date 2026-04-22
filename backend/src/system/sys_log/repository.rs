use crate::common::base::{order_desc, RepoExt, make_condition};
use crate::common::traits::{DynFuture, SeaOrmOptResult, SeaOrmResult, SysLogRepository};
use crate::impl_repo_conn;
use crate::system::sys_log::domain::{
    CreateSysLogRequest, SysLog, SysLogPageQuery, SysLogVO, UpdateSysLogRequest,
};
use crate::system::sys_log::entity::ActiveModel;
use crate::system::sys_log::entity::Column as SysLogColumn;
use crate::system::sys_log::entity::Entity as SysLogEntity;
use async_trait::async_trait;
use chrono::TimeZone;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use std::sync::Arc;

pub struct SeaOrmSysLogRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmSysLogRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl_repo_conn!(SeaOrmSysLogRepository);

#[async_trait]
impl SysLogRepository for SeaOrmSysLogRepository {
    fn create(&self, req: &CreateSysLogRequest, id: &i64) -> DynFuture<SeaOrmResult<SysLog>> {
        let req = req.clone();
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let now = chrono::Utc::now();
                let active_model = req.to_active_model(id, now);

                SysLogEntity::insert(active_model).exec(&*conn).await?;

                let log = SysLogEntity::find_by_id(id).one(&*conn).await?;

                Ok(log.unwrap())
            })
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<SysLog>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let log = SysLogEntity::find_by_id(id)
                    .filter(SysLogColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(log)
            })
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<SysLog>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let logs = SysLogEntity::find()
                    .filter(SysLogColumn::IsDeleted.eq(0))
                    .order_by(SysLogColumn::CreateTime, order_desc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(logs)
            })
        })
    }

    fn find_all_with_page(
        &self,
        req: &SysLogPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<SysLogVO>, i64)>> {
        let req = req.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let base_query = SysLogEntity::find().filter(SysLogColumn::IsDeleted.eq(0));

                let mut cond = make_condition();
                let conditions: Vec<_> = [
                    req.tenant.as_ref().map(|v| SysLogColumn::Tenant.contains(v)),
                    req.type_.as_ref().map(|v| SysLogColumn::Type.contains(v)),
                    req.sub_type
                        .as_ref()
                        .map(|v| SysLogColumn::SubType.contains(v)),
                    req.biz_no.as_ref().map(|v| SysLogColumn::BizNo.contains(v)),
                    req.operator
                        .as_ref()
                        .map(|v| SysLogColumn::Operator.contains(v)),
                    req.action.as_ref().map(|v| SysLogColumn::Action.contains(v)),
                    req.ip.as_ref().map(|v| SysLogColumn::Ip.contains(v)),
                ]
                .into_iter()
                .filter_map(|c| c)
                .collect();
                for c in conditions {
                    cond = cond.add(c);
                }

                // Handle date range filtering
                if let Some(ref begin) = req.begin_time {
                    let begin_dt = chrono::NaiveDateTime::parse_from_str(
                        &format!("{0} 00:00:00", begin),
                        "%Y-%m-%d %H:%M:%S",
                    );
                    if let Ok(dt) = begin_dt {
                        let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
                        let utc_dt = tz.from_local_datetime(&dt).unwrap();
                        cond = cond.add(SysLogColumn::CreateTime.gte(utc_dt));
                    }
                }
                if let Some(ref end) = req.end_time {
                    let end_dt = chrono::NaiveDateTime::parse_from_str(
                        &format!("{0} 23:59:59", end),
                        "%Y-%m-%d %H:%M:%S",
                    );
                    if let Ok(dt) = end_dt {
                        let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
                        let utc_dt = tz.from_local_datetime(&dt).unwrap();
                        cond = cond.add(SysLogColumn::CreateTime.lte(utc_dt));
                    }
                }

                let total = base_query
                    .clone()
                    .filter(cond.clone())
                    .count(&*conn)
                    .await?;

                let offset = (req.page() - 1) * req.size();
                let records = base_query
                    .filter(cond)
                    .order_by(SysLogColumn::CreateTime, order_desc())
                    .offset(Some(offset as u64))
                    .limit(req.size() as u64)
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .map(SysLogVO::from)
                    .collect();

                Ok((records, total as i64))
            })
        })
    }

    fn update(&self, id: &i64, req: &UpdateSysLogRequest) -> DynFuture<SeaOrmOptResult<SysLog>> {
        let req = req.clone();
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let exists = SysLogEntity::find()
                    .filter(SysLogColumn::Id.eq(id))
                    .filter(SysLogColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if exists.is_none() {
                    return Ok(None);
                }

                let active_model = req.to_active_model(id);
                SysLogEntity::update(active_model)
                    .filter(SysLogColumn::Id.eq(id))
                    .filter(SysLogColumn::IsDeleted.eq(0))
                    .exec(&*conn)
                    .await?;

                let log = SysLogEntity::find()
                    .filter(SysLogColumn::Id.eq(id))
                    .filter(SysLogColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(log)
            })
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let log = SysLogEntity::find()
                    .filter(SysLogColumn::Id.eq(id))
                    .filter(SysLogColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if let Some(mut log) = log {
                    log.is_deleted = 1;
                    let mut active_model: ActiveModel = log.into();
                    active_model.is_deleted = ActiveValue::Set(1);
                    SysLogEntity::update(active_model).exec(&*conn).await?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        })
    }
}