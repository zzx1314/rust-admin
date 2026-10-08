use crate::common::base::{RepoExt, make_condition, order_desc};
use crate::common::traits::{DynFuture, SeaOrmOptResult, SeaOrmResult, UserRepository};
use crate::impl_repo_conn;
use crate::system::sys_role::user_role::Column as UserRoleColumn;
use crate::system::sys_role::user_role::Entity as UserRoleEntity;
use crate::system::sys_user::domain::{
    CreateUserRequest, UpdateUserRequest, User, UserPageQuery, UserVO,
};
use crate::system::sys_user::entity::ActiveModel;
use crate::system::sys_user::entity::Column as UserColumn;
use crate::system::sys_user::entity::Entity as UserEntity;
use async_trait::async_trait;
use chrono::TimeZone;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use std::sync::Arc;

pub struct SeaOrmUserRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmUserRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl_repo_conn!(SeaOrmUserRepository);

#[async_trait]
impl UserRepository for SeaOrmUserRepository {
    fn create(&self, req: &CreateUserRequest, id: &i64) -> DynFuture<SeaOrmResult<User>> {
        let req = req.clone();
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let now = chrono::Utc::now();
                let active_model = req.to_active_model(id, now);

                UserEntity::insert(active_model).exec(&*conn).await?;

                let user = UserEntity::find_by_id(id).one(&*conn).await?;

                Ok(user.unwrap())
            })
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<User>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let user = UserEntity::find_by_id(id)
                    .filter(UserColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(user)
            })
        })
    }

    fn find_by_email(&self, email: &str) -> DynFuture<SeaOrmOptResult<User>> {
        let email = email.to_string();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let user = UserEntity::find()
                    .filter(UserColumn::Email.eq(email))
                    .filter(UserColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(user)
            })
        })
    }

    fn find_by_username(&self, username: &str) -> DynFuture<SeaOrmOptResult<User>> {
        let username = username.to_string();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let user = UserEntity::find()
                    .filter(UserColumn::Username.eq(username))
                    .filter(UserColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(user)
            })
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<User>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let users = UserEntity::find()
                    .filter(UserColumn::IsDeleted.eq(0))
                    .order_by(UserColumn::CreateTime, order_desc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(users)
            })
        })
    }

    fn find_all_with_page(
        &self,
        req: &UserPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<UserVO>, i64)>> {
        let req = req.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let base_query = UserEntity::find().filter(UserColumn::IsDeleted.eq(0));

                let mut cond = make_condition();
                let conditions: Vec<_> = [
                    req.username
                        .as_ref()
                        .map(|v| UserColumn::Username.contains(v)),
                    req.real_name
                        .as_ref()
                        .map(|v| UserColumn::RealName.contains(v)),
                    req.phone.as_ref().map(|v| UserColumn::Phone.contains(v)),
                    req.email.as_ref().map(|v| UserColumn::Email.contains(v)),
                    req.enable.map(|v| UserColumn::Enable.eq(v)),
                ]
                .into_iter()
                .flatten()
                .collect();
                for c in conditions {
                    cond = cond.add(c);
                }

                let org_ids: Vec<i64> = req
                    .org_ids
                    .as_deref()
                    .or(req.org_id.as_deref())
                    .map(parse_id_list)
                    .unwrap_or_default();
                if org_ids.len() == 1 {
                    cond = cond.add(UserColumn::OrgId.eq(org_ids[0]));
                } else if !org_ids.is_empty() {
                    cond = cond.add(UserColumn::OrgId.is_in(org_ids));
                }

                if let Some(role_id) = req.role {
                    let user_roles = UserRoleEntity::find()
                        .filter(UserRoleColumn::RoleId.eq(role_id))
                        .all(&*conn)
                        .await?;
                    let user_ids: Vec<i64> = user_roles.into_iter().map(|ur| ur.user_id).collect();
                    if user_ids.is_empty() {
                        return Ok((vec![], 0));
                    }
                    cond = cond.add(UserColumn::Id.is_in(user_ids));
                }

                if let Some(ref begin) = req.begin_time
                    && let Some(utc_dt) = parse_beijing_datetime(begin, false)
                {
                    cond = cond.add(UserColumn::CreateTime.gte(utc_dt));
                }
                if let Some(ref end) = req.end_time
                    && let Some(utc_dt) = parse_beijing_datetime(end, true)
                {
                    cond = cond.add(UserColumn::CreateTime.lte(utc_dt));
                }

                let total = base_query
                    .clone()
                    .filter(cond.clone())
                    .count(&*conn)
                    .await?;

                let offset = (req.page() - 1) * req.size();
                let records = base_query
                    .filter(cond)
                    .order_by(UserColumn::CreateTime, order_desc())
                    .offset(Some(offset as u64))
                    .limit(req.size() as u64)
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .map(UserVO::from)
                    .collect();

                Ok((records, total as i64))
            })
        })
    }

    fn update(&self, id: &i64, req: &UpdateUserRequest) -> DynFuture<SeaOrmOptResult<User>> {
        let req = req.clone();
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let exists = UserEntity::find()
                    .filter(UserColumn::Id.eq(id))
                    .filter(UserColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if exists.is_none() {
                    return Ok(None);
                }

                let active_model = req.to_active_model(id, chrono::Utc::now());
                UserEntity::update(active_model)
                    .filter(UserColumn::Id.eq(id))
                    .filter(UserColumn::IsDeleted.eq(0))
                    .exec(&*conn)
                    .await?;

                let user = UserEntity::find()
                    .filter(UserColumn::Id.eq(id))
                    .filter(UserColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(user)
            })
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let user = UserEntity::find()
                    .filter(UserColumn::Id.eq(id))
                    .filter(UserColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if let Some(mut user) = user {
                    user.is_deleted = 1;
                    user.update_time = chrono::Utc::now();
                    let mut active_model: ActiveModel = user.into();
                    active_model.is_deleted = ActiveValue::Set(1);
                    UserEntity::update(active_model).exec(&*conn).await?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        })
    }
}

fn parse_id_list(s: &str) -> Vec<i64> {
    s.split(',').filter_map(|p| p.trim().parse().ok()).collect()
}

fn parse_beijing_datetime(s: &str, is_end: bool) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let naive = if s.len() > 10 {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").ok()?
    } else if is_end {
        chrono::NaiveDateTime::parse_from_str(&format!("{} 23:59:59", s), "%Y-%m-%d %H:%M:%S")
            .ok()?
    } else {
        chrono::NaiveDateTime::parse_from_str(&format!("{} 00:00:00", s), "%Y-%m-%d %H:%M:%S")
            .ok()?
    };
    let tz = chrono::FixedOffset::east_opt(8 * 3600)?;
    tz.from_local_datetime(&naive)
        .single()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}
