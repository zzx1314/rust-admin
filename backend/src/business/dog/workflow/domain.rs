use crate::business::dog::workflow::entity::ActiveModel as WorkflowActiveModel;
use crate::business::dog::workflow::entity::Model as WorkflowModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogWorkflow = WorkflowModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkflowRequest {
    pub uuid: String,
    pub nodes: Option<String>,
    pub edges: Option<String>,
    pub task_id: Option<i64>,
    pub command_json_string: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWorkflowRequest {
    pub nodes: Option<String>,
    pub edges: Option<String>,
    pub task_id: Option<i64>,
    pub command_json_string: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowVO {
    pub id: i64,
    pub uuid: String,
    pub nodes: Option<String>,
    pub edges: Option<String>,
    pub task_id: Option<i64>,
    pub command_json_string: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogWorkflow> for WorkflowVO {
    fn from(w: DogWorkflow) -> Self {
        Self {
            id: w.id,
            uuid: w.uuid,
            nodes: w.nodes,
            edges: w.edges,
            task_id: w.task_id,
            command_json_string: w.command_json_string,
            create_time: format_datetime(w.create_time),
            update_time: w.update_time.map(format_datetime),
        }
    }
}

impl CreateWorkflowRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> WorkflowActiveModel {
        WorkflowActiveModel {
            id: ActiveValue::set(id),
            uuid: ActiveValue::set(self.uuid.clone()),
            nodes: ActiveValue::set(self.nodes.clone()),
            edges: ActiveValue::set(self.edges.clone()),
            task_id: ActiveValue::set(self.task_id),
            command_json_string: ActiveValue::set(self.command_json_string.clone()),
            create_time: ActiveValue::set(now),
            update_time: ActiveValue::set(Some(now)),
            is_deleted: ActiveValue::set(0),
        }
    }
}

fn format_datetime(dt: DateTime<Utc>) -> String {
    let beijing = chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset");
    dt.with_timezone(&beijing)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}
