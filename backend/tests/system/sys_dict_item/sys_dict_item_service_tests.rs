use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::common::error::AppError;
use x_rust::common::traits::{
    DynFuture, SeaOrmOptResult, SeaOrmResult, SysDictItemRepository, SysDictRepository,
};
use x_rust::system::sys_dict::domain::{
    CreateSysDictRequest, SysDict, SysDictPageQuery, SysDictVO, UpdateSysDictRequest,
};
use x_rust::system::sys_dict_item::domain::{
    CreateSysDictItemRequest, SysDictItem, SysDictItemPageQuery, SysDictItemVO,
    UpdateSysDictItemRequest,
};
use x_rust::system::sys_dict_item::service::SysDictItemService;

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
        let id = *id;
        let r#type = req.r#type.clone();
        let dict_type = req.dict_type.clone();
        let description = req.description.clone();
        let remarks = req.remarks.clone();
        let allow_deletion = req.allow_deletion;
        let is_show = req.is_show;
        Box::pin(async move {
            let dict = SysDict {
                id,
                r#type,
                dict_type,
                description,
                remarks,
                create_time: Some(Utc::now()),
                update_time: Some(Utc::now()),
                is_deleted: 0,
                allow_deletion,
                is_show,
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

    fn find_by_type(&self, r#type: &str) -> DynFuture<SeaOrmOptResult<SysDict>> {
        let dicts = self.dicts.clone();
        let r#type = r#type.to_string();
        Box::pin(async move {
            let dict = dicts
                .lock()
                .unwrap()
                .values()
                .filter(|d| d.is_deleted == 0 && d.r#type == r#type)
                .cloned()
                .next();
            Ok(dict)
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
            let vec: Vec<SysDictVO> = dicts
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
                        if !d.description.as_ref().map_or(false, |desc| desc.contains(v.as_str())) {
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
            let total = vec.len() as i64;
            let offset = (query.page() - 1) * query.size();
            let records: Vec<SysDictVO> = vec
                .into_iter()
                .skip(offset as usize)
                .take(query.size() as usize)
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

// ==================== Fake SysDictItem Repository ====================

struct FakeSysDictItemRepository {
    items: Arc<Mutex<HashMap<i64, SysDictItem>>>,
}

impl FakeSysDictItemRepository {
    fn new() -> Self {
        Self {
            items: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl SysDictItemRepository for FakeSysDictItemRepository {
    fn create(
        &self,
        req: &CreateSysDictItemRequest,
        id: &i64,
    ) -> DynFuture<SeaOrmResult<SysDictItem>> {
        let items = self.items.clone();
        let req = req.clone();
        let id = *id;
        Box::pin(async move {
            let item = SysDictItem {
                id,
                r#type: req.r#type.clone(),
                label: req.label.clone(),
                dict_id: req.dict_id,
                value: req.value.clone(),
                sort: req.sort.unwrap_or(0),
                description: req.description.clone(),
                create_time: Some(Utc::now()),
                update_time: Some(Utc::now()),
                is_deleted: 0,
                remarks: req.remarks.clone(),
                allow_deletion: req.allow_deletion,
            };
            items.lock().unwrap().insert(id, item.clone());
            Ok(item)
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<SysDictItem>> {
        let items = self.items.clone();
        let id = *id;
        Box::pin(async move {
            let item = items.lock().unwrap().get(&id).cloned();
            if let Some(ref i) = item {
                if i.is_deleted == 0 {
                    return Ok(item);
                }
            }
            Ok(None)
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<SysDictItem>>> {
        let items = self.items.clone();
        Box::pin(async move {
            Ok(items
                .lock()
                .unwrap()
                .values()
                .filter(|i| i.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn find_by_dict_id(&self, dict_id: &i64) -> DynFuture<SeaOrmResult<Vec<SysDictItem>>> {
        let items = self.items.clone();
        let dict_id = *dict_id;
        Box::pin(async move {
            Ok(items
                .lock()
                .unwrap()
                .values()
                .filter(|i| i.is_deleted == 0 && i.dict_id == Some(dict_id))
                .cloned()
                .collect())
        })
    }

    fn find_by_type(&self, r#type: &str) -> DynFuture<SeaOrmResult<Vec<SysDictItem>>> {
        let items = self.items.clone();
        let r#type = r#type.to_string();
        Box::pin(async move {
            Ok(items
                .lock()
                .unwrap()
                .values()
                .filter(|i| i.is_deleted == 0 && i.r#type == r#type)
                .cloned()
                .collect())
        })
    }

    fn find_all_with_page(
        &self,
        query: &SysDictItemPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<SysDictItemVO>, i64)>> {
        let items = self.items.clone();
        let query = query.clone();
        Box::pin(async move {
            let mut vec: Vec<SysDictItemVO> = items
                .lock()
                .unwrap()
                .values()
                .filter(|i| i.is_deleted == 0)
                .filter(|i| {
                    if let Some(v) = query.dict_id {
                        if i.dict_id != Some(v) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.r#type {
                        if !i.r#type.contains(v.as_str()) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.label {
                        if !i.label.as_ref().map_or(false, |l| l.contains(v.as_str())) {
                            return false;
                        }
                    }
                    true
                })
                .map(|i| SysDictItemVO {
                    id: i.id,
                    r#type: i.r#type.clone(),
                    label: i.label.clone(),
                    dict_id: i.dict_id,
                    value: i.value.clone(),
                    sort: i.sort,
                    description: i.description.clone(),
                    create_time: i.create_time.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    update_time: i.update_time.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    is_deleted: i.is_deleted,
                    remarks: i.remarks.clone(),
                    allow_deletion: i.allow_deletion,
                })
                .collect();
            vec.sort_by(|a, b| a.sort.cmp(&b.sort));
            let total = vec.len() as i64;
            let offset = (query.page() - 1) * query.size();
            let records: Vec<SysDictItemVO> = vec
                .iter()
                .skip(offset as usize)
                .take(query.size() as usize)
                .cloned()
                .collect();
            Ok((records, total))
        })
    }

    fn update(
        &self,
        id: &i64,
        req: &UpdateSysDictItemRequest,
    ) -> DynFuture<SeaOrmOptResult<SysDictItem>> {
        let items = self.items.clone();
        let id = *id;
        let r#type = req.r#type.clone();
        let label = req.label.clone();
        let dict_id = req.dict_id;
        let value = req.value.clone();
        let sort = req.sort;
        let description = req.description.clone();
        let remarks = req.remarks.clone();
        let allow_deletion = req.allow_deletion;
        Box::pin(async move {
            let mut items_lock = items.lock().unwrap();
            if let Some(item) = items_lock.get_mut(&id) {
                if let Some(v) = r#type {
                    item.r#type = v;
                }
                if let Some(v) = label {
                    item.label = Some(v);
                }
                if let Some(v) = dict_id {
                    item.dict_id = Some(v);
                }
                if let Some(v) = value {
                    item.value = Some(v);
                }
                if let Some(v) = sort {
                    item.sort = v;
                }
                if let Some(v) = description {
                    item.description = Some(v);
                }
                if let Some(v) = remarks {
                    item.remarks = Some(v);
                }
                if let Some(v) = allow_deletion {
                    item.allow_deletion = Some(v);
                }
                item.update_time = Some(Utc::now());
                Ok(Some(item.clone()))
            } else {
                Ok(None)
            }
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let items = self.items.clone();
        let id = *id;
        Box::pin(async move {
            let mut items_lock = items.lock().unwrap();
            if let Some(item) = items_lock.get_mut(&id) {
                item.is_deleted = 1;
                item.update_time = Some(Utc::now());
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }
}

#[tokio::test]
async fn test_create_dict_item_success() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo, dict_repo);

    let req = CreateSysDictItemRequest {
        r#type: "gender".to_string(),
        label: Some("男".to_string()),
        dict_id: Some(1),
        value: Some("1".to_string()),
        sort: Some(1),
        description: Some("男性".to_string()),
        remarks: None,
        allow_deletion: Some(1),
    };

    let result = service.create_dict_item(req).await;
    assert!(result.is_ok());
    let item = result.unwrap();
    assert_eq!(item.label, Some("男".to_string()));
    assert_eq!(item.value, Some("1".to_string()));
}

#[tokio::test]
async fn test_get_dict_item_success() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo.clone(), dict_repo);

    let req = CreateSysDictItemRequest {
        r#type: "gender".to_string(),
        label: Some("女".to_string()),
        dict_id: Some(1),
        value: Some("2".to_string()),
        sort: Some(2),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req, &1i64).await.unwrap();

    let result = service.get_dict_item(&1i64).await.unwrap();
    assert_eq!(result.label, Some("女".to_string()));
}

#[tokio::test]
async fn test_get_dict_item_not_found() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo, dict_repo);

    let result = service.get_dict_item(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_all_dict_items() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo.clone(), dict_repo);

    let req1 = CreateSysDictItemRequest {
        r#type: "status".to_string(),
        label: Some("启用".to_string()),
        dict_id: Some(1),
        value: Some("1".to_string()),
        sort: Some(1),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req1, &1i64).await.unwrap();

    let req2 = CreateSysDictItemRequest {
        r#type: "status".to_string(),
        label: Some("禁用".to_string()),
        dict_id: Some(1),
        value: Some("0".to_string()),
        sort: Some(2),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req2, &2i64).await.unwrap();

    let result = service.get_all_dict_items().await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_get_dict_items_by_dict_id() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo.clone(), dict_repo);

    let req1 = CreateSysDictItemRequest {
        r#type: "gender".to_string(),
        label: Some("男".to_string()),
        dict_id: Some(1),
        value: Some("1".to_string()),
        sort: Some(1),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req1, &1i64).await.unwrap();

    let req2 = CreateSysDictItemRequest {
        r#type: "gender".to_string(),
        label: Some("女".to_string()),
        dict_id: Some(1),
        value: Some("2".to_string()),
        sort: Some(2),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req2, &2i64).await.unwrap();

    let req3 = CreateSysDictItemRequest {
        r#type: "status".to_string(),
        label: Some("启用".to_string()),
        dict_id: Some(2),
        value: Some("1".to_string()),
        sort: Some(1),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req3, &3i64).await.unwrap();

    let result = service.get_dict_items_by_dict_id(&1i64).await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_get_dict_items_by_type() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo.clone(), dict_repo);

    let req1 = CreateSysDictItemRequest {
        r#type: "gender".to_string(),
        label: Some("男".to_string()),
        dict_id: Some(1),
        value: Some("1".to_string()),
        sort: Some(1),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req1, &1i64).await.unwrap();

    let req2 = CreateSysDictItemRequest {
        r#type: "gender".to_string(),
        label: Some("女".to_string()),
        dict_id: Some(1),
        value: Some("2".to_string()),
        sort: Some(2),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req2, &2i64).await.unwrap();

    let result = service.get_dict_items_by_type("gender").await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_update_dict_item_success() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo.clone(), dict_repo);

    let req = CreateSysDictItemRequest {
        r#type: "gender".to_string(),
        label: Some("原始".to_string()),
        dict_id: Some(1),
        value: Some("1".to_string()),
        sort: Some(1),
        description: Some("原始描述".to_string()),
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req, &1i64).await.unwrap();

    let update_req = UpdateSysDictItemRequest {
        r#type: Some("gender".to_string()),
        label: Some("更新".to_string()),
        dict_id: None,
        value: Some("0".to_string()),
        sort: Some(10),
        description: Some("更新描述".to_string()),
        remarks: None,
        allow_deletion: None,
    };
    let result = service.update_dict_item(&1i64, update_req).await.unwrap();
    assert_eq!(result.label, Some("更新".to_string()));
    assert_eq!(result.sort, 10);
}

#[tokio::test]
async fn test_update_dict_item_not_found() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo, dict_repo);

    let req = UpdateSysDictItemRequest {
        r#type: Some("gender".to_string()),
        label: Some("更新".to_string()),
        dict_id: None,
        value: None,
        sort: None,
        description: None,
        remarks: None,
        allow_deletion: None,
    };

    let result = service.update_dict_item(&999i64, req).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_dict_item_success() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo.clone(), dict_repo);

    let req = CreateSysDictItemRequest {
        r#type: "gender".to_string(),
        label: Some("删除".to_string()),
        dict_id: Some(1),
        value: Some("1".to_string()),
        sort: Some(1),
        description: None,
        remarks: None,
        allow_deletion: Some(1),
    };
    repo.create(&req, &1i64).await.unwrap();

    let result = service.delete_dict_item(&1i64).await;
    assert!(result.is_ok());

    let find_result = service.get_dict_item(&1i64).await;
    assert!(matches!(find_result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_dict_item_not_found() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo, dict_repo);

    let result = service.delete_dict_item(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_dict_items_page_default() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo.clone(), dict_repo);

    for i in 1..=15 {
        let req = CreateSysDictItemRequest {
            r#type: "type1".to_string(),
            label: Some(format!("label{}", i)),
            dict_id: Some(1),
            value: Some(format!("{}", i)),
            sort: Some(i),
            description: None,
            remarks: None,
            allow_deletion: Some(1),
        };
        repo.create(&req, &(i as i64)).await.unwrap();
    }

    let result = service
        .get_dict_items_page(SysDictItemPageQuery {
            current: 1,
            size: 10,
            dict_id: None,
            r#type: None,
            label: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 10);
    assert_eq!(result.total, 15);
    assert_eq!(result.current, 1);
    assert_eq!(result.size, 10);
}

#[tokio::test]
async fn test_get_dict_items_page_empty() {
    let repo = Arc::new(FakeSysDictItemRepository::new());
    let dict_repo = Arc::new(FakeSysDictRepository::new());
    let service = SysDictItemService::new(repo, dict_repo);

    let result = service
        .get_dict_items_page(SysDictItemPageQuery {
            current: 1,
            size: 10,
            dict_id: None,
            r#type: None,
            label: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 0);
    assert_eq!(result.total, 0);
}
