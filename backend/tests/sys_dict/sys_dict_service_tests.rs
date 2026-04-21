use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, SeaOrmOptResult, SeaOrmResult, SysDictRepository};
use x_rust::system::sys_dict::domain::{
    CreateSysDictRequest, SysDict, SysDictPageQuery, SysDictVO, UpdateSysDictRequest,
};
use x_rust::system::sys_dict::service::SysDictService;

// ==================== Fake SysDict Repository ====================

struct FakeSysDictRepository {
    dicts: Arc<Mutex<HashMap<i64, SysDict>>>,
}

impl FakeSysDictRepository {
    fn new() -> Self {
        Self {
            dicts: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl SysDictRepository for FakeSysDictRepository {
    fn create(&self, req: &CreateSysDictRequest, id: &i64) -> DynFuture<SeaOrmResult<SysDict>> {
        let dicts = self.dicts.clone();
        let req = req.clone();
        let id = *id;
        Box::pin(async move {
            let dict = SysDict {
                id,
                r#type: req.r#type.clone(),
                dict_type: req.dict_type.clone(),
                description: req.description.clone(),
                remarks: req.remarks.clone(),
                create_time: Some(Utc::now()),
                update_time: Some(Utc::now()),
                is_deleted: 0,
                allow_deletion: req.allow_deletion,
                is_show: req.is_show,
            };
            dicts.lock().unwrap().insert(id, dict.clone());
            Ok(dict)
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<SysDict>> {
        let dicts = self.dicts.clone();
        let id = *id;
        Box::pin(async move {
            let dict = dicts.lock().unwrap().get(&id).cloned();
            if let Some(ref d) = dict {
                if d.is_deleted == 0 {
                    return Ok(dict);
                }
            }
            Ok(None)
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<SysDict>>> {
        let dicts = self.dicts.clone();
        Box::pin(async move {
            Ok(dicts
                .lock()
                .unwrap()
                .values()
                .filter(|d| d.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn find_all_with_page(
        &self,
        query: &SysDictPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<SysDictVO>, i64)>> {
        let dicts = self.dicts.clone();
        let query = query.clone();
        Box::pin(async move {
            let mut vec: Vec<SysDictVO> = dicts
                .lock()
                .unwrap()
                .values()
                .filter(|d| d.is_deleted == 0)
                .filter(|d| {
                    if let Some(ref v) = query.r#type {
                        if !d.r#type.contains(v.as_str()) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.description {
                        if !d
                            .description
                            .as_ref()
                            .map_or(false, |desc| desc.contains(v.as_str()))
                        {
                            return false;
                        }
                    }
                    true
                })
                .map(|d| SysDictVO {
                    id: d.id,
                    r#type: d.r#type.clone(),
                    dict_type: d.dict_type.clone(),
                    description: d.description.clone(),
                    remarks: d.remarks.clone(),
                    create_time: d.create_time.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    update_time: d.update_time.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    is_deleted: d.is_deleted,
                    allow_deletion: d.allow_deletion,
                    is_show: d.is_show,
                })
                .collect();
            vec.sort_by(|a, b| b.id.cmp(&a.id));
            let total = vec.len() as i64;
            let offset = (query.page() - 1) * query.size();
            let records: Vec<SysDictVO> = vec
                .iter()
                .skip(offset as usize)
                .take(query.size() as usize)
                .cloned()
                .collect();
            Ok((records, total))
        })
    }

    fn update(&self, id: &i64, req: &UpdateSysDictRequest) -> DynFuture<SeaOrmOptResult<SysDict>> {
        let dicts = self.dicts.clone();
        let id = *id;
        let r#type = req.r#type.clone();
        let dict_type = req.dict_type.clone();
        let description = req.description.clone();
        let remarks = req.remarks.clone();
        let allow_deletion = req.allow_deletion;
        let is_show = req.is_show;
        Box::pin(async move {
            let mut dicts_lock = dicts.lock().unwrap();
            if let Some(dict) = dicts_lock.get_mut(&id) {
                if let Some(v) = r#type {
                    dict.r#type = v;
                }
                if let Some(v) = dict_type {
                    dict.dict_type = Some(v);
                }
                if let Some(v) = description {
                    dict.description = Some(v);
                }
                if let Some(v) = remarks {
                    dict.remarks = Some(v);
                }
                if let Some(v) = allow_deletion {
                    dict.allow_deletion = Some(v);
                }
                if let Some(v) = is_show {
                    dict.is_show = Some(v);
                }
                dict.update_time = Some(Utc::now());
                Ok(Some(dict.clone()))
            } else {
                Ok(None)
            }
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let dicts = self.dicts.clone();
        let id = *id;
        Box::pin(async move {
            let mut dicts_lock = dicts.lock().unwrap();
            if let Some(dict) = dicts_lock.get_mut(&id) {
                dict.is_deleted = 1;
                dict.update_time = Some(Utc::now());
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }
}

// ==================== SysDict Service Tests ====================

#[tokio::test]
async fn test_create_dict_success() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo);

    let req = CreateSysDictRequest {
        r#type: "user_status".to_string(),
        dict_type: Some("用户状态".to_string()),
        description: Some("用户状态字典".to_string()),
        remarks: None,
        allow_deletion: Some(1),
        is_show: Some(1),
    };

    let result = service.create_dict(req).await;
    assert!(result.is_ok());
    let dict = result.unwrap();
    assert_eq!(dict.r#type, "user_status");
    assert_eq!(dict.dict_type, Some("用户状态".to_string()));
}

#[tokio::test]
async fn test_get_dict_success() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo.clone());

    let req = CreateSysDictRequest {
        r#type: "gender".to_string(),
        dict_type: Some("性别".to_string()),
        description: Some("性别字典".to_string()),
        remarks: None,
        allow_deletion: Some(1),
        is_show: Some(1),
    };
    repo.create(&req, &1i64).await.unwrap();

    let result = service.get_dict(&1i64).await.unwrap();
    assert_eq!(result.r#type, "gender");
}

#[tokio::test]
async fn test_get_dict_not_found() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo);

    let result = service.get_dict(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_all_dicts() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo.clone());

    let req1 = CreateSysDictRequest {
        r#type: "status1".to_string(),
        dict_type: Some("状态1".to_string()),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
        is_show: Some(1),
    };
    repo.create(&req1, &1i64).await.unwrap();

    let req2 = CreateSysDictRequest {
        r#type: "status2".to_string(),
        dict_type: Some("状态2".to_string()),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
        is_show: Some(1),
    };
    repo.create(&req2, &2i64).await.unwrap();

    let result = service.get_all_dicts().await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_update_dict_success() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo.clone());

    let req = CreateSysDictRequest {
        r#type: "original".to_string(),
        dict_type: Some("原始".to_string()),
        description: Some("原始描述".to_string()),
        remarks: None,
        allow_deletion: Some(1),
        is_show: Some(1),
    };
    repo.create(&req, &1i64).await.unwrap();

    let update_req = UpdateSysDictRequest {
        r#type: Some("updated".to_string()),
        dict_type: Some("更新".to_string()),
        description: Some("更新描述".to_string()),
        remarks: None,
        allow_deletion: None,
        is_show: None,
    };
    let result = service.update_dict(&1i64, update_req).await.unwrap();
    assert_eq!(result.r#type, "updated");
    assert_eq!(result.dict_type, Some("更新".to_string()));
}

#[tokio::test]
async fn test_update_dict_not_found() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo);

    let req = UpdateSysDictRequest {
        r#type: Some("updated".to_string()),
        dict_type: None,
        description: None,
        remarks: None,
        allow_deletion: None,
        is_show: None,
    };

    let result = service.update_dict(&999i64, req).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_dict_success() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo.clone());

    let req = CreateSysDictRequest {
        r#type: "deleteme".to_string(),
        dict_type: Some("删除".to_string()),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
        is_show: Some(1),
    };
    repo.create(&req, &1i64).await.unwrap();

    let result = service.delete_dict(&1i64).await;
    assert!(result.is_ok());

    let find_result = service.get_dict(&1i64).await;
    assert!(matches!(find_result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_dict_not_found() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo);

    let result = service.delete_dict(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_dicts_page_default() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo.clone());

    for i in 1..=15 {
        let req = CreateSysDictRequest {
            r#type: format!("type{}", i),
            dict_type: Some(format!("类型{}", i)),
            description: None,
            remarks: None,
            allow_deletion: Some(1),
            is_show: Some(1),
        };
        repo.create(&req, &(i as i64)).await.unwrap();
    }

    let result = service
        .get_dicts_page(SysDictPageQuery {
            current: 1,
            size: 10,
            r#type: None,
            description: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 10);
    assert_eq!(result.total, 15);
    assert_eq!(result.current, 1);
    assert_eq!(result.size, 10);
}

#[tokio::test]
async fn test_get_dicts_page_empty() {
    let repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictService::new(repo);

    let result = service
        .get_dicts_page(SysDictPageQuery {
            current: 1,
            size: 10,
            r#type: None,
            description: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 0);
    assert_eq!(result.total, 0);
}
