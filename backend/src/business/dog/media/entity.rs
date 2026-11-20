use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "dog_media")]
pub struct Model {
    #[sea_orm(primary_key, column_name = "id")]
    pub id: i64,
    #[sea_orm(column_name = "media_name", column_type = "Text")]
    pub media_name: String,
    #[sea_orm(column_name = "media_type", column_type = "Text", nullable)]
    pub media_type: Option<String>,
    #[sea_orm(column_name = "media_path", column_type = "Text", nullable)]
    pub media_path: Option<String>,
    #[sea_orm(column_name = "media_size", column_type = "Integer", nullable)]
    pub media_size: Option<i64>,
    #[sea_orm(column_name = "remarks", column_type = "Text", nullable)]
    pub remarks: Option<String>,
    // ---- 来源信息（迁移 31）：手工上传的历史行为 NULL ----
    #[sea_orm(column_name = "device_code", column_type = "Text", nullable)]
    pub device_code: Option<String>,
    #[sea_orm(column_name = "park_code", column_type = "Text", nullable)]
    pub park_code: Option<String>,
    /// 巡检计划（板端 `detect_jobs`/`media-data` 的一级目录名，= 业务任务号）
    #[sea_orm(column_name = "plan_id", column_type = "Text", nullable)]
    pub plan_id: Option<String>,
    /// 航点下标（0 基，与 `task_progress.waypointIndex` 一致）
    #[sea_orm(column_name = "point_index", column_type = "Integer", nullable)]
    pub point_index: Option<i32>,
    /// 采集时间（板端文件时间，北京时区字符串）
    #[sea_orm(column_name = "captured_at", column_type = "Text", nullable)]
    pub captured_at: Option<String>,
    /// 来源标记：`ai-patrol`（航点 AI 存证）/ `manual`（手工上传）
    #[sea_orm(column_name = "source", column_type = "Text", nullable)]
    pub source: Option<String>,
    /// 原图字节的 SHA-256（十六进制），重复回传按它去重
    #[sea_orm(column_name = "sha256", column_type = "Text", nullable)]
    pub sha256: Option<String>,
    #[sea_orm(column_name = "create_time", column_type = "Timestamp")]
    pub create_time: DateTime<Utc>,
    #[sea_orm(column_name = "update_time", column_type = "Timestamp", nullable)]
    pub update_time: Option<DateTime<Utc>>,
    #[sea_orm(column_name = "is_deleted", column_type = "Integer")]
    pub is_deleted: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
