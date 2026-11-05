use crate::business::dog::config::entity::ActiveModel as ConfigActiveModel;
use crate::business::dog::config::entity::Model as ConfigModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogConfig = ConfigModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateConfigRequest {
    pub config_name: String,
    pub config_value: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConfigRequest {
    pub config_name: Option<String>,
    pub config_value: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPageQuery {
    pub config_name: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConfigVO {
    pub id: i64,
    pub config_name: String,
    pub config_value: Option<String>,
    pub description: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogConfig> for ConfigVO {
    fn from(c: DogConfig) -> Self {
        Self {
            id: c.id,
            config_name: c.config_name,
            config_value: c.config_value,
            description: c.description,
            create_time: format_datetime(c.create_time),
            update_time: c.update_time.map(format_datetime),
        }
    }
}

impl CreateConfigRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> ConfigActiveModel {
        ConfigActiveModel {
            id: ActiveValue::set(id),
            config_name: ActiveValue::set(self.config_name.clone()),
            config_value: ActiveValue::set(self.config_value.clone()),
            description: ActiveValue::set(self.description.clone()),
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
