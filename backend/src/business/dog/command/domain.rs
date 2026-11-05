use crate::business::dog::command::entity::ActiveModel as CommandActiveModel;
use crate::business::dog::command::entity::Model as CommandModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogCommand = CommandModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateCommandRequest {
    pub command_name: String,
    pub command_type: Option<String>,
    pub command_params: Option<String>,
    pub device_id: Option<String>,
    pub status: Option<String>,
    pub return_value: Option<String>,
    pub task_id: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCommandRequest {
    pub command_name: Option<String>,
    pub command_type: Option<String>,
    pub command_params: Option<String>,
    pub device_id: Option<String>,
    pub status: Option<String>,
    pub return_value: Option<String>,
    pub task_id: Option<i64>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommandPageQuery {
    pub command_name: Option<String>,
    pub command_type: Option<String>,
    pub status: Option<String>,
    pub device_id: Option<String>,
    pub task_id: Option<i64>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommandVO {
    pub id: i64,
    pub command_name: String,
    pub command_type: Option<String>,
    pub command_params: Option<String>,
    pub device_id: Option<String>,
    pub status: String,
    pub return_value: Option<String>,
    pub task_id: Option<i64>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogCommand> for CommandVO {
    fn from(c: DogCommand) -> Self {
        Self {
            id: c.id,
            command_name: c.command_name,
            command_type: c.command_type,
            command_params: c.command_params,
            device_id: c.device_id,
            status: c.status,
            return_value: c.return_value,
            task_id: c.task_id,
            create_time: format_datetime(c.create_time),
            update_time: c.update_time.map(format_datetime),
        }
    }
}

impl CreateCommandRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> CommandActiveModel {
        CommandActiveModel {
            id: ActiveValue::set(id),
            command_name: ActiveValue::set(self.command_name.clone()),
            command_type: ActiveValue::set(self.command_type.clone()),
            command_params: ActiveValue::set(self.command_params.clone()),
            device_id: ActiveValue::set(self.device_id.clone()),
            status: ActiveValue::set(self.status.clone().unwrap_or_else(|| "pending".to_string())),
            return_value: ActiveValue::set(self.return_value.clone()),
            task_id: ActiveValue::set(self.task_id),
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
