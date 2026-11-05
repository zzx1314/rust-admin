use crate::business::dog::task::domain::{CreateTaskRequest, TaskPageQuery, UpdateTaskRequest};
use crate::business::dog::task::entity::ActiveModel as TaskActiveModel;
use crate::business::dog::task::entity::Column as TaskColumn;
use crate::business::dog::task::entity::Entity as TaskEntity;
use crate::business::dog::task::entity::Model as Task;
use crate::common::base::{BaseRepository, order_desc};
use sea_orm::{
    ActiveValue, ColumnTrait, Condition, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct SeaOrmDogTaskRepository {
    conn: Arc<DatabaseConnection>,
}

impl SeaOrmDogTaskRepository {
    pub fn new(conn: Arc<DatabaseConnection>) -> Self {
        Self { conn }
    }
}

impl BaseRepository for SeaOrmDogTaskRepository {
    fn conn(&self) -> Arc<DatabaseConnection> {
        self.conn.clone()
    }
}

impl SeaOrmDogTaskRepository {
    pub async fn create(&self, req: &CreateTaskRequest, id: i64) -> Result<Task, sea_orm::DbErr> {
        let now = chrono::Utc::now();
        let active_model = req.to_active_model(id, now);
        TaskEntity::insert(active_model).exec(&*self.conn).await?;
        TaskEntity::find_by_id(id)
            .one(&*self.conn)
            .await
            .map(|r| r.unwrap())
    }

    pub async fn find_by_id(&self, id: i64) -> Result<Option<Task>, sea_orm::DbErr> {
        TaskEntity::find()
            .filter(TaskColumn::Id.eq(id))
            .filter(TaskColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await
    }

    pub async fn find_all_with_page(
        &self,
        query: &TaskPageQuery,
    ) -> Result<(Vec<Task>, i64), sea_orm::DbErr> {
        let base_query = TaskEntity::find().filter(TaskColumn::IsDeleted.eq(0));
        let mut cond = Condition::all();
        if let Some(ref name) = query.task_name {
            cond = cond.add(TaskColumn::TaskName.contains(name.as_str()));
        }
        if let Some(ref status) = query.task_status {
            cond = cond.add(TaskColumn::TaskStatus.eq(status.as_str()));
        }
        if let Some(device_id) = query.device_id {
            cond = cond.add(TaskColumn::DeviceId.eq(device_id));
        }

        let total = base_query
            .clone()
            .filter(cond.clone())
            .count(&*self.conn)
            .await?;
        let current = query.current.max(1);
        let size = query.size.max(1);
        let offset = (current - 1) * size;

        let records = base_query
            .filter(cond)
            .order_by(TaskColumn::CreateTime, order_desc())
            .offset(Some(offset as u64))
            .limit(size as u64)
            .all(&*self.conn)
            .await?;

        Ok((records, total as i64))
    }

    pub async fn update(
        &self,
        id: i64,
        req: &UpdateTaskRequest,
    ) -> Result<Option<Task>, sea_orm::DbErr> {
        let entity = TaskEntity::find()
            .filter(TaskColumn::Id.eq(id))
            .filter(TaskColumn::IsDeleted.eq(0))
            .one(&*self.conn)
            .await?;

        let entity = match entity {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut active_model: TaskActiveModel = entity.into();
        if let Some(name) = &req.task_name {
            active_model.task_name = ActiveValue::set(name.clone());
        }
        if let Some(desc) = &req.task_description {
            active_model.task_description = ActiveValue::set(Some(desc.clone()));
        }
        if let Some(status) = &req.task_status {
            active_model.task_status = ActiveValue::set(status.clone());
        }
        if let Some(workflow_id) = req.workflow_id {
            active_model.workflow_id = ActiveValue::set(Some(workflow_id));
        }
        if let Some(uuid) = &req.workflow_uuid {
            active_model.workflow_uuid = ActiveValue::set(Some(uuid.clone()));
        }
        if let Some(device_id) = req.device_id {
            active_model.device_id = ActiveValue::set(Some(device_id));
        }
        if let Some(route_id) = req.route_id {
            active_model.route_id = ActiveValue::set(Some(route_id));
        }
        active_model.update_time = ActiveValue::set(Some(chrono::Utc::now()));

        TaskEntity::update(active_model).exec(&*self.conn).await?;
        self.find_by_id(id).await
    }

    pub async fn update_status(
        &self,
        id: i64,
        status: &str,
    ) -> Result<Option<Task>, sea_orm::DbErr> {
        let task = self.find_by_id(id).await?;
        if let Some(mut task) = task {
            task.task_status = status.to_string();
            task.update_time = Some(chrono::Utc::now());
            let mut active_model: TaskActiveModel = task.into();
            active_model.task_status = ActiveValue::Set(status.to_string());
            TaskEntity::update(active_model).exec(&*self.conn).await?;
            self.find_by_id(id).await
        } else {
            Ok(None)
        }
    }

    pub async fn find_all(&self) -> Result<Vec<Task>, sea_orm::DbErr> {
        TaskEntity::find()
            .filter(TaskColumn::IsDeleted.eq(0))
            .order_by(TaskColumn::CreateTime, order_desc())
            .all(&*self.conn)
            .await
    }

    pub async fn delete(&self, id: i64) -> Result<bool, sea_orm::DbErr> {
        let task = self.find_by_id(id).await?;
        if let Some(mut task) = task {
            task.is_deleted = 1;
            task.update_time = Some(chrono::Utc::now());
            let mut active_model: TaskActiveModel = task.into();
            active_model.is_deleted = ActiveValue::Set(1);
            TaskEntity::update(active_model).exec(&*self.conn).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
