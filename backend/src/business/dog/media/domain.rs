use crate::business::dog::media::entity::ActiveModel as MediaActiveModel;
use crate::business::dog::media::entity::Model as MediaModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogMedia = MediaModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateMediaRequest {
    pub media_name: String,
    pub media_type: Option<String>,
    pub media_path: Option<String>,
    pub media_size: Option<i64>,
    pub remarks: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMediaRequest {
    pub media_name: Option<String>,
    pub media_type: Option<String>,
    pub media_path: Option<String>,
    pub media_size: Option<i64>,
    pub remarks: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MediaPageQuery {
    pub media_name: Option<String>,
    pub media_type: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MediaVO {
    pub id: i64,
    pub media_name: String,
    pub media_type: Option<String>,
    pub media_path: Option<String>,
    pub media_size: Option<i64>,
    pub remarks: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogMedia> for MediaVO {
    fn from(m: DogMedia) -> Self {
        Self {
            id: m.id,
            media_name: m.media_name,
            media_type: m.media_type,
            media_path: m.media_path,
            media_size: m.media_size,
            remarks: m.remarks,
            create_time: format_datetime(m.create_time),
            update_time: m.update_time.map(format_datetime),
        }
    }
}

impl CreateMediaRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> MediaActiveModel {
        MediaActiveModel {
            id: ActiveValue::set(id),
            media_name: ActiveValue::set(self.media_name.clone()),
            media_type: ActiveValue::set(self.media_type.clone()),
            media_path: ActiveValue::set(self.media_path.clone()),
            media_size: ActiveValue::set(self.media_size),
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
