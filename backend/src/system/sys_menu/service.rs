use crate::common::error::AppError;
use crate::common::traits::{MenuRepository, RoleRepository};
use crate::system::sys_menu::domain::{
    CreateMenuRequest, Menu, MenuMeta, MenuTree, MenuVo, UpdateMenuRequest, build_menu_tree,
};
use std::collections::HashMap;
use std::sync::Arc;

pub struct MenuService {
    menu_repo: Arc<dyn MenuRepository>,
    role_repo: Arc<dyn RoleRepository>,
}

impl MenuService {
    pub fn new(menu_repo: Arc<dyn MenuRepository>, role_repo: Arc<dyn RoleRepository>) -> Self {
        Self {
            menu_repo,
            role_repo,
        }
    }

    pub async fn create_menu(&self, req: CreateMenuRequest) -> Result<Menu, AppError> {
        let id = self.generate_id().await;
        self.menu_repo
            .create(&req, &id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_menu(&self, id: &i64) -> Result<Menu, AppError> {
        self.menu_repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Menu with id {} not found", id)))
    }

    pub async fn get_all_menus(&self) -> Result<Vec<Menu>, AppError> {
        self.menu_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_menu_tree(&self) -> Result<Vec<Menu>, AppError> {
        self.menu_repo
            .find_tree()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_menus_by_parent(&self, parent_id: Option<i64>) -> Result<Vec<Menu>, AppError> {
        self.menu_repo
            .find_by_parent_id(parent_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update_menu(&self, id: &i64, req: UpdateMenuRequest) -> Result<Menu, AppError> {
        self.menu_repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Menu with id {} not found", id)))
    }

    pub async fn delete_menu(&self, id: &i64) -> Result<(), AppError> {
        let deleted = self
            .menu_repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if !deleted {
            return Err(AppError::NotFound(format!("Menu with id {} not found", id)));
        }
        Ok(())
    }

    pub async fn get_user_menu(&self, user_id: &str) -> Result<Vec<MenuTree>, AppError> {
        let user_id: i64 = user_id
            .parse()
            .map_err(|_| AppError::BadRequest("Invalid user id".to_string()))?;
        let roles = self
            .role_repo
            .find_roles_by_user_id(&user_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let role_id = roles
            .first()
            .ok_or_else(|| AppError::NotFound("当前用户没有分配角色".to_string()))?;

        let menus = self
            .menu_repo
            .find_menus_by_role_id(&role_id.id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let buttons_by_parent: HashMap<i64, Vec<String>> = menus
            .iter()
            .filter(|m| m.r#type == Some(2))
            .filter_map(|m| {
                m.parent_id
                    .and_then(|pid| m.permission.as_ref().map(|p| (pid, p.clone())))
            })
            .fold(HashMap::new(), |mut acc, (pid, perm)| {
                acc.entry(pid).or_default().push(perm);
                acc
            });

        let mut vos: Vec<MenuVo> = menus
            .into_iter()
            .map(|m| {
                let mut vo = MenuVo::from(m);
                let roles = vo
                    .role_code
                    .as_ref()
                    .map(|rc| rc.split(',').map(|s| s.trim().to_string()).collect());
                if let Some(auths) = buttons_by_parent.get(&vo.id) {
                    vo.meta = Some(MenuMeta {
                        icon: vo.icon.clone(),
                        rank: vo.sort,
                        show_parent: vo.leaf,
                        title: Some(vo.name.clone()),
                        auths: Some(auths.clone()),
                        roles,
                    });
                } else {
                    vo.meta = Some(MenuMeta {
                        icon: vo.icon.clone(),
                        rank: vo.sort,
                        show_parent: vo.leaf,
                        title: Some(vo.name.clone()),
                        auths: None,
                        roles,
                    });
                }
                vo
            })
            .filter(|vo| vo.r#type != Some(2))
            .collect();

        vos.sort_by_key(|vo| vo.meta.as_ref().and_then(|m| m.rank).unwrap_or(0));

        let trees: Vec<MenuTree> = vos.into_iter().map(MenuTree::from).collect();

        Ok(build_menu_tree(trees))
    }

    async fn generate_id(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}
