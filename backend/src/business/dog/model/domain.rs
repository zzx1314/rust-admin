use crate::business::dog::model::entity::ActiveModel as ModelActiveModel;
use crate::business::dog::model::entity::Model as ModelEntity;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogModel = ModelEntity;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateModelRequest {
    pub model_name: String,
    pub model_type: Option<String>,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_format: Option<String>,
    pub file_size: Option<i64>,
    pub remarks: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModelRequest {
    pub model_name: Option<String>,
    pub model_type: Option<String>,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_format: Option<String>,
    pub file_size: Option<i64>,
    pub remarks: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelPageQuery {
    pub model_name: Option<String>,
    pub model_type: Option<String>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelVO {
    pub id: i64,
    pub model_name: String,
    pub model_type: Option<String>,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_format: Option<String>,
    pub file_size: Option<i64>,
    pub remarks: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogModel> for ModelVO {
    fn from(m: DogModel) -> Self {
        Self {
            id: m.id,
            model_name: m.model_name,
            model_type: m.model_type,
            file_name: m.file_name,
            file_path: m.file_path,
            file_format: m.file_format,
            file_size: m.file_size,
            remarks: m.remarks,
            create_time: format_datetime(m.create_time),
            update_time: m.update_time.map(format_datetime),
        }
    }
}

impl CreateModelRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> ModelActiveModel {
        ModelActiveModel {
            id: ActiveValue::set(id),
            model_name: ActiveValue::set(self.model_name.clone()),
            model_type: ActiveValue::set(self.model_type.clone()),
            file_name: ActiveValue::set(self.file_name.clone()),
            file_path: ActiveValue::set(self.file_path.clone()),
            file_format: ActiveValue::set(self.file_format.clone()),
            file_size: ActiveValue::set(self.file_size),
            remarks: ActiveValue::set(self.remarks.clone()),
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
