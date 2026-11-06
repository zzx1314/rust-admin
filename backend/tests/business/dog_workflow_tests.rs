//! Workflow persistence tests for the dog business module.
//!
//! 流程设计保存后需要能再次回显：
//! - 按任务能查回已保存的流程（`GET /api/dogWorkflow/byTask/{taskId}` 的底层实现）
//! - 保存时会把 workflow id / uuid 回写到 `dog_task`，任务执行与列表回显依赖该字段

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use x_rust::api::AppState;
use x_rust::app::App;
use x_rust::business::dog::device::domain::CreateDeviceRequest;
use x_rust::business::dog::route_library::domain::CreateRouteRequest;
use x_rust::business::dog::services::domain::CreateServicesRequest;
use x_rust::business::dog::task::domain::CreateTaskRequest;
use x_rust::business::dog::workflow::domain::{CreateWorkflowRequest, UpdateWorkflowRequest};
use x_rust::common::data_scope::DataScope;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};

const USER_ID: i64 = 101;
const JWT_SECRET: &str = "test-secret";

struct FakeTokenStore {
    store: Arc<Mutex<HashMap<String, String>>>,
}

impl FakeTokenStore {
    fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl TokenStore for FakeTokenStore {
    fn set_token(
        &self,
        user_id: &str,
        token: &str,
        _ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        let token = token.to_string();
        Box::pin(async move {
            store.lock().unwrap().insert(key, token);
            Ok(())
        })
    }

    fn get_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        Box::pin(async move { Ok(store.lock().unwrap().get(&key).cloned()) })
    }

    fn delete_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        Box::pin(async move {
            store.lock().unwrap().remove(&key);
            Ok(())
        })
    }

    fn set_refresh_token(
        &self,
        user_id: &str,
        token: &str,
        _ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        let token = token.to_string();
        Box::pin(async move {
            store.lock().unwrap().insert(key, token);
            Ok(())
        })
    }

    fn get_refresh_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        Box::pin(async move { Ok(store.lock().unwrap().get(&key).cloned()) })
    }

    fn delete_refresh_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        Box::pin(async move {
            store.lock().unwrap().remove(&key);
            Ok(())
        })
    }
}

async fn test_state() -> AppState {
    let app = App::new("sqlite::memory:")
        .await
        .expect("in-memory database must migrate");
    app.build_state_with_token_store(Arc::new(FakeTokenStore::new()), JWT_SECRET)
}

fn workflow_request(task_id: i64, uuid: &str, nodes: &str) -> CreateWorkflowRequest {
    CreateWorkflowRequest {
        uuid: uuid.to_string(),
        nodes: Some(nodes.to_string()),
        edges: Some("[]".to_string()),
        task_id: Some(task_id),
        command_json_string: None,
    }
}

#[tokio::test]
async fn workflow_is_restorable_after_save() {
    let state = test_state().await;
    let scope = DataScope::new(USER_ID, false);

    let task = state
        .dog_task_service
        .create(
            CreateTaskRequest {
                task_name: "巡检任务".to_string(),
                task_description: None,
                task_status: None,
                workflow_id: None,
                workflow_uuid: None,
                device_id: None,
                route_id: None,
            },
            &scope,
        )
        .await
        .expect("task must be created");

    let uuid = format!("task_{}", task.id);
    let workflow = state
        .dog_workflow_service
        .create(workflow_request(task.id, &uuid, r#"[{"id":"node_1"}]"#))
        .await
        .expect("workflow must be created");

    // 按 taskId 能查回流程（再次进入流程设计页面的回显依据）
    let restored = state
        .dog_workflow_service
        .get_by_task_id(task.id)
        .await
        .expect("lookup by task must succeed")
        .expect("workflow must exist for the task");
    assert_eq!(restored.id, workflow.id);
    assert_eq!(restored.nodes.as_deref(), Some(r#"[{"id":"node_1"}]"#));

    // 保存流程时回写任务上的 workflow 关联
    let task_after = state
        .dog_task_service
        .get_by_id(task.id, &scope)
        .await
        .expect("task must be readable");
    assert_eq!(task_after.workflow_id, Some(workflow.id));
    assert_eq!(task_after.workflow_uuid.as_deref(), Some(uuid.as_str()));

    // 覆盖保存后仍是同一份流程，节点数据更新
    let updated = state
        .dog_workflow_service
        .update(
            workflow.id,
            UpdateWorkflowRequest {
                nodes: Some(r#"[{"id":"node_2"}]"#.to_string()),
                edges: Some("[]".to_string()),
                task_id: Some(task.id),
                command_json_string: None,
            },
        )
        .await
        .expect("workflow must be updated");
    assert_eq!(updated.id, workflow.id);

    let by_task = state
        .dog_workflow_service
        .get_by_task_id(task.id)
        .await
        .expect("lookup by task must succeed")
        .expect("workflow must still exist");
    assert_eq!(by_task.id, workflow.id);
    assert_eq!(by_task.nodes.as_deref(), Some(r#"[{"id":"node_2"}]"#));
}

/// 保存流程时生成的指令 JSON 需要与 mgmt_platform 的 payload 结构一致：
/// type / deviceId / route / tasks(actions) / services / onErrorActions
#[tokio::test]
async fn workflow_command_json_matches_flow_design() {
    let state = test_state().await;

    let device = state
        .dog_device_service
        .create(CreateDeviceRequest {
            device_id: "DOG-001".to_string(),
            device_type: None,
            status: None,
            device_ip: None,
            model: None,
            speed: None,
            height: None,
            battery: None,
            course: None,
            location: None,
        })
        .await
        .expect("device must be created");

    let route = state
        .dog_route_library_service
        .create(CreateRouteRequest {
            route_name: "巡检路线".to_string(),
            route_type: None,
            route_status: None,
            route_description: None,
            park_id: None,
            model_id: 1,
            route_data: Some("[[0,0],[10,5]]".to_string()),
        })
        .await
        .expect("route must be created");

    state
        .dog_services_service
        .create(CreateServicesRequest {
            r#type: Some("YOLO".to_string()),
            params: Some(r#"{"model":"yolov8","event":{"onStart":"yolo_start"}}"#.to_string()),
        })
        .await
        .expect("service must be created");

    let nodes = json!([
        { "id": "node_start", "type": "start", "data": {} },
        {
            "id": "node_task",
            "type": "task",
            "data": {
                "taskId": "node_task",
                "deviceId": device.id,
                "routeId": route.id,
                "taskInfo": {
                    "fromWp": { "wpId": 1 },
                    "toWp": { "wpId": 2 },
                    "event": "init_a"
                }
            }
        },
        {
            "id": "node_action",
            "type": "action",
            "data": {
                "nodeType": "action",
                "action": {
                    "type": "TAKEOFF",
                    "timeoutSec": 10,
                    "params": {
                        "targetAlt": 3,
                        "event": { "onComplete": "takeoff_done", "onFail": "takeoff_fail" }
                    }
                }
            }
        },
        {
            "id": "node_action2",
            "type": "action",
            "data": {
                "nodeType": "action",
                "action": {
                    "type": "SERVICE_START_YOLO",
                    "timeoutSec": 5,
                    "params": {
                        "type": "YOLO",
                        "event": { "onComplete": "yolo_done", "onFail": "yolo_fail" }
                    }
                }
            }
        },
        {
            "id": "node_error",
            "type": "error",
            "data": {
                "nodeType": "action",
                "action": {
                    "type": "",
                    "params": { "reason": "超时", "event": { "onComplete": "" } }
                }
            }
        },
        { "id": "node_end", "type": "end", "data": {} }
    ]);
    let edges = json!([
        { "source": "node_start", "target": "node_task" },
        { "source": "node_task", "target": "node_action" },
        { "source": "node_action", "target": "node_action2" },
        { "source": "node_action2", "target": "node_error" },
        { "source": "node_action2", "target": "node_end" }
    ]);

    let workflow = state
        .dog_workflow_service
        .create(CreateWorkflowRequest {
            uuid: "task_flow".to_string(),
            nodes: Some(nodes.to_string()),
            edges: Some(edges.to_string()),
            task_id: None,
            command_json_string: None,
        })
        .await
        .expect("workflow must be created");

    let command: Value =
        serde_json::from_str(workflow.command_json_string.as_deref().unwrap_or_default())
            .expect("command json must be valid");

    assert_eq!(command["type"], "toros");
    assert_eq!(command["deviceId"], "DOG-001");

    let route_points = command["route"].as_array().expect("route must be an array");
    assert_eq!(route_points.len(), 2);
    assert_eq!(route_points[1]["wpId"], 2);
    assert_eq!(route_points[1]["lat"], 5.0);
    assert_eq!(route_points[1]["lon"], 10.0);

    let tasks = command["tasks"].as_array().expect("tasks must be an array");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["taskId"], "node_task");
    assert_eq!(tasks[0]["event"], "init_a");
    assert_eq!(tasks[0]["fromWp"]["wpId"], 1);
    assert_eq!(tasks[0]["toWp"]["wpId"], 2);

    let actions = tasks[0]["actions"].as_array().expect("actions must exist");
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0]["after"], "init_a");
    assert_eq!(actions[1]["after"], "takeoff_done");
    // 服务参数会合并进动作
    assert_eq!(actions[1]["params"]["model"], "yolov8");
    assert_eq!(actions[1]["params"]["event"]["onStart"], "yolo_start");

    let services = command["services"].as_array().expect("services must exist");
    assert_eq!(services.len(), 1);
    assert_eq!(services[0]["type"], "YOLO");

    let error_actions = command["onErrorActions"]
        .as_array()
        .expect("onErrorActions must exist");
    assert_eq!(error_actions.len(), 1);
    assert_eq!(error_actions[0]["after"], "yolo_fail");
    assert_eq!(error_actions[0]["params"]["reason"], "超时");
}
