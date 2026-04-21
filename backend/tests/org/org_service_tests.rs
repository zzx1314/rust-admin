use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, OrgRepository, SeaOrmOptResult, SeaOrmResult};
use x_rust::org::domain::{CreateOrgRequest, Org, OrgTreeDto, OrgTreeQuery, UpdateOrgRequest};
use x_rust::org::service::OrgService;

struct FakeOrgRepository {
    data: Arc<Mutex<HashMap<i64, Org>>>,
}

fn flatten_org_tree_names(orgs: &[OrgTreeDto]) -> Vec<&str> {
    let mut names = Vec::new();
    for org in orgs {
        names.push(org.name.as_str());
        if let Some(ref children) = org.children {
            names.extend(flatten_org_tree_names(children));
        }
    }
    names
}

impl FakeOrgRepository {
    fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl OrgRepository for FakeOrgRepository {
    fn create(&self, req: &CreateOrgRequest, id: &i64) -> DynFuture<SeaOrmResult<Org>> {
        let data = self.data.clone();
        let id_value = *id;
        let name = req.name.clone();
        let sort = req.sort;
        let parent_id = req.parent_id;
        let parent_name = req.parent_name.clone();
        let org_duty = req.org_duty.clone();
        let desrc = req.desrc.clone();
        let r#type = req.r#type.clone();
        let remarks = req.remarks.clone();
        Box::pin(async move {
            let now = Utc::now();
            let org = Org {
                id: id_value,
                name,
                sort,
                parent_id,
                parent_name,
                org_duty,
                desrc,
                r#type,
                create_time: now,
                update_time: now,
                is_deleted: 0,
                remarks,
            };
            data.lock().unwrap().insert(id_value, org.clone());
            Ok(org)
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<Org>> {
        let data = self.data.clone();
        let id = *id;
        Box::pin(async move { Ok(data.lock().unwrap().get(&id).cloned()) })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<Org>>> {
        let data = self.data.clone();
        Box::pin(async move {
            Ok(data
                .lock()
                .unwrap()
                .values()
                .filter(|o| o.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn find_by_parent_id(&self, parent_id: Option<i64>) -> DynFuture<SeaOrmResult<Vec<Org>>> {
        let data = self.data.clone();
        let parent_id = parent_id;
        Box::pin(async move {
            Ok(data
                .lock()
                .unwrap()
                .values()
                .filter(|o| {
                    o.is_deleted == 0
                        && match &parent_id {
                            Some(pid) => o.parent_id.as_ref() == Some(pid),
                            None => o.parent_id.is_none(),
                        }
                })
                .cloned()
                .collect())
        })
    }

    fn find_tree(&self) -> DynFuture<SeaOrmResult<Vec<Org>>> {
        let data = self.data.clone();
        Box::pin(async move {
            Ok(data
                .lock()
                .unwrap()
                .values()
                .filter(|o| o.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn find_tree_with_filter(&self, query: &OrgTreeQuery) -> DynFuture<SeaOrmResult<Vec<Org>>> {
        let data = self.data.clone();
        let name = query.name.clone();
        let r#type = query.r#type.clone();
        Box::pin(async move {
            let all: Vec<Org> = data
                .lock()
                .unwrap()
                .values()
                .filter(|o| o.is_deleted == 0)
                .cloned()
                .collect();

            if name.is_none() && r#type.is_none() {
                return Ok(all);
            }

            let filtered = all
                .into_iter()
                .filter(|o| {
                    let name_match = name.as_ref().map(|n| o.name.contains(n)).unwrap_or(false);
                    let type_match = r#type
                        .as_ref()
                        .map(|t| o.r#type.as_ref() == Some(t))
                        .unwrap_or(false);
                    name_match || type_match
                })
                .collect();
            Ok(filtered)
        })
    }

    fn update(&self, id: &i64, req: &UpdateOrgRequest) -> DynFuture<SeaOrmOptResult<Org>> {
        let data = self.data.clone();
        let id = *id;
        let name = req.name.clone();
        let sort = req.sort;
        let parent_id = req.parent_id;
        let parent_name = req.parent_name.clone();
        let org_duty = req.org_duty.clone();
        let desrc = req.desrc.clone();
        let r#type = req.r#type.clone();
        let is_out = req.is_out;
        let remarks = req.remarks.clone();
        Box::pin(async move {
            let mut data_lock = data.lock().unwrap();
            if let Some(org) = data_lock.get_mut(&id) {
                if let Some(v) = name {
                    org.name = v;
                }
                if let Some(v) = sort {
                    org.sort = Some(v);
                }
                if let Some(v) = parent_id {
                    org.parent_id = Some(v);
                }
                if let Some(v) = parent_name {
                    org.parent_name = Some(v);
                }
                if let Some(v) = org_duty {
                    org.org_duty = Some(v);
                }
                if let Some(v) = desrc {
                    org.desrc = Some(v);
                }
                if let Some(v) = r#type {
                    org.r#type = Some(v);
                }
                if let Some(v) = remarks {
                    org.remarks = Some(v);
                }
                org.update_time = Utc::now();
                Ok(Some(org.clone()))
            } else {
                Ok(None)
            }
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let data = self.data.clone();
        let id = *id;
        Box::pin(async move {
            let mut data_lock = data.lock().unwrap();
            if let Some(org) = data_lock.get_mut(&id) {
                org.is_deleted = 1;
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }
}

#[tokio::test]
async fn test_create_org_success() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo);
    let req = CreateOrgRequest {
        name: "Engineering".to_string(),
        sort: Some(1),
        parent_id: None,
        parent_name: None,
        org_duty: Some("Development".to_string()),
        desrc: Some("Engineering department".to_string()),
        r#type: Some("department".to_string()),
        is_out: Some(false),
        remarks: None,
    };
    let result = service.create_org(req).await.unwrap();
    assert_eq!(result.name, "Engineering");
    assert_eq!(result.is_deleted, 0);
}

#[tokio::test]
async fn test_get_org_success() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    let org_id = 1i64;
    repo.create(
        &CreateOrgRequest {
            name: "Sales".to_string(),
            sort: None,
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &org_id,
    )
    .await
    .unwrap();
    let result = service.get_org(&org_id).await.unwrap();
    assert_eq!(result.id, org_id);
    assert_eq!(result.name, "Sales");
}

#[tokio::test]
async fn test_get_org_not_found() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo);
    let result = service.get_org(&9999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_all_orgs() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    repo.create(
        &CreateOrgRequest {
            name: "Org 1".to_string(),
            sort: None,
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Org 2".to_string(),
            sort: None,
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &2i64,
    )
    .await
    .unwrap();
    let result = service.get_all_orgs().await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_get_org_tree_no_filter() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    repo.create(
        &CreateOrgRequest {
            name: "Root".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Child".to_string(),
            sort: Some(2),
            parent_id: Some(1i64),
            parent_name: Some("Root".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &2i64,
    )
    .await
    .unwrap();
    let query = OrgTreeQuery {
        name: None,
        r#type: None,
    };
    let result = service.get_org_tree(query).await.unwrap();
    let names = flatten_org_tree_names(&result);
    assert!(names.contains(&"Root"));
    assert!(names.contains(&"Child"));
}

#[tokio::test]
async fn test_get_org_tree_with_name_filter() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    repo.create(
        &CreateOrgRequest {
            name: "Company".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Engineering Dept".to_string(),
            sort: Some(2),
            parent_id: Some(1i64),
            parent_name: Some("Company".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &2i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Backend Team".to_string(),
            sort: Some(3),
            parent_id: Some(2i64),
            parent_name: Some("Engineering Dept".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &3i64,
    )
    .await
    .unwrap();
    let query = OrgTreeQuery {
        name: Some("Backend".to_string()),
        r#type: None,
    };
    let result = service.get_org_tree(query).await.unwrap();
    let names = flatten_org_tree_names(&result);
    assert!(
        names.contains(&"Company"),
        "missing Company, got {:?}",
        names
    );
    assert!(
        names.contains(&"Engineering Dept"),
        "missing Engineering Dept, got {:?}",
        names
    );
    assert!(
        names.contains(&"Backend Team"),
        "missing Backend Team, got {:?}",
        names
    );
}

#[tokio::test]
async fn test_get_org_tree_with_type_filter() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    repo.create(
        &CreateOrgRequest {
            name: "Company".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: Some("company".to_string()),
            is_out: None,
            remarks: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Dept A".to_string(),
            sort: Some(2),
            parent_id: Some(1i64),
            parent_name: Some("Company".to_string()),
            org_duty: None,
            desrc: None,
            r#type: Some("department".to_string()),
            is_out: None,
            remarks: None,
        },
        &2i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Dept B".to_string(),
            sort: Some(3),
            parent_id: Some(1i64),
            parent_name: Some("Company".to_string()),
            org_duty: None,
            desrc: None,
            r#type: Some("department".to_string()),
            is_out: None,
            remarks: None,
        },
        &3i64,
    )
    .await
    .unwrap();
    let query = OrgTreeQuery {
        name: None,
        r#type: Some("department".to_string()),
    };
    let result = service.get_org_tree(query).await.unwrap();
    let names = flatten_org_tree_names(&result);
    assert!(
        names.contains(&"Company"),
        "missing Company, got {:?}",
        names
    );
    assert!(names.contains(&"Dept A"), "missing Dept A, got {:?}", names);
    assert!(names.contains(&"Dept B"), "missing Dept B, got {:?}", names);
}

#[tokio::test]
async fn test_get_org_tree_filter_expands_ancestors() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    repo.create(
        &CreateOrgRequest {
            name: "Grandparent".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Parent".to_string(),
            sort: Some(2),
            parent_id: Some(1i64),
            parent_name: Some("Grandparent".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &2i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Child".to_string(),
            sort: Some(3),
            parent_id: Some(2i64),
            parent_name: Some("Parent".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &3i64,
    )
    .await
    .unwrap();
    let query = OrgTreeQuery {
        name: Some("Child".to_string()),
        r#type: None,
    };
    let result = service.get_org_tree(query).await.unwrap();
    let names = flatten_org_tree_names(&result);
    assert!(
        names.contains(&"Grandparent"),
        "missing Grandparent, got {:?}",
        names
    );
    assert!(names.contains(&"Parent"), "missing Parent, got {:?}", names);
    assert!(names.contains(&"Child"), "missing Child, got {:?}", names);
}

#[tokio::test]
async fn test_get_org_tree_filter_expands_descendants() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    repo.create(
        &CreateOrgRequest {
            name: "Root Match".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Child 1".to_string(),
            sort: Some(2),
            parent_id: Some(1i64),
            parent_name: Some("Root Match".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &2i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Grandchild".to_string(),
            sort: Some(3),
            parent_id: Some(2i64),
            parent_name: Some("Child 1".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &3i64,
    )
    .await
    .unwrap();
    let query = OrgTreeQuery {
        name: Some("Root Match".to_string()),
        r#type: None,
    };
    let result = service.get_org_tree(query).await.unwrap();
    let names = flatten_org_tree_names(&result);
    assert!(
        names.contains(&"Root Match"),
        "missing Root Match, got {:?}",
        names
    );
    assert!(
        names.contains(&"Child 1"),
        "missing Child 1, got {:?}",
        names
    );
    assert!(
        names.contains(&"Grandchild"),
        "missing Grandchild, got {:?}",
        names
    );
}

#[tokio::test]
async fn test_get_orgs_by_parent() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    repo.create(
        &CreateOrgRequest {
            name: "Parent".to_string(),
            sort: None,
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &1i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Child 1".to_string(),
            sort: None,
            parent_id: Some(1i64),
            parent_name: Some("Parent".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &2i64,
    )
    .await
    .unwrap();
    repo.create(
        &CreateOrgRequest {
            name: "Child 2".to_string(),
            sort: None,
            parent_id: Some(1i64),
            parent_name: Some("Parent".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &3i64,
    )
    .await
    .unwrap();
    let result = service.get_orgs_by_parent(Some(1)).await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_update_org_success() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    let org_id = 1i64;
    repo.create(
        &CreateOrgRequest {
            name: "Old Name".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &org_id,
    )
    .await
    .unwrap();
    let req = UpdateOrgRequest {
        name: Some("New Name".to_string()),
        sort: Some(10),
        parent_id: None,
        parent_name: None,
        org_duty: None,
        desrc: None,
        r#type: None,
        is_out: None,
        remarks: None,
    };
    let result = service.update_org(&org_id, req).await.unwrap();
    assert_eq!(result.name, "New Name");
    assert_eq!(result.sort, Some(10));
}

#[tokio::test]
async fn test_update_org_not_found() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo);
    let req = UpdateOrgRequest {
        name: Some("Updated".to_string()),
        sort: None,
        parent_id: None,
        parent_name: None,
        org_duty: None,
        desrc: None,
        r#type: None,
        is_out: None,
        remarks: None,
    };
    let result = service.update_org(&9999i64, req).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_org_success() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo.clone());
    let org_id = 1i64;
    repo.create(
        &CreateOrgRequest {
            name: "Temp Org".to_string(),
            sort: None,
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        &org_id,
    )
    .await
    .unwrap();
    service.delete_org(&org_id).await.unwrap();
    let all = service.get_all_orgs().await.unwrap();
    assert_eq!(all.len(), 0);
}

#[tokio::test]
async fn test_delete_org_not_found() {
    let repo = Arc::new(FakeOrgRepository::new());
    let service = OrgService::new(repo);
    let result = service.delete_org(&9999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}
