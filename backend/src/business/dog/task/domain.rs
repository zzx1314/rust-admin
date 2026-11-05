use crate::business::dog::task::entity::ActiveModel as TaskActiveModel;
use crate::business::dog::task::entity::Model as TaskModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogTask = TaskModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub task_name: String,
    pub task_description: Option<String>,
    pub task_status: Option<String>,
    pub workflow_id: Option<i64>,
    pub workflow_uuid: Option<String>,
    pub device_id: Option<i64>,
    pub route_id: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskRequest {
    pub task_name: Option<String>,
    pub task_description: Option<String>,
    pub task_status: Option<String>,
    pub workflow_id: Option<i64>,
    pub workflow_uuid: Option<String>,
    pub device_id: Option<i64>,
    pub route_id: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskPageQuery {
    pub task_name: Option<String>,
    pub task_status: Option<String>,
    pub device_id: Option<i64>,
    pub user_id: Option<i64>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TaskVO {
    pub id: i64,
    pub task_name: String,
    pub task_description: Option<String>,
    pub task_status: String,
    pub workflow_id: Option<i64>,
    pub workflow_uuid: Option<String>,
    pub device_id: Option<i64>,
    pub route_id: Option<i64>,
    pub user_id: Option<i64>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogTask> for TaskVO {
    fn from(t: DogTask) -> Self {
        Self {
            id: t.id,
            task_name: t.task_name,
            task_description: t.task_description,
            task_status: t.task_status,
            workflow_id: t.workflow_id,
            workflow_uuid: t.workflow_uuid,
            device_id: t.device_id,
            route_id: t.route_id,
            user_id: t.user_id,
            create_time: format_datetime(t.create_time),
            update_time: t.update_time.map(format_datetime),
        }
    }
}

impl CreateTaskRequest {
    pub fn to_active_model(
        &self,
        id: i64,
        now: DateTime<Utc>,
        user_id: Option<i64>,
    ) -> TaskActiveModel {
        TaskActiveModel {
            id: ActiveValue::set(id),
            task_name: ActiveValue::set(self.task_name.clone()),
            task_description: ActiveValue::set(self.task_description.clone()),
            task_status: ActiveValue::set(
                self.task_status
                    .clone()
                    .unwrap_or_else(|| "pending".to_string()),
            ),
            workflow_id: ActiveValue::set(self.workflow_id),
            workflow_uuid: ActiveValue::set(self.workflow_uuid.clone()),
            device_id: ActiveValue::set(self.device_id),
            route_id: ActiveValue::set(self.route_id),
            user_id: ActiveValue::set(user_id),
            create_time: ActiveValue::set(now),
            update_time: ActiveValue::set(Some(now)),
            is_deleted: ActiveValue::set(0),
        }
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}

fn format_datetime(dt: DateTime<Utc>) -> String {
    let beijing = chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset");
    dt.with_timezone(&beijing)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}
