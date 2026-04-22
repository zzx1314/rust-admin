use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, SeaOrmOptResult, SeaOrmResult, SysLogRepository};
use x_rust::system::sys_log::domain::{
    CreateSysLogRequest, SysLog, SysLogPageQuery, SysLogVO, UpdateSysLogRequest,
};
use x_rust::system::sys_log::service::SysLogService;

// ==================== Fake SysLog Repository ====================

struct FakeSysLogRepository {
    logs: Arc<Mutex<HashMap<i64, SysLog>>>,
}

impl FakeSysLogRepository {
    fn new() -> Self {
        Self {
            logs: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl SysLogRepository for FakeSysLogRepository {
    fn create(&self, req: &CreateSysLogRequest, id: &i64) -> DynFuture<SeaOrmResult<SysLog>> {
        let logs = self.logs.clone();
        let req = req.clone();
        let id = *id;
        Box::pin(async move {
            let log = SysLog {
                id,
                tenant: req.tenant.clone(),
                type_: req.type_.clone(),
                sub_type: req.sub_type.clone(),
                biz_no: req.biz_no.clone(),
                operator: req.operator.clone(),
                action: req.action.clone(),
                fail: req.fail.unwrap_or(false),
                create_time: Some(Utc::now()),
                extra: req.extra.clone(),
                code_variable: req.code_variable.clone(),
                ip: req.ip.clone(),
                is_deleted: 0,
            };
            logs.lock().unwrap().insert(id, log.clone());
            Ok(log)
        })
    }

    fn find_by_id(&self, id: &i64) -> DynFuture<SeaOrmOptResult<SysLog>> {
        let logs = self.logs.clone();
        let id = *id;
        Box::pin(async move {
            let log = logs.lock().unwrap().get(&id).cloned();
            if let Some(ref l) = log {
                if l.is_deleted == 0 {
                    return Ok(log);
                }
            }
            Ok(None)
        })
    }

    fn find_all(&self) -> DynFuture<SeaOrmResult<Vec<SysLog>>> {
        let logs = self.logs.clone();
        Box::pin(async move {
            Ok(logs
                .lock()
                .unwrap()
                .values()
                .filter(|l| l.is_deleted == 0)
                .cloned()
                .collect())
        })
    }

    fn find_all_with_page(
        &self,
        query: &SysLogPageQuery,
    ) -> DynFuture<SeaOrmResult<(Vec<SysLogVO>, i64)>> {
        let logs = self.logs.clone();
        let query = query.clone();
        Box::pin(async move {
            let mut vec: Vec<SysLogVO> = logs
                .lock()
                .unwrap()
                .values()
                .filter(|l| l.is_deleted == 0)
                .filter(|l| {
                    if let Some(ref v) = query.tenant {
                        if !l.tenant.as_ref().map_or(false, |t| t.contains(v.as_str())) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.type_ {
                        if !l.type_.as_ref().map_or(false, |t| t.contains(v.as_str())) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.sub_type {
                        if !l.sub_type.as_ref().map_or(false, |t| t.contains(v.as_str())) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.biz_no {
                        if !l.biz_no.as_ref().map_or(false, |t| t.contains(v.as_str())) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.operator {
                        if !l.operator.as_ref().map_or(false, |t| t.contains(v.as_str())) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.action {
                        if !l.action.as_ref().map_or(false, |t| t.contains(v.as_str())) {
                            return false;
                        }
                    }
                    if let Some(ref v) = query.ip {
                        if !l.ip.as_ref().map_or(false, |t| t.contains(v.as_str())) {
                            return false;
                        }
                    }
                    true
                })
                .map(|l| SysLogVO {
                    id: l.id,
                    tenant: l.tenant.clone(),
                    type_: l.type_.clone(),
                    sub_type: l.sub_type.clone(),
                    biz_no: l.biz_no.clone(),
                    operator: l.operator.clone(),
                    action: l.action.clone(),
                    fail: l.fail,
                    create_time: l.create_time.map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string()),
                    extra: l.extra.clone(),
                    code_variable: l.code_variable.clone(),
                    ip: l.ip.clone(),
                    is_deleted: l.is_deleted,
                })
                .collect();
            vec.sort_by(|a, b| b.id.cmp(&a.id));
            let total = vec.len() as i64;
            let offset = (query.page() - 1) * query.size();
            let records: Vec<SysLogVO> = vec
                .iter()
                .skip(offset as usize)
                .take(query.size() as usize)
                .cloned()
                .collect();
            Ok((records, total))
        })
    }

    fn update(&self, id: &i64, req: &UpdateSysLogRequest) -> DynFuture<SeaOrmOptResult<SysLog>> {
        let logs = self.logs.clone();
        let id = *id;
        let tenant = req.tenant.clone();
        let type_ = req.type_.clone();
        let sub_type = req.sub_type.clone();
        let biz_no = req.biz_no.clone();
        let operator = req.operator.clone();
        let action = req.action.clone();
        let fail = req.fail;
        let extra = req.extra.clone();
        let code_variable = req.code_variable.clone();
        let ip = req.ip.clone();
        Box::pin(async move {
            let mut logs_lock = logs.lock().unwrap();
            if logs_lock.get(&id).map_or(true, |l| l.is_deleted != 0) {
                return Ok(None);
            }
            if let Some(log) = logs_lock.get_mut(&id) {
                if let Some(v) = tenant {
                    log.tenant = Some(v);
                }
                if let Some(v) = type_ {
                    log.type_ = Some(v);
                }
                if let Some(v) = sub_type {
                    log.sub_type = Some(v);
                }
                if let Some(v) = biz_no {
                    log.biz_no = Some(v);
                }
                if let Some(v) = operator {
                    log.operator = Some(v);
                }
                if let Some(v) = action {
                    log.action = Some(v);
                }
                if let Some(v) = fail {
                    log.fail = v;
                }
                if let Some(v) = extra {
                    log.extra = Some(v);
                }
                if let Some(v) = code_variable {
                    log.code_variable = Some(v);
                }
                if let Some(v) = ip {
                    log.ip = Some(v);
                }
                Ok(Some(log.clone()))
            } else {
                Ok(None)
            }
        })
    }

    fn delete(&self, id: &i64) -> DynFuture<SeaOrmResult<bool>> {
        let logs = self.logs.clone();
        let id = *id;
        Box::pin(async move {
            let mut logs_lock = logs.lock().unwrap();
            if let Some(log) = logs_lock.get_mut(&id) {
                log.is_deleted = 1;
                Ok(true)
            } else {
                Ok(false)
            }
        })
    }
}

// ==================== SysLog Service Tests ====================

#[tokio::test]
async fn test_create_log_success() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo);

    let req = CreateSysLogRequest {
        tenant: Some("default".to_string()),
        type_: Some("login".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("admin".to_string()),
        action: Some("用户登录".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: Some("127.0.0.1".to_string()),
    };

    let result = service.create_log(req).await;
    assert!(result.is_ok());
    let log = result.unwrap();
    assert_eq!(log.type_, Some("login".to_string()));
    assert_eq!(log.operator, Some("admin".to_string()));
    assert_eq!(log.fail, false);
    assert_eq!(log.is_deleted, 0);
}

#[tokio::test]
async fn test_get_log_success() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo.clone());

    let req = CreateSysLogRequest {
        tenant: Some("default".to_string()),
        type_: Some("operation".to_string()),
        sub_type: Some("create".to_string()),
        biz_no: Some("ORD-001".to_string()),
        operator: Some("admin".to_string()),
        action: Some("创建订单".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: Some("192.168.1.1".to_string()),
    };
    repo.create(&req, &1i64).await.unwrap();

    let result = service.get_log(&1i64).await.unwrap();
    assert_eq!(result.type_, Some("operation".to_string()));
    assert_eq!(result.sub_type, Some("create".to_string()));
    assert_eq!(result.biz_no, Some("ORD-001".to_string()));
}

#[tokio::test]
async fn test_get_log_not_found() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo);

    let result = service.get_log(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_all_logs() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo.clone());

    let req1 = CreateSysLogRequest {
        tenant: None,
        type_: Some("login".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("user1".to_string()),
        action: Some("登录".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: None,
    };
    repo.create(&req1, &1i64).await.unwrap();

    let req2 = CreateSysLogRequest {
        tenant: None,
        type_: Some("logout".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("user1".to_string()),
        action: Some("登出".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: None,
    };
    repo.create(&req2, &2i64).await.unwrap();

    let result = service.get_all_logs().await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_update_log_success() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo.clone());

    let req = CreateSysLogRequest {
        tenant: Some("default".to_string()),
        type_: Some("login".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("admin".to_string()),
        action: Some("用户登录".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: Some("127.0.0.1".to_string()),
    };
    repo.create(&req, &1i64).await.unwrap();

    let update_req = UpdateSysLogRequest {
        tenant: None,
        type_: Some("error".to_string()),
        sub_type: None,
        biz_no: None,
        operator: None,
        action: Some("登录失败".to_string()),
        fail: Some(true),
        extra: None,
        code_variable: None,
        ip: None,
    };
    let result = service.update_log(&1i64, update_req).await.unwrap();
    assert_eq!(result.type_, Some("error".to_string()));
    assert_eq!(result.action, Some("登录失败".to_string()));
    assert_eq!(result.fail, true);
    // Unchanged fields remain the same
    assert_eq!(result.operator, Some("admin".to_string()));
    assert_eq!(result.ip, Some("127.0.0.1".to_string()));
}

#[tokio::test]
async fn test_update_log_not_found() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo);

    let req = UpdateSysLogRequest {
        tenant: None,
        type_: Some("updated".to_string()),
        sub_type: None,
        biz_no: None,
        operator: None,
        action: None,
        fail: None,
        extra: None,
        code_variable: None,
        ip: None,
    };

    let result = service.update_log(&999i64, req).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_log_success() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo.clone());

    let req = CreateSysLogRequest {
        tenant: None,
        type_: Some("login".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("admin".to_string()),
        action: Some("登录".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: None,
    };
    repo.create(&req, &1i64).await.unwrap();

    let result = service.delete_log(&1i64).await;
    assert!(result.is_ok());

    let find_result = service.get_log(&1i64).await;
    assert!(matches!(find_result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_delete_log_not_found() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo);

    let result = service.delete_log(&999i64).await;
    assert!(matches!(result, Err(AppError::NotFound(_))));
}

#[tokio::test]
async fn test_get_logs_page_default() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo.clone());

    for i in 1..=15 {
        let req = CreateSysLogRequest {
            tenant: Some(format!("tenant{}", i % 3)),
            type_: Some(format!("type{}", i % 4)),
            sub_type: None,
            biz_no: None,
            operator: Some(format!("operator{}", i)),
            action: Some(format!("action{}", i)),
            fail: Some(i % 5 == 0),
            extra: None,
            code_variable: None,
            ip: Some(format!("10.0.0.{}", i)),
        };
        repo.create(&req, &(i as i64)).await.unwrap();
    }

    let result = service
        .get_logs_page(SysLogPageQuery {
            current: 1,
            size: 10,
            tenant: None,
            type_: None,
            sub_type: None,
            biz_no: None,
            operator: None,
            action: None,
            ip: None,
            begin_time: None,
            end_time: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 10);
    assert_eq!(result.total, 15);
    assert_eq!(result.current, 1);
    assert_eq!(result.size, 10);
}

#[tokio::test]
async fn test_get_logs_page_empty() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo);

    let result = service
        .get_logs_page(SysLogPageQuery {
            current: 1,
            size: 10,
            tenant: None,
            type_: None,
            sub_type: None,
            biz_no: None,
            operator: None,
            action: None,
            ip: None,
            begin_time: None,
            end_time: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 0);
    assert_eq!(result.total, 0);
}

#[tokio::test]
async fn test_get_logs_page_with_type_filter() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo.clone());

    let req1 = CreateSysLogRequest {
        tenant: None,
        type_: Some("login".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("admin".to_string()),
        action: Some("登录".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: None,
    };
    repo.create(&req1, &1i64).await.unwrap();

    let req2 = CreateSysLogRequest {
        tenant: None,
        type_: Some("error".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("admin".to_string()),
        action: Some("系统错误".to_string()),
        fail: Some(true),
        extra: None,
        code_variable: None,
        ip: None,
    };
    repo.create(&req2, &2i64).await.unwrap();

    let req3 = CreateSysLogRequest {
        tenant: None,
        type_: Some("login".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("user1".to_string()),
        action: Some("登录".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: None,
    };
    repo.create(&req3, &3i64).await.unwrap();

    let result = service
        .get_logs_page(SysLogPageQuery {
            current: 1,
            size: 10,
            tenant: None,
            type_: Some("login".to_string()),
            sub_type: None,
            biz_no: None,
            operator: None,
            action: None,
            ip: None,
            begin_time: None,
            end_time: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 2);
    assert_eq!(result.total, 2);
}

#[tokio::test]
async fn test_get_logs_page_with_operator_filter() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo.clone());

    let req1 = CreateSysLogRequest {
        tenant: None,
        type_: Some("login".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("admin".to_string()),
        action: Some("登录".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: None,
    };
    repo.create(&req1, &1i64).await.unwrap();

    let req2 = CreateSysLogRequest {
        tenant: None,
        type_: Some("create".to_string()),
        sub_type: None,
        biz_no: None,
        operator: Some("user1".to_string()),
        action: Some("创建".to_string()),
        fail: Some(false),
        extra: None,
        code_variable: None,
        ip: None,
    };
    repo.create(&req2, &2i64).await.unwrap();

    let result = service
        .get_logs_page(SysLogPageQuery {
            current: 1,
            size: 10,
            tenant: None,
            type_: None,
            sub_type: None,
            biz_no: None,
            operator: Some("admin".to_string()),
            action: None,
            ip: None,
            begin_time: None,
            end_time: None,
        })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.total, 1);
    assert_eq!(result.records[0].operator, Some("admin".to_string()));
}

#[tokio::test]
async fn test_create_log_with_fail_status() {
    let repo = Arc::new(FakeSysLogRepository::new());
    let service = SysLogService::new(repo);

    let req = CreateSysLogRequest {
        tenant: Some("system".to_string()),
        type_: Some("error".to_string()),
        sub_type: Some("runtime".to_string()),
        biz_no: None,
        operator: Some("system".to_string()),
        action: Some("数据库连接失败".to_string()),
        fail: Some(true),
        extra: Some("connection timeout".to_string()),
        code_variable: Some("DB_CONN".to_string()),
        ip: Some("10.0.0.1".to_string()),
    };

    let result = service.create_log(req).await.unwrap();
    assert_eq!(result.fail, true);
    assert_eq!(result.type_, Some("error".to_string()));
    assert_eq!(result.sub_type, Some("runtime".to_string()));
    assert_eq!(result.extra, Some("connection timeout".to_string()));
    assert_eq!(result.code_variable, Some("DB_CONN".to_string()));
}
