use crate::business::dog::workflow::command::build_command_json;
use crate::business::dog::workflow::domain::{
    CreateWorkflowRequest, DogWorkflow, UpdateWorkflowRequest,
};
use crate::business::dog::workflow::repository::SeaOrmDogWorkflowRepository;
use crate::common::error::AppError;

pub struct DogWorkflowService {
    repo: SeaOrmDogWorkflowRepository,
}

impl DogWorkflowService {
    pub fn new(repo: SeaOrmDogWorkflowRepository) -> Self {
        Self { repo }
    }

    pub async fn create(&self, req: CreateWorkflowRequest) -> Result<DogWorkflow, AppError> {
        let id = generate_id();
        let task_id = req.task_id;
        let mut req = req;
        // 保存流程时同步生成下发给设备的指令 JSON（与 mgmt_platform 一致）
        if req.command_json_string.is_none() {
            let conn = self.repo.connection();
            req.command_json_string =
                build_command_json(conn.as_ref(), req.nodes.as_deref(), req.edges.as_deref())
                    .await?;
        }
        let workflow = self
            .repo
            .create(&req, id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if let Some(task_id) = task_id {
            self.repo
                .link_task(task_id, workflow.id, &workflow.uuid)
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;
        }
        Ok(workflow)
    }

    pub async fn get_by_id(&self, id: i64) -> Result<DogWorkflow, AppError> {
        self.repo
            .find_by_id(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Workflow with id {} not found", id)))
    }

    pub async fn get_by_uuid(&self, uuid: &str) -> Result<DogWorkflow, AppError> {
        self.repo
            .find_by_uuid(uuid)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Workflow with uuid {} not found", uuid)))
    }

    pub async fn get_by_task_id(&self, task_id: i64) -> Result<Option<DogWorkflow>, AppError> {
        let workflows = self
            .repo
            .find_by_task_id(task_id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        Ok(workflows.into_iter().next())
    }

    pub async fn update(
        &self,
        id: i64,
        req: UpdateWorkflowRequest,
    ) -> Result<DogWorkflow, AppError> {
        let mut req = req;
        let has_task_id = req.task_id;
        // 流程图变化时重新生成指令 JSON
        if req.command_json_string.is_none() && (req.nodes.is_some() || req.edges.is_some()) {
            let current = self.get_by_id(id).await?;
            let nodes = req.nodes.clone().or(current.nodes);
            let edges = req.edges.clone().or(current.edges);
            let conn = self.repo.connection();
            req.command_json_string =
                build_command_json(conn.as_ref(), nodes.as_deref(), edges.as_deref()).await?;
        }
        let workflow = self
            .repo
            .update(id, &req)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?
            .ok_or_else(|| AppError::NotFound(format!("Workflow with id {} not found", id)))?;
        if let Some(task_id) = has_task_id {
            self.repo
                .link_task(task_id, workflow.id, &workflow.uuid)
                .await
                .map_err(AppError::DatabaseErrorSeaOrm)?;
        }
        Ok(workflow)
    }

    pub async fn delete(&self, id: i64) -> Result<(), AppError> {
        let deleted = self
            .repo
            .delete(id)
            .await
            .map_err(AppError::DatabaseErrorSeaOrm)?;
        if !deleted {
            return Err(AppError::NotFound(format!(
                "Workflow with id {} not found",
                id
            )));
        }
        Ok(())
    }
}

fn generate_id() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
