use crate::common::base::{RepoExt, make_condition, order_desc};
use crate::common::traits::{DynFuture, RoleRepository, SeaOrmOptResult, SeaOrmResult};
use crate::impl_repo_conn;
use crate::role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use crate::role::entity::ActiveModel as RoleActiveModel;
use crate::role::entity::Column as RoleColumn;
use crate::role::entity::Entity as RoleEntity;
use crate::role::sys_role_menu::ActiveModel as SysRoleMenuActiveModel;
use crate::role::sys_role_menu::Entity as SysRoleMenuEntity;
use crate::role::user_role::ActiveModel as UserRoleActiveModel;
use crate::role::user_role::Column as UserRoleColumn;
use crate::role::user_role::Entity as UserRoleEntity;
use crate::user::domain::User;
use crate::user::entity::Column as UserColumn;
use crate::user::entity::Entity as UserEntity;
use async_trait::async_trait;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect,
};
use std::sync::Arc;

pub struct SeaOrmRoleRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmRoleRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl_repo_conn!(SeaOrmRoleRepository);

#[async_trait]
impl RoleRepository for SeaOrmRoleRepository {
    fn create(&self, req: &CreateRoleRequest, id: &i64) -> DynFuture<SeaOrmResult<Role>> {
        let req = req.clone();
        let id = *id;

        self.with_conn(move |conn| {
            Box::pin(async move {
                let active_model = req.to_active_model(id);

                RoleEntity::insert(active_model).exec(&*conn).await?;

                let role = RoleEntity::find_by_id(id).one(&*conn).await?;

                Ok(role.unwrap())
            })
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Role>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let role = RoleEntity::find()
                    .filter(RoleColumn::Id.eq(id))
                    .filter(RoleColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(role)
            })
        })
    }

    fn find_by_code(&self, code: &str) -> DynFuture<SeaOrmOptResult<Role>> {
        let code = code.to_string();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let role = RoleEntity::find()
                    .filter(RoleColumn::Code.eq(&code))
                    .filter(RoleColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(role)
            })
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Role>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let roles = RoleEntity::find()
                    .filter(RoleColumn::IsDeleted.eq(0))
                    .order_by(RoleColumn::CreateTime, order_desc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(roles)
            })
        })
    }

    fn find_all_with_page(&self, req: &RolePageQuery) -> DynFuture<SeaOrmResult<(Vec<Role>, i64)>> {
        let req = req.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let base_query = RoleEntity::find().filter(RoleColumn::IsDeleted.eq(0));

                let mut cond = make_condition();
                let conditions: Vec<_> = [
                    req.name.as_ref().map(|v| RoleColumn::Name.contains(v)),
                    req.code.as_ref().map(|v| RoleColumn::Code.contains(v)),
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
                    .order_by(RoleColumn::CreateTime, order_desc())
                    .offset(Some(offset as u64))
                    .limit(req.size() as u64)
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();

                Ok((records, total as i64))
            })
        })
    }

    fn update(&self, id: &i64, req: &UpdateRoleRequest) -> DynFuture<SeaOrmOptResult<Role>> {
        let id = *id;
        let req = req.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let exists = RoleEntity::find()
                    .filter(RoleColumn::Id.eq(id))
                    .filter(RoleColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if exists.is_none() {
                    return Ok(None);
                }

                let active_model = req.to_active_model(id);
                RoleEntity::update(active_model)
                    .filter(RoleColumn::Id.eq(id))
                    .filter(RoleColumn::IsDeleted.eq(0))
                    .exec(&*conn)
                    .await?;

                let role = RoleEntity::find()
                    .filter(RoleColumn::Id.eq(id))
                    .filter(RoleColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(role)
            })
        })
    }
    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let id = *id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let role = RoleEntity::find()
                    .filter(RoleColumn::Id.eq(id))
                    .filter(RoleColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if let Some(mut role) = role {
                    role.is_deleted = 1;
                    role.update_time = chrono::Utc::now();
                    let mut active_model: RoleActiveModel = role.into();
                    active_model.is_deleted = ActiveValue::Set(1);
                    RoleEntity::update(active_model).exec(&*conn).await?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        })
    }

    fn assign_role_to_user(&self, user_id: &i64, role_id: &i64) -> DynFuture<SeaOrmResult<()>> {
        let user_id = *user_id;
        let role_id = *role_id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let active_model = UserRoleActiveModel {
                    user_id: ActiveValue::set(user_id),
                    role_id: ActiveValue::set(role_id),
                };
                UserRoleEntity::insert(active_model).exec(&*conn).await?;
                Ok(())
            })
        })
    }

    fn remove_role_from_user(&self, user_id: &i64, role_id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let user_id = *user_id;
        let role_id = *role_id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let result = UserRoleEntity::delete_many()
                    .filter(UserRoleColumn::UserId.eq(user_id))
                    .filter(UserRoleColumn::RoleId.eq(role_id))
                    .exec(&*conn)
                    .await?;
                Ok(result.rows_affected > 0)
            })
        })
    }

    fn find_roles_by_user_id(&self, user_id: &i64) -> DynFuture<SeaOrmResult<Vec<Role>>> {
        let user_id = *user_id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let user_roles = UserRoleEntity::find()
                    .filter(UserRoleColumn::UserId.eq(user_id))
                    .all(&*conn)
                    .await?;

                let mut roles = Vec::new();
                for ur in user_roles {
                    if let Some(role) = RoleEntity::find_by_id(ur.role_id)
                        .filter(RoleColumn::IsDeleted.eq(0))
                        .one(&*conn)
                        .await?
                    {
                        roles.push(Role::from(role));
                    }
                }
                Ok(roles)
            })
        })
    }

    fn find_users_by_role_id(&self, role_id: &i64) -> DynFuture<SeaOrmResult<Vec<User>>> {
        let role_id = *role_id;
        self.with_conn(move |conn| {
            Box::pin(async move {
                let user_roles = UserRoleEntity::find()
                    .filter(UserRoleColumn::RoleId.eq(role_id))
                    .all(&*conn)
                    .await?;

                let mut users = Vec::new();
                for ur in user_roles {
                    if let Some(user) = UserEntity::find()
                        .filter(UserColumn::Id.eq(ur.user_id))
                        .filter(UserColumn::IsDeleted.eq(0))
                        .one(&*conn)
                        .await?
                    {
                        users.push(User::from(user));
                    }
                }
                Ok(users)
            })
        })
    }

    fn set_menus(&self, role_id: &i64, menu_ids: &[i64]) -> DynFuture<SeaOrmResult<()>> {
        let role_id = *role_id;
        let menu_ids = menu_ids.to_vec();
        self.with_conn(move |conn| {
            Box::pin(async move {
                SysRoleMenuEntity::delete_many()
                    .filter(crate::role::sys_role_menu::Column::RoleId.eq(role_id))
                    .exec(&*conn)
                    .await?;

                for menu_id in menu_ids {
                    let active_model = SysRoleMenuActiveModel {
                        role_id: ActiveValue::set(role_id),
                        menu_id: ActiveValue::set(menu_id),
                    };
                    SysRoleMenuEntity::insert(active_model).exec(&*conn).await?;
                }
                Ok(())
            })
        })
    }
}
