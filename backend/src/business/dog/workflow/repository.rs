use crate::business::dog::workflow::domain::{CreateWorkflowRequest, UpdateWorkflowRequest};
use crate::business::dog::workflow::entity::ActiveModel as WorkflowActiveModel;
use crate::business::dog::workflow::entity::Column as WorkflowColumn;
use crate::business::dog::workflow::entity::Entity as WorkflowEntity;
use crate::business::dog::workflow::entity::Model as DogWorkflow;
use crate::common::base::{BaseRepository, order_desc};
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogWorkflowRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogWorkflowRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }

    /// 供指令 JSON 生成等跨表查询使用。
    pub fn connection(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl BaseRepository for SeaOrmDogWorkflowRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmDogWorkflowRepository {
    pub async fn create(
        &self,
        req: &CreateWorkflowRequest,
        id: i64,
    ) -> Result<DogWorkflow, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        WorkflowEntity::insert(active_model)
            .exec(&*self.conn)
            .await?;
        WorkflowEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<DogWorkflow>, sea_orm::DbErr> {
        WorkflowEntity::find()
            .filter(WorkflowColumn::Id.eq(id))
            .filter(WorkflowColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_by_uuid(&self, uuid: &str) -> Result<Option<DogWorkflow>, sea_orm::DbErr> {
        WorkflowEntity::find()
            .filter(WorkflowColumn::Uuid.eq(uuid))
            .filter(WorkflowColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_by_task_id(&self, task_id: i64) -> Result<Vec<DogWorkflow>, sea_orm::DbErr> {
        WorkflowEntity::find()
            .filter(WorkflowColumn::TaskId.eq(task_id))
            .filter(WorkflowColumn::IsDeleted.eq(0))
            .order_by(WorkflowColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    /// 保存工作流时把工作流 id / uuid 回写到所属任务，便于任务执行与再次回显。
    pub async fn link_task(
        &self,
        task_id: i64,
        workflow_id: i64,
        workflow_uuid: &str,
    ) -> Result<(), sea_orm::DbErr> {
        use crate::business::dog::task::entity::{
            ActiveModel as TaskActiveModel, Column as TaskColumn, Entity as TaskEntity,
        };

        let task = TaskEntity::find()
            .filter(TaskColumn::Id.eq(task_id))
            .filter(TaskColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;
        let Some(task) = task else {
            return Ok(());
        };

        let mut active_model: TaskActiveModel = task.into();
        active_model.workflow_id = ActiveValue::set(Some(workflow_id));
        active_model.workflow_uuid = ActiveValue::set(Some(workflow_uuid.to_string()));
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));
        TaskEntity::update(active_model).exec(&*self.conn).await?;
        Ok(())
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateWorkflowRequest,
    ) -> Result<Option<DogWorkflow>, sea_orm::DbErr> {
        let entity = WorkflowEntity::find()
            .filter(WorkflowColumn::Id.eq(id))
            .filter(WorkflowColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: WorkflowActiveModel = entity.into();
        if let Some(nodes) = &req.nodes {
            active_model.nodes = ActiveValue::set(Some(nodes.clone()));
        }
        if let Some(edges) = &req.edges {
            active_model.edges = ActiveValue::set(Some(edges.clone()));
        }
        if let Some(tid) = req.task_id {
            active_model.task_id = ActiveValue::set(Some(tid));
        }
        if let Some(cmd) = &req.command_json_string {
            active_model.command_json_string = ActiveValue::set(Some(cmd.clone()));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        WorkflowEntity::update(active_model)
            .exec(&*self.conn)
            .await?;
        self.find_by_id(id).await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let workflow = self.find_by_id(id).await?;
        if let Some(mut workflow) = workflow {
            workflow.is_deleted = 1;
            workflow.update_time = Some(chrono::Utc::now());
            let mut active_model: WorkflowActiveModel = workflow.into();
            active_model.is_deleted = ActiveValue::Set(1);
            WorkflowEntity::update(active_model)
                .exec(&*self.conn)
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
