use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::business::dog::command::domain::CreateCommandRequest;
use crate::business::dog::command::repository::SeaOrmDogCommandRepository;
use crate::business::dog::device::repository::SeaOrmDogDeviceRepository;
use crate::business::dog::gateway::DogGateway;
use crate::business::dog::map::repository::SeaOrmDogMapRepository;
use crate::business::dog::route_library::repository::SeaOrmDogRouteLibraryRepository;
use crate::business::dog::task::domain::{
    CreateTaskRequest, DogTask, TaskPageQuery, TaskStatusVO, TaskVO, UpdateTaskRequest,
};
use crate::business::dog::task::repository::SeaOrmDogTaskRepository;
use crate::business::dog::workflow::repository::SeaOrmDogWorkflowRepository;
use crate::common::data_scope::DataScope;
use crate::common::error::AppError;
use crate::common::pagination::PageResponse;
use crate::common::util::generate_id;

pub struct DogTaskService {
    repo: SeaOrmDogTaskRepository,
    device_repo: SeaOrmDogDeviceRepository,
    workflow_repo: SeaOrmDogWorkflowRepository,
    command_repo: SeaOrmDogCommandRepository,
    route_repo: SeaOrmDogRouteLibraryRepository,
    map_repo: SeaOrmDogMapRepository,
    gateway: Arc<DogGateway>,
}

impl DogTaskService {
    pub fn new(
        repo: SeaOrmDogTaskRepository,
        device_repo: SeaOrmDogDeviceRepository,
        workflow_repo: SeaOrmDogWorkflowRepository,
        command_repo: SeaOrmDogCommandRepository,
        route_repo: SeaOrmDogRouteLibraryRepository,
        map_repo: SeaOrmDogMapRepository,
        gateway: Arc<DogGateway>,
    ) -> Self {
        Self {
            repo,
            device_repo,
            workflow_repo,
            command_repo,
            route_repo,
            map_repo,
            gateway,
        }
    }

    pub async fn create(
        &self,
        req: CreateTaskRequest,
        scope: &DataScope,
    ) -> Result<DogTask, AppError> {
        if let Some(device_id) = req.device_id {
            self.ensure_device_access(device_id, scope).await?;
        }
        let id = generate_id();
        self.repo
            .create(&req, id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn get_by_id(&self, id: i64, scope: &DataScope) -> Result<DogTask, AppError> {
        self.repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Task with id {} not found", id)))
    }

    pub async fn list_page(
        &self,
        query: TaskPageQuery,
        scope: &DataScope,
    ) -> Result<PageResponse<TaskVO>, AppError> {
        let (records, total) = self
            .repo
            .find_all_with_page(&query, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        Ok(PageResponse::new(
            records,
            total,
            query.current.max(1),
            query.size.max(1),
        ))
    }

    pub async fn list_all(&self, scope: &DataScope) -> Result<Vec<DogTask>, AppError> {
        self.repo
            .find_all(scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateTaskRequest,
        scope: &DataScope,
    ) -> Result<DogTask, AppError> {
        if let Some(device_id) = req.device_id {
            self.ensure_device_access(device_id, scope).await?;
        }
        self.repo
            .update(id, &req, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Task with id {} not found", id)))
    }

    pub async fn update_status(
        &self,
        id: i64,
        status: &str,
        scope: &DataScope,
    ) -> Result<DogTask, AppError> {
        self.repo
            .update_status(id, status, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Task with id {} not found", id)))
    }

    pub async fn delete(&self, id: i64, scope: &DataScope) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!("Task with id {} not found", id)));
        }
        Ok(())
    }

    /// Start a task: find workflow, create a command with its command_json_string,
    /// dispatch via WebSocket gateway, and update task status to "进行中".
    pub async fn start_task(&self, id: i64, scope: &DataScope) -> Result<DogTask, AppError> {
        let task = self
            .repo
            .find_by_id(id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Task with id {} not found", id)))?;

        // Find workflow by workflow_uuid (or workflow_id)
        let workflow = if let Some(ref uuid) = task.workflow_uuid {
            self.workflow_repo
                .find_by_uuid(uuid)
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?
        } else if let Some(wid) = task.workflow_id {
            self.workflow_repo
                .find_by_id(wid)
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?
        } else {
            return Err(AppError::BadRequest(
                "Task has no associated workflow".to_string(),
            ));
        };

        let workflow = workflow
            .ok_or_else(|| AppError::NotFound("Associated workflow not found".to_string()))?;

        // Get the command JSON from the workflow
        let command_json = workflow.command_json_string.unwrap_or_default();
        if command_json.is_empty() {
            return Err(AppError::BadRequest(
                "Workflow command_json_string is empty, design the workflow first".to_string(),
            ));
        }

        // 任务引用路线时，下发包必须携带冻结的地图基线快照；设备执行前校验 mapHash。
        let map_payload = self.resolve_map_payload(task.route_id).await?;
        let command_json = match map_payload {
            Some(map) => {
                let mut params: Value = serde_json::from_str(&command_json).map_err(|error| {
                    AppError::BadRequest(format!(
                        "Workflow command_json_string is not valid JSON: {error}"
                    ))
                })?;
                let object = params.as_object_mut().ok_or_else(|| {
                    AppError::BadRequest(
                        "Workflow command_json_string must be a JSON object".to_string(),
                    )
                })?;
                object.insert("map".to_string(), map);
                serde_json::to_string(&params).map_err(|error| {
                    AppError::InternalError(format!("failed to encode command params: {error}"))
                })?
            }
            None => command_json,
        };

        // Resolve the device code (device_id string) from the task's device_id
        let device_code = match task.device_id {
            Some(did) => {
                let device = self
                    .device_repo
                    .find_by_id_unscoped(did)
                    .await
                    .map_err(AppError::DatabaseErrorSeaOrm)?
                    .ok_or_else(|| {
                        AppError::NotFound(format!("Device with id {} not found", did))
                    })?;
                device.device_id
            }
            None => {
                return Err(AppError::BadRequest(
                    "Task has no associated device".to_string(),
                ));
            }
        };

        // Create a command record
        let command_id = generate_id();
        let command_req = CreateCommandRequest {
            command_name: format!("task_{}", task.task_name),
            command_type: Some("mission".to_string()),
            command_params: Some(command_json.clone()),
            device_id: Some(device_code.clone()),
            status: Some("pending".to_string()),
            return_value: None,
            task_id: Some(task.id),
        };

        let command = self
            .command_repo
            .create(&command_req, command_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        // Dispatch via gateway
        match self.gateway.dispatch_command(&command).await {
            Ok(_) => {
                tracing::info!(task_id = id, "task command dispatched via gateway");
            }
            Err(error) => {
                tracing::warn!(task_id = id, %error, "failed to dispatch command via gateway");
            }
        }

        // Update task status to "进行中"
        self.repo
            .update_status_unscoped(id, "进行中")
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| {
                AppError::NotFound(format!("Task with id {} not found after update", id))
            })
    }

    /// 解析任务路线的冻结地图基线，生成设备校验快照。
    ///
    /// 路线已绑定地图时，缺少版本 / 哈希的地图基线一律拒绝下发，避免
    /// 「拿 A 版地图跑 B 版路线」。
    async fn resolve_map_payload(&self, route_id: Option<i64>) -> Result<Option<Value>, AppError> {
        let Some(route_id) = route_id else {
            return Ok(None);
        };
        let route = self
            .route_repo
            .find_by_id(route_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::BadRequest(format!("Route with id {route_id} not found")))?;
        let map_id = route
            .map_id
            .ok_or_else(|| AppError::BadRequest("路线未绑定地图基线，禁止下发任务".to_string()))?;
        let map = self
            .map_repo
            .find_any_by_id(map_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| {
                AppError::BadRequest(format!("Route {route_id} references missing map {map_id}"))
            })?;
        let payload = map.dispatch_payload().ok_or_else(|| {
            AppError::BadRequest(format!(
                "地图基线 {} 缺少 map_hash，请重新上传地图包并发布后再下发",
                map.id
            ))
        })?;
        Ok(Some(payload))
    }

    /// Get task execution status by querying the command result items.
    pub async fn get_task_status(&self, id: i64) -> Result<TaskStatusVO, AppError> {
        let mut success_ids = Vec::new();
        let mut fail_ids = Vec::new();
        let mut error_info_map = HashMap::new();

        // Find commands for this task (use raw SQL query on command repo)
        let commands = self
            .command_repo
            .find_by_task_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;

        for command in commands {
            if let Some(ref return_value) = command.return_value {
                // Try to parse the return_value as JSON
                if let Ok(json) = serde_json::from_str::<Value>(return_value) {
                    // Check for action results
                    let action_result = json.get("actionResult").or_else(|| json.get("result"));
                    if let Some(result_obj) = action_result {
                        let state = result_obj
                            .get("state")
                            .or_else(|| result_obj.get("success"))
                            .and_then(|v| v.as_bool());

                        let action_id = json
                            .get("actionId")
                            .or_else(|| json.get("commandId"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        let task_id_str = json
                            .get("taskId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        match state {
                            Some(true) => {
                                if !action_id.is_empty() {
                                    success_ids.push(action_id);
                                }
                                if !task_id_str.is_empty() {
                                    success_ids.push(task_id_str);
                                }
                            }
                            Some(false) => {
                                let msg = result_obj
                                    .get("msg")
                                    .or_else(|| result_obj.get("error"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown error")
                                    .to_string();
                                if !action_id.is_empty() {
                                    fail_ids.push(action_id.clone());
                                    error_info_map.insert(action_id, msg.clone());
                                }
                                if !task_id_str.is_empty() {
                                    fail_ids.push(task_id_str);
                                }
                            }
                            None => {
                                // Status field directly on the result
                                let status_str = result_obj.get("status").and_then(|v| v.as_str());
                                if let Some("success" | "SUCCESS" | "ok" | "OK") = status_str {
                                    let id_str = command.id.to_string();
                                    success_ids.push(id_str);
                                }
                            }
                        }
                    } else {
                        // Command-level status
                        let status = command.status.as_str();
                        if status == "success" || status == "completed" {
                            success_ids.push(command.id.to_string());
                        } else if status == "error" || status == "failed" {
                            fail_ids.push(command.id.to_string());
                            error_info_map.insert(
                                command.id.to_string(),
                                format!("Command status: {}", status),
                            );
                        }
                    }
                } else {
                    // Plain text return value
                    if command.status == "success" || command.status == "completed" {
                        success_ids.push(command.id.to_string());
                    }
                }
            } else if command.status == "success" || command.status == "completed" {
                success_ids.push(command.id.to_string());
            }
        }

        // Deduplicate: remove fail_ids from success_ids
        let fail_set: std::collections::HashSet<_> = fail_ids.iter().cloned().collect();
        success_ids.retain(|id| !fail_set.contains(id));

        Ok(TaskStatusVO {
            success_ids,
            fail_ids,
            error_info_map,
        })
    }

    async fn ensure_device_access(
        &self,
        device_id: i64,
        scope: &DataScope,
    ) -> Result<(), AppError> {
        let accessible = self
            .device_repo
            .exists_for_scope(device_id, scope)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !accessible {
            return Err(AppError::BadRequest(format!(
                "Device with id {} not found or not accessible",
                device_id
            )));
        }
        Ok(())
    }
}
