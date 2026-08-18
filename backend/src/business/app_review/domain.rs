use crate::business::app_review::entity::ActiveModel as ReviewActiveModel;
use crate::business::app_review::entity::Model as ReviewModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub type Review = ReviewModel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReviewStatus {
    Pending,
    Approved,
    Rejected,
}

impl ReviewStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewStatus::Pending => "pending",
            ReviewStatus::Approved => "approved",
            ReviewStatus::Rejected => "rejected",
        }
    }
}

impl FromStr for ReviewStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(ReviewStatus::Pending),
            "approved" => Ok(ReviewStatus::Approved),
            "rejected" => Ok(ReviewStatus::Rejected),
            _ => Err(format!("Unknown review status: {}", s)),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateReviewRequest {
    pub src_project: String,
    pub dest_project: String,
    pub repository_name: String,
    pub tag: String,
    pub digest: Option<String>,
    pub artifact_id: Option<i64>,
    pub reviewer_comment: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReviewActionRequest {
    pub reviewer_comment: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPageQuery {
    pub src_project: Option<String>,
    pub repository_name: Option<String>,
    pub status: Option<String>,
    pub current: i64,
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReviewVO {
    pub id: i64,
    pub src_project: String,
    pub dest_project: String,
    pub repository_name: String,
    pub tag: String,
    pub digest: Option<String>,
    pub artifact_id: Option<i64>,
    pub status: String,
    pub reviewer_comment: Option<String>,
    pub created_by: Option<i64>,
    pub reviewer_id: Option<i64>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<Review> for ReviewVO {
    fn from(review: Review) -> Self {
        Self {
            id: review.id,
            src_project: review.src_project.clone(),
            dest_project: review.dest_project.clone(),
            repository_name: review.repository_name.clone(),
            tag: review.tag.clone(),
            digest: review.digest.clone(),
            artifact_id: review.artifact_id,
            status: review.status.clone(),
            reviewer_comment: review.reviewer_comment.clone(),
            created_by: review.created_by,
            reviewer_id: review.reviewer_id,
            create_time: format_datetime(review.create_time),
            update_time: review.update_time.map(format_datetime),
        }
    }
}

impl CreateReviewRequest {
    pub fn to_active_model(&self, id: i64, created_by: Option<i64>, now: DateTime<Utc>) -> ReviewActiveModel {
        ReviewActiveModel {
            id: ActiveValue::set(id),
            src_project: ActiveValue::set(self.src_project.clone()),
            dest_project: ActiveValue::set(self.dest_project.clone()),
            repository_name: ActiveValue::set(self.repository_name.clone()),
            tag: ActiveValue::set(self.tag.clone()),
            digest: ActiveValue::set(self.digest.clone()),
            artifact_id: ActiveValue::set(self.artifact_id),
            status: ActiveValue::set(ReviewStatus::Pending.as_str().to_string()),
            reviewer_comment: ActiveValue::set(self.reviewer_comment.clone()),
            created_by: ActiveValue::set(created_by),
            reviewer_id: ActiveValue::set(None),
            create_time: ActiveValue::set(now),
            update_time: ActiveValue::set(Some(now)),
            is_deleted: ActiveValue::set(0),
        }
    }
}

fn format_datetime(dt: DateTime<Utc>) -> String {
    let beijing = chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset");
    dt.with_timezone(&beijing).format("%Y-%m-%d %H:%M:%S").to_string()
}
