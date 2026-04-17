pub mod domain;
pub mod entity;
pub mod handlers;
pub mod repository;
pub mod service;
pub mod sys_role_menu;
pub mod user_role;

pub use entity::Entity as RoleEntity;
pub use sys_role_menu::Entity as SysRoleMenuEntity;
pub use user_role::Entity as UserRoleEntity;
