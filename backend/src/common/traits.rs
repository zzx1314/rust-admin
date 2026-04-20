use crate::common::error::AppError;
use crate::menu::domain::{CreateMenuRequest, Menu, UpdateMenuRequest};
use crate::org::domain::{CreateOrgRequest, Org, OrgTreeQuery, UpdateOrgRequest};
use crate::role::domain::{CreateRoleRequest, Role, RolePageQuery, UpdateRoleRequest};
use crate::user::domain::{CreateUserRequest, UpdateUserRequest, User, UserPageQuery, UserVO};
use sea_orm::DbErr;
use std::future::Future;
use std::pin::Pin;

pub type SeaOrmResult<T> = Result<T, DbErr>;
pub type SeaOrmOptResult<T> = Result<Option<T>, DbErr>;
pub type DynFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

pub trait UserRepository: Send + Sync {
    fn create(&self, req: &CreateUserRequest, id: &i64) -> DynFuture<SeaOrmResult<User>>;
    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<User>>;
    fn find_by_email(&self, email: &str) -> DynFuture<SeaOrmOptResult<User>>;
    fn find_by_username(&self, username: &str) -> DynFuture<SeaOrmOptResult<User>>;
    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<User>>>;
    fn find_all_with_page(
        &self,
        query: &UserPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<UserVO>, i64)>>;
    fn update(&self, id: &i64, req: &UpdateUserRequest) -> DynFuture<SeaOrmOptResult<User>>;
    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>>;
}

pub trait RoleRepository: Send + Sync {
    fn create(&self, role: &CreateRoleRequest, id: &i64) -> DynFuture<SeaOrmResult<Role>>;
    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Role>>;
    fn find_by_code(&self, code: &str) -> DynFuture<SeaOrmOptResult<Role>>;
    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Role>>>;
    fn find_all_with_page(
        &self,
        query: &RolePageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<Role>, i64)>>;
    fn update(&self, id: &i64, req: &UpdateRoleRequest) -> DynFuture<SeaOrmOptResult<Role>>;
    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>>;

    fn assign_role_to_user(&self, user_id: &i64, role_id: &i64) -> DynFuture<SeaOrmResult<()>>;
    fn remove_role_from_user(&self, user_id: &i64, role_id: &i64) -> DynFuture<SeaOrmResult<bool>>;
    fn find_roles_by_user_id(&self, user_id: &i64) -> DynFuture<SeaOrmResult<Vec<Role>>>;
    fn find_users_by_role_id(&self, role_id: &i64) -> DynFuture<SeaOrmResult<Vec<User>>>;
    fn set_menus(&self, role_id: &i64, menu_ids: &[i64]) -> DynFuture<SeaOrmResult<()>>;
}

pub trait TokenStore: Send + Sync {
    fn set_token(
        &self,
        user_id: &str,
        token: &str,
        ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>>;
    fn get_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>>;
    fn delete_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>>;
}

pub trait MenuRepository: Send + Sync {
    fn create(&self, menu: &CreateMenuRequest, id: &i64) -> DynFuture<SeaOrmResult<Menu>>;
    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Menu>>;
    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Menu>>>;
    fn find_by_parent_id(&self, parent_id: Option<i64>) -> DynFuture<SeaOrmResult<Vec<Menu>>>;
    fn find_tree(&self) -> DynFuture<SeaOrmResult<Vec<Menu>>>;
    fn find_menus_by_role_id(&self, role_id: &i64) -> DynFuture<SeaOrmResult<Vec<Menu>>>;
    fn update(&self, id: &i64, req: &UpdateMenuRequest) -> DynFuture<SeaOrmOptResult<Menu>>;
    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>>;
}

pub trait OrgRepository: Send + Sync {
    fn create(&self, org: &CreateOrgRequest, id: &i64) -> DynFuture<SeaOrmResult<Org>>;
    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Org>>;
    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Org>>>;
    fn find_by_parent_id(&self, parent_id: Option<i64>) -> DynFuture<SeaOrmResult<Vec<Org>>>;
    fn find_tree(&self) -> DynFuture<SeaOrmResult<Vec<Org>>>;
    fn find_tree_with_filter(&self, query: &OrgTreeQuery) -> DynFuture<SeaOrmResult<Vec<Org>>>;
    fn update(&self, id: &i64, req: &UpdateOrgRequest) -> DynFuture<SeaOrmOptResult<Org>>;
    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>>;
}
