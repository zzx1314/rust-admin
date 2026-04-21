use crate::common::error::AppError;
use crate::common::traits::{MenuRepository, RoleRepository};
use crate::menu::domain::Menu;
use crate::sys_auth::domain::{SetMenuAuthRequest, SysAuthMenuVo, SysAuthTitleVo};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub struct SysAuthService {
    menu_repo: Arc<dyn MenuRepository>,
    role_repo: Arc<dyn RoleRepository>,
}

impl SysAuthService {
    pub fn new(menu_repo: Arc<dyn MenuRepository>, role_repo: Arc<dyn RoleRepository>) -> Self {
        Self {
            menu_repo,
            role_repo,
        }
    }

    pub async fn get_role_auth(&self, role_code: &str) -> Result<Vec<SysAuthMenuVo>, AppError> {
        let all_menus = self
            .menu_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let all_menu_map: HashMap<i64, Menu> = all_menus.into_iter().map(|m| (m.id, m)).collect();

        let role_menus = self
            .menu_repo
            .find_menus_by_role_id(&role_code.parse().unwrap_or(0))
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if role_menus.is_empty() {
            return Ok(Vec::new());
        }

        let role_menu_ids: HashSet<i64> = role_menus.iter().map(|m| m.id).collect();

        let buttons: Vec<&Menu> = role_menus.iter().filter(|m| m.r#type == Some(2)).collect();

        let parent_ids: HashSet<i64> = buttons.iter().filter_map(|b| b.parent_id).collect();

        let mut result: Vec<SysAuthMenuVo> = Vec::new();

        for parent_id in parent_ids {
            let mut menu_path_list: Vec<String> = Vec::new();
            self.build_menu_path(&parent_id, &all_menu_map, &mut menu_path_list);
            menu_path_list.reverse();
            let title = menu_path_list.join("/");

            let parent_buttons: Vec<&Menu> = buttons
                .iter()
                .filter(|b| b.parent_id == Some(parent_id))
                .map(|b| *b)
                .collect();

            let auth_list: Vec<SysAuthTitleVo> = parent_buttons
                .iter()
                .map(|b| SysAuthTitleVo {
                    id: b.id,
                    name: b.name.clone(),
                    permission: b.permission.clone(),
                })
                .collect();

            let use_auth_list: HashSet<i64> = parent_buttons
                .iter()
                .filter(|b| role_menu_ids.contains(&b.id))
                .map(|b| b.id)
                .collect();

            let is_check_all = auth_list.len() == use_auth_list.len();

            result.push(SysAuthMenuVo {
                id: 0,
                title,
                auth_list,
                use_auth_list,
                is_check_all,
            });
        }

        result.sort_by(|a, b| a.title.cmp(&b.title));

        for (i, item) in result.iter_mut().enumerate() {
            item.id = (i + 1) as i64;
        }

        Ok(result)
    }

    fn build_menu_path(
        &self,
        menu_id: &i64,
        all_menu_map: &HashMap<i64, Menu>,
        path: &mut Vec<String>,
    ) {
        if let Some(menu) = all_menu_map.get(menu_id) {
            path.push(menu.name.clone());
            if let Some(parent_id) = menu.parent_id {
                if parent_id != 0 {
                    self.build_menu_path(&parent_id, all_menu_map, path);
                }
            }
        }
    }

    pub async fn set_menu_auth(&self, req: SetMenuAuthRequest) -> Result<(), AppError> {
        let role = self
            .role_repo
            .find_by_code(&req.role_code)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| {
                AppError::NotFound(format!("Role with code {} not found", req.role_code))
            })?;

        if req.auth_list.is_empty() {
            self.role_repo
                .set_menus(&role.id, &[])
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;
            return Ok(());
        }

        let all_menus = self
            .menu_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let menu_map: HashMap<i64, Menu> = all_menus.into_iter().map(|m| (m.id, m)).collect();

        let mut menu_id_set: HashSet<i64> = HashSet::new();

        for auth_id in &req.auth_list {
            if let Some(menu) = menu_map.get(auth_id) {
                menu_id_set.insert(menu.id);
                self.collect_parent_ids(menu, &menu_map, &mut menu_id_set);
            }
        }

        let menu_ids: Vec<i64> = menu_id_set.into_iter().collect();

        self.role_repo
            .set_menus(&role.id, &menu_ids)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(())
    }

    fn collect_parent_ids(
        &self,
        menu: &Menu,
        menu_map: &HashMap<i64, Menu>,
        set: &mut HashSet<i64>,
    ) {
        if let Some(parent_id) = menu.parent_id {
            if parent_id != 0 {
                set.insert(parent_id);
                if let Some(parent) = menu_map.get(&parent_id) {
                    self.collect_parent_ids(parent, menu_map, set);
                }
            }
        }
    }
}
