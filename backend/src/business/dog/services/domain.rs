use crate::business::dog::services::entity::ActiveModel as ServicesActiveModel;
use crate::business::dog::services::entity::Model as ServicesModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogServices = ServicesModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateServicesRequest {
    pub r#type: Option<String>,
    pub params: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateServicesRequest {
    pub r#type: Option<String>,
    pub params: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServicesQuery {
    pub r#type: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ServicesVO {
    pub id: i64,
    pub r#type: Option<String>,
    pub params: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogServices> for ServicesVO {
    fn from(s: DogServices) -> Self {
        Self {
            id: s.id,
            r#type: s.r#type,
            params: s.params,
            create_time: format_datetime(s.create_time),
            update_time: s.update_time.map(format_datetime),
        }
    }
}

impl CreateServicesRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> ServicesActiveModel {
        ServicesActiveModel {
            id: ActiveValue::set(id),
            r#type: ActiveValue::set(self.r#type.clone()),
            params: ActiveValue::set(self.params.clone()),
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
