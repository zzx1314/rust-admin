use crate::org::entity::ActiveModel as OrgActiveModel;
use crate::org::entity::Model as OrgModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type Org = OrgModel;

#[derive(Debug, Deserialize, Clone)]
pub struct CreateOrgRequest {
    pub name: String,
    pub sort: Option<i32>,
    pub parent_id: Option<i64>,
    pub parent_name: Option<String>,
    pub org_duty: Option<String>,
    pub desrc: Option<String>,
    pub r#type: Option<String>,
    pub is_out: Option<bool>,
    pub remarks: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpdateOrgRequest {
    pub name: Option<String>,
    pub sort: Option<i32>,
    pub parent_id: Option<i64>,
    pub parent_name: Option<String>,
    pub org_duty: Option<String>,
    pub desrc: Option<String>,
    pub r#type: Option<String>,
    pub is_out: Option<bool>,
    pub remarks: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrgTreeDto {
    pub id: i64,
    pub name: String,
    pub sort: Option<i32>,
    pub parent_id: Option<i64>,
    pub parent_name: Option<String>,
    pub org_duty: Option<String>,
    pub desrc: Option<String>,
    pub r#type: Option<String>,
    pub is_deleted: i32,
    pub remarks: Option<String>,
    pub create_time: DateTime<Utc>,
    pub children: Option<Vec<OrgTreeDto>>,
}

impl From<Org> for OrgTreeDto {
    fn from(org: Org) -> Self {
        Self {
            id: org.id,
            name: org.name,
            sort: org.sort,
            parent_id: org.parent_id,
            parent_name: org.parent_name,
            org_duty: org.org_duty,
            desrc: org.desrc,
            r#type: org.r#type,
            is_deleted: org.is_deleted,
            remarks: org.remarks,
            create_time: org.create_time,
            children: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct OrgTreeQuery {
    pub name: Option<String>,
    pub r#type: Option<String>,
}

pub fn build_org_tree(orgs: Vec<OrgTreeDto>) -> Vec<OrgTreeDto> {
    let mut id_map: std::collections::HashMap<i64, OrgTreeDto> =
        std::collections::HashMap::new();

    for org in &orgs {
        id_map.insert(org.id, org.clone());
    }

    let mut root_ids: Vec<i64> = Vec::new();

    for org in &orgs {
        match &org.parent_id {
            Some(pid) if *pid != 0 && id_map.contains_key(pid) => {}
            _ => {
                root_ids.push(org.id);
            }
        }
    }

    fn build_children(
        node: &mut OrgTreeDto,
        id_map: &std::collections::HashMap<i64, OrgTreeDto>,
        children_of: &std::collections::HashMap<i64, Vec<i64>>,
    ) {
        if let Some(child_ids) = children_of.get(&node.id) {
            let mut children: Vec<OrgTreeDto> = child_ids
                .iter()
                .filter_map(|cid| id_map.get(cid).cloned())
                .collect();
            children.sort_by(|a, b| a.sort.unwrap_or(0).cmp(&b.sort.unwrap_or(0)));
            for child in &mut children {
                build_children(child, id_map, children_of);
            }
            node.children = Some(children);
        }
    }

    let mut children_of: std::collections::HashMap<i64, Vec<i64>> =
        std::collections::HashMap::new();
    for org in &orgs {
        if let Some(ref pid) = org.parent_id
            && *pid != 0
            && id_map.contains_key(pid)
        {
            children_of
                .entry(*pid)
                .or_default()
                .push(org.id);
        }
    }

    let mut roots: Vec<OrgTreeDto> = root_ids
        .iter()
        .filter_map(|id| id_map.get(id).cloned())
        .collect();
    roots.sort_by(|a, b| a.sort.unwrap_or(0).cmp(&b.sort.unwrap_or(0)));

    for root in &mut roots {
        build_children(root, &id_map, &children_of);
    }

    roots
}

impl CreateOrgRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> OrgActiveModel {
        OrgActiveModel {
            id: ActiveValue::set(id),
            name: ActiveValue::set(self.name.clone()),
            sort: ActiveValue::set(self.sort),
            parent_id: ActiveValue::set(self.parent_id),
            parent_name: ActiveValue::set(self.parent_name.clone()),
            org_duty: ActiveValue::set(self.org_duty.clone()),
            desrc: ActiveValue::set(self.desrc.clone()),
            r#type: ActiveValue::set(self.r#type.clone()),
            is_deleted: ActiveValue::set(0),
            remarks: ActiveValue::set(self.remarks.clone()),
            create_time: ActiveValue::set(now),
            update_time: ActiveValue::set(now),
        }
    }
}

impl UpdateOrgRequest {
    pub fn to_active_model(&self, id: i64) -> OrgActiveModel {
        OrgActiveModel {
            id: ActiveValue::unchanged(id),
            name: set_string(self.name.clone()),
            sort: set_opt_i32(self.sort),
            parent_id: set_opt_i64(self.parent_id),
            parent_name: set_opt_string(self.parent_name.clone()),
            org_duty: set_opt_string(self.org_duty.clone()),
            desrc: set_opt_string(self.desrc.clone()),
            r#type: set_opt_string(self.r#type.clone()),
            is_deleted: ActiveValue::unchanged(0),
            remarks: set_opt_string(self.remarks.clone()),
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

fn set_opt_i64(opt: Option<i64>) -> ActiveValue<Option<i64>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}
