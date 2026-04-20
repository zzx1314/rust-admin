use crate::menu::entity::ActiveModel as MenuActiveModel;
use crate::menu::entity::Model as MenuModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type Menu = MenuModel;

#[derive(Debug, Deserialize, Clone)]
pub struct CreateMenuRequest {
    pub name: String,
    pub code: Option<String>,
    pub permission: Option<String>,
    pub path_url: Option<String>,
    pub icon: Option<String>,
    pub parent_id: Option<String>,
    pub component: Option<String>,
    pub sort: Option<i32>,
    pub keep_alive: Option<i32>,
    pub r#type: Option<i32>,
    pub remarks: Option<String>,
    pub leaf: Option<bool>,
    pub role_code: Option<String>,
    pub disabled: Option<bool>,
    pub find_auth_id: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpdateMenuRequest {
    pub name: Option<String>,
    pub code: Option<String>,
    pub permission: Option<String>,
    pub path_url: Option<String>,
    pub icon: Option<String>,
    pub parent_id: Option<String>,
    pub component: Option<String>,
    pub sort: Option<i32>,
    pub keep_alive: Option<i32>,
    pub r#type: Option<i32>,
    pub remarks: Option<String>,
    pub leaf: Option<bool>,
    pub role_code: Option<String>,
    pub disabled: Option<bool>,
    pub find_auth_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MenuMeta {
    pub icon: Option<String>,
    pub rank: Option<i32>,
    #[serde(rename = "showParent")]
    pub show_parent: Option<bool>,
    pub title: Option<String>,
    pub auths: Option<Vec<String>>,
    pub roles: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MenuVo {
    pub id: String,
    pub name: String,
    pub code: Option<String>,
    pub permission: Option<String>,
    pub path_url: Option<String>,
    pub icon: Option<String>,
    pub parent_id: Option<String>,
    pub component: Option<String>,
    pub sort: Option<i32>,
    pub keep_alive: Option<i32>,
    pub r#type: Option<i32>,
    pub remarks: Option<String>,
    pub leaf: Option<bool>,
    pub disabled: Option<bool>,
    pub role_code: Option<String>,
    pub meta: Option<MenuMeta>,
}

impl From<Menu> for MenuVo {
    fn from(menu: Menu) -> Self {
        Self {
            id: menu.id,
            name: menu.name,
            code: menu.code,
            permission: menu.permission,
            path_url: menu.path_url,
            icon: menu.icon,
            parent_id: menu.parent_id,
            component: menu.component,
            sort: menu.sort,
            keep_alive: menu.keep_alive,
            r#type: menu.r#type,
            remarks: menu.remarks,
            leaf: menu.leaf,
            disabled: menu.disabled,
            role_code: menu.role_code,
            meta: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MenuTree {
    pub id: String,
    pub name: String,
    pub code: Option<String>,
    pub permission: Option<String>,
    #[serde(rename = "path")]
    pub path_url: Option<String>,
    pub icon: Option<String>,
    #[serde(rename = "parentId")]
    pub parent_id: Option<String>,
    pub component: Option<String>,
    pub sort: Option<i32>,
    pub keep_alive: Option<i32>,
    pub r#type: Option<i32>,
    pub remarks: Option<String>,
    pub leaf: Option<bool>,
    pub disabled: Option<bool>,
    pub meta: Option<MenuMeta>,
    pub children: Option<Vec<MenuTree>>,
}

impl From<MenuVo> for MenuTree {
    fn from(vo: MenuVo) -> Self {
        Self {
            id: vo.id,
            name: vo.name,
            code: vo.code,
            permission: vo.permission,
            path_url: vo.path_url,
            icon: vo.icon,
            parent_id: vo.parent_id,
            component: vo.component,
            sort: vo.sort,
            keep_alive: vo.keep_alive,
            r#type: vo.r#type,
            remarks: vo.remarks,
            leaf: vo.leaf,
            disabled: vo.disabled,
            meta: vo.meta,
            children: None,
        }
    }
}

pub fn build_menu_tree(menu_trees: Vec<MenuTree>) -> Vec<MenuTree> {
    let mut id_map: std::collections::HashMap<String, MenuTree> = std::collections::HashMap::new();
    let mut roots: Vec<MenuTree> = Vec::new();

    for menu in &menu_trees {
        id_map.insert(menu.id.clone(), menu.clone());
    }

    for menu in menu_trees {
        let pid = menu.parent_id.clone();
        match &pid {
            Some(p) if !p.is_empty() && p != "-1" => {
                if id_map.contains_key(p) {
                } else {
                    roots.push(menu);
                }
            }
            _ => {
                roots.push(menu);
            }
        }
    }

    fn attach_children(menu: &mut MenuTree, id_map: &std::collections::HashMap<String, MenuTree>) {
        let children_ids: Vec<String> = id_map
            .iter()
            .filter(|(_, m)| m.parent_id.as_ref() == Some(&menu.id))
            .map(|(k, _)| k.clone())
            .collect();

        if !children_ids.is_empty() {
            let mut children = Vec::new();
            for cid in children_ids {
                if let Some(mut child) = id_map.get(&cid).cloned() {
                    attach_children(&mut child, id_map);
                    children.push(child);
                }
            }
            children.sort_by(|a, b| {
                a.meta.as_ref().and_then(|m| m.rank).unwrap_or(0)
                    .cmp(&b.meta.as_ref().and_then(|m| m.rank).unwrap_or(0))
            });
            menu.children = Some(children);
        }
    }

    for root in &mut roots {
        attach_children(root, &id_map);
    }

    roots.sort_by(|a, b| {
        a.meta.as_ref().and_then(|m| m.rank).unwrap_or(0)
            .cmp(&b.meta.as_ref().and_then(|m| m.rank).unwrap_or(0))
    });

    roots
}


impl CreateMenuRequest {
    pub fn to_active_model(&self, id: &str, now: DateTime<Utc>) -> MenuActiveModel {
        MenuActiveModel {
            id: ActiveValue::set(id.to_string()),
            name: ActiveValue::set(self.name.clone()),
            code: ActiveValue::set(self.code.clone()),
            permission: ActiveValue::set(self.permission.clone()),
            path_url: ActiveValue::set(self.path_url.clone()),
            icon: ActiveValue::set(self.icon.clone()),
            parent_id: ActiveValue::set(self.parent_id.clone()),
            component: ActiveValue::set(self.component.clone()),
            sort: ActiveValue::set(self.sort),
            keep_alive: ActiveValue::set(self.keep_alive),
            r#type: ActiveValue::set(self.r#type),
            is_deleted: ActiveValue::set(0),
            remarks: ActiveValue::set(self.remarks.clone()),
            leaf: ActiveValue::set(self.leaf),
            role_code: ActiveValue::set(self.role_code.clone()),
            disabled: ActiveValue::set(self.disabled),
            find_auth_id: set_opt_string(self.find_auth_id.clone()),
            create_time: ActiveValue::set(now),
            update_time: ActiveValue::set(now),
        }
    }
}

impl UpdateMenuRequest {
    pub fn to_active_model(&self, id: &str) -> MenuActiveModel {
        MenuActiveModel {
            id: ActiveValue::unchanged(id.to_string()),
            name: set_string(self.name.clone()),
            code: set_opt_string(self.code.clone()),
            permission: set_opt_string(self.permission.clone()),
            path_url: set_opt_string(self.path_url.clone()),
            icon: set_opt_string(self.icon.clone()),
            parent_id: set_opt_string(self.parent_id.clone()),
            component: set_opt_string(self.component.clone()),
            sort: set_opt_i32(self.sort),
            keep_alive: set_opt_i32(self.keep_alive),
            r#type: set_opt_i32(self.r#type),
            is_deleted: ActiveValue::unchanged(0),
            remarks: set_opt_string(self.remarks.clone()),
            leaf: set_opt_bool(self.leaf),
            role_code: set_opt_string(self.role_code.clone()),
            disabled: set_opt_bool(self.disabled),
            find_auth_id: set_opt_string(self.find_auth_id.clone()),
            update_time: ActiveValue::set(Utc::now()),
            ..Default::default()
        }
    }
}

fn set_string(opt: Option<String>) -> ActiveValue<String> {
    match opt {
        Some(v) => ActiveValue::set(v),
        None => ActiveValue::not_set(),
    }
}

fn set_opt_string(opt: Option<String>) -> ActiveValue<Option<String>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}

fn set_opt_i32(opt: Option<i32>) -> ActiveValue<Option<i32>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}

fn set_opt_bool(opt: Option<bool>) -> ActiveValue<Option<bool>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}
