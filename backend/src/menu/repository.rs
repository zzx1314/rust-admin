use crate::common::base::{RepoExt, make_condition, order_asc};
use crate::common::traits::{DynFuture, MenuRepository, SeaOrmOptResult, SeaOrmResult};
use crate::impl_repo_conn;
use crate::menu::domain::{CreateMenuRequest, Menu, UpdateMenuRequest};
use crate::menu::entity::ActiveModel as MenuActiveModel;
use crate::menu::entity::Column as MenuColumn;
use crate::menu::entity::Entity as MenuEntity;
use crate::role::sys_role_menu::Column as SysRoleMenuColumn;
use crate::role::sys_role_menu::Entity as SysRoleMenuEntity;
use async_trait::async_trait;
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use std::sync::Arc;

pub struct SeaOrmMenuRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmMenuRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl_repo_conn!(SeaOrmMenuRepository);

#[async_trait]
impl MenuRepository for SeaOrmMenuRepository {
    fn create(&self, menu: &CreateMenuRequest, id: &str) -> DynFuture<SeaOrmResult<Menu>> {
        let id_str = id.to_string();
        let menu = menu.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let now = chrono::Utc::now();
                let active_model = menu.to_active_model(&id_str, now);

                MenuEntity::insert(active_model).exec(&*conn).await?;

                let created = MenuEntity::find_by_id(&id_str).one(&*conn).await?;
                Ok(created.unwrap())
            })
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let menus = MenuEntity::find()
                    .filter(MenuColumn::IsDeleted.eq(0))
                    .order_by(MenuColumn::Sort, order_asc())
                    .order_by(MenuColumn::CreateTime, order_asc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(menus)
            })
        })
    }

    fn find_by_id(&self, id: &str) -> DynFuture<SeaOrmOptResult<Menu>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let menu = MenuEntity::find()
                    .filter(MenuColumn::Id.eq(&id))
                    .filter(MenuColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(menu)
            })
        })
    }

    fn find_by_parent_id(&self, parent_id: Option<&str>) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        let parent_id = parent_id.map(String::from);
        self.with_conn(move |conn| {
            Box::pin(async move {
                let mut cond = make_condition().add(MenuColumn::IsDeleted.eq(0));

                if let Some(ref pid) = parent_id {
                    cond = cond.add(MenuColumn::ParentId.eq(pid));
                } else {
                    cond = cond.add(MenuColumn::ParentId.is_null());
                }

                let menus = MenuEntity::find()
                    .filter(cond)
                    .order_by(MenuColumn::Sort, order_asc())
                    .order_by(MenuColumn::CreateTime, order_asc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(menus)
            })
        })
    }

    fn find_tree(&self) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        self.with_conn(|conn| {
            Box::pin(async move {
                let menus = MenuEntity::find()
                    .filter(MenuColumn::IsDeleted.eq(0))
                    .order_by(MenuColumn::Sort, order_asc())
                    .order_by(MenuColumn::CreateTime, order_asc())
                    .all(&*conn)
                    .await?
                    .into_iter()
                    .collect();
                Ok(menus)
            })
        })
    }

    fn find_menus_by_role_id(&self, role_id: &str) -> DynFuture<SeaOrmResult<Vec<Menu>>> {
        let role_id = role_id.to_string();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let sys_role_menus = SysRoleMenuEntity::find()
                    .filter(SysRoleMenuColumn::RoleId.eq(&role_id))
                    .all(&*conn)
                    .await?;

                let mut menus = Vec::new();
                for srm in sys_role_menus {
                    if let Some(menu) = MenuEntity::find_by_id(&srm.menu_id)
                        .filter(MenuColumn::IsDeleted.eq(0))
                        .one(&*conn)
                        .await?
                    {
                        menus.push(Menu::from(menu));
                    }
                }
                Ok(menus)
            })
        })
    }

    fn update(&self, id: &str, req: &UpdateMenuRequest) -> DynFuture<SeaOrmOptResult<Menu>> {
        let id_str = id.to_string();
        let req = req.clone();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let exists = MenuEntity::find()
                    .filter(MenuColumn::Id.eq(&id_str))
                    .filter(MenuColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if exists.is_none() {
                    return Ok(None);
                }

                let active_model = req.to_active_model(&id_str);
                MenuEntity::update(active_model)
                    .filter(MenuColumn::Id.eq(&id_str))
                    .filter(MenuColumn::IsDeleted.eq(0))
                    .exec(&*conn)
                    .await?;

                let menu = MenuEntity::find()
                    .filter(MenuColumn::Id.eq(&id_str))
                    .filter(MenuColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;
                Ok(menu)
            })
        })
    }

    fn delete(&self, id: &str) -> DynFuture<SeaOrmResult<bool>> {
        let id = id.to_string();
        self.with_conn(move |conn| {
            Box::pin(async move {
                let menu = MenuEntity::find()
                    .filter(MenuColumn::Id.eq(&id))
                    .filter(MenuColumn::IsDeleted.eq(0))
                    .one(&*conn)
                    .await?;

                if let Some(mut menu) = menu {
                    menu.is_deleted = 1;
                    menu.update_time = chrono::Utc::now();
                    let mut active_model: MenuActiveModel = menu.into();
                    active_model.is_deleted = ActiveValue::Set(1);
                    MenuEntity::update(active_model).exec(&*conn).await?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        })
    }
}
