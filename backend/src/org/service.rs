use crate::common::error::AppError;
use crate::common::traits::OrgRepository;
use crate::org::domain::{
    CreateOrgRequest, Org, OrgTreeDto, OrgTreeQuery, UpdateOrgRequest, build_org_tree,
};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

pub struct OrgService {
    org_repo: Arc<dyn OrgRepository>,
}

impl OrgService {
    pub fn new(org_repo: Arc<dyn OrgRepository>) -> Self {
        Self { org_repo }
    }

    pub async fn create_org(&self, req: CreateOrgRequest) -> Result<Org, AppError> {
        let id = Uuid::new_v4().to_string();
        self.org_repo
            .create(&req, &id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_org(&self, id: &str) -> Result<Org, AppError> {
        self.org_repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Org with id {} not found", id)))
    }

    pub async fn get_all_orgs(&self) -> Result<Vec<Org>, AppError> {
        self.org_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_org_tree(&self, query: OrgTreeQuery) -> Result<Vec<OrgTreeDto>, AppError> {
        if query.name.is_none() && query.r#type.is_none() {
            let orgs = self
                .org_repo
                .find_tree()
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;
            let dtos: Vec<OrgTreeDto> = orgs.into_iter().map(OrgTreeDto::from).collect();
            return Ok(build_org_tree(dtos));
        }

        let all_orgs = self
            .org_repo
            .find_tree()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let matched = self
            .org_repo
            .find_tree_with_filter(&query)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        let all_id_map: HashMap<String, &Org> =
            all_orgs.iter().map(|o| (o.id.clone(), o)).collect();
        let mut collected: HashMap<String, Org> = HashMap::new();

        for org in &matched {
            let mut current = Some(org);
            while let Some(o) = current {
                if collected.contains_key(&o.id) {
                    break;
                }
                collected.insert(o.id.clone(), o.clone());
                current = o.parent_id.as_ref().and_then(|pid| {
                    if pid.is_empty() || pid == &o.id {
                        None
                    } else {
                        all_id_map.get(pid).copied()
                    }
                });
            }

            let mut queue: Vec<String> = vec![org.id.clone()];
            let mut idx = 0;
            while idx < queue.len() {
                let parent_id = queue[idx].clone();
                idx += 1;
                for o in &all_orgs {
                    if let Some(ref pid) = o.parent_id
                        && pid == &parent_id
                        && !collected.contains_key(&o.id)
                    {
                        collected.insert(o.id.clone(), o.clone());
                        queue.push(o.id.clone());
                    }
                }
            }
        }

        let mut expanded: Vec<Org> = collected.into_values().collect();
        expanded.sort_by(|a, b| a.sort.unwrap_or(0).cmp(&b.sort.unwrap_or(0)));

        let dtos: Vec<OrgTreeDto> = expanded.into_iter().map(OrgTreeDto::from).collect();
        Ok(build_org_tree(dtos))
    }

    pub async fn get_orgs_by_parent(&self, parent_id: Option<&str>) -> Result<Vec<Org>, AppError> {
        self.org_repo
            .find_by_parent_id(parent_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update_org(&self, id: &str, req: UpdateOrgRequest) -> Result<Org, AppError> {
        self.org_repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Org with id {} not found", id)))
    }

    pub async fn delete_org(&self, id: &str) -> Result<(), AppError> {
        let deleted = self
            .org_repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        if !deleted {
            return Err(AppError::NotFound(format!("Org with id {} not found", id)));
        }
        Ok(())
    }
}
