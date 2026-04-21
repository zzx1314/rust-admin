use crate::common::error::AppError;
use crate::common::traits::OrgRepository;
use crate::org::domain::{
    CreateOrgRequest, Org, OrgTreeDto, OrgTreeQuery, SysOrgVo, UpdateOrgRequest, build_org_tree,
};
use std::collections::HashMap;
use std::sync::Arc;

pub struct OrgService {
    org_repo: Arc<dyn OrgRepository>,
}

impl OrgService {
    pub fn new(org_repo: Arc<dyn OrgRepository>) -> Self {
        Self { org_repo }
    }

    pub async fn create_org(&self, req: CreateOrgRequest) -> Result<Org, AppError> {
        let id = self.generate_id().await;
        self.org_repo
            .create(&req, &id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_org(&self, id: &i64) -> Result<Org, AppError> {
        self.org_repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Org with id {} not found", id)))
    }

    pub async fn get_all_orgs(&self) -> Result<Vec<SysOrgVo>, AppError> {
        let orgs = self
            .org_repo
            .find_all()
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(orgs.into_iter().map(SysOrgVo::from).collect())
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

        let all_id_map: HashMap<i64, &Org> = all_orgs.iter().map(|o| (o.id, o)).collect();
        let mut collected: HashMap<i64, Org> = HashMap::new();

        for org in &matched {
            let mut current = Some(org);
            while let Some(o) = current {
                if collected.contains_key(&o.id) {
                    break;
                }
                collected.insert(o.id, o.clone());
                current = o.parent_id.and_then(|pid| {
                    if pid == 0 || pid == o.id {
                        None
                    } else {
                        all_id_map.get(&pid).copied()
                    }
                });
            }

            let mut queue: Vec<i64> = vec![org.id];
            let mut idx = 0;
            while idx < queue.len() {
                let parent_id = queue[idx];
                idx += 1;
                for o in &all_orgs {
                    if let Some(ref pid) = o.parent_id
                        && *pid == parent_id
                        && !collected.contains_key(&o.id)
                    {
                        collected.insert(o.id, o.clone());
                        queue.push(o.id);
                    }
                }
            }
        }

        let mut expanded: Vec<Org> = collected.into_values().collect();
        expanded.sort_by(|a, b| a.sort.unwrap_or(0).cmp(&b.sort.unwrap_or(0)));

        let dtos: Vec<OrgTreeDto> = expanded.into_iter().map(OrgTreeDto::from).collect();
        Ok(build_org_tree(dtos))
    }

    pub async fn get_orgs_by_parent(&self, parent_id: Option<i64>) -> Result<Vec<Org>, AppError> {
        self.org_repo
            .find_by_parent_id(parent_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update_org(&self, id: &i64, req: UpdateOrgRequest) -> Result<Org, AppError> {
        self.org_repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Org with id {} not found", id)))
    }

    pub async fn delete_org(&self, id: &i64) -> Result<(), AppError> {
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

    async fn generate_id(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}
