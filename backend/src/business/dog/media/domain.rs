use crate::business::dog::media::entity::ActiveModel as MediaActiveModel;
use crate::business::dog::media::entity::Model as MediaModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogMedia = MediaModel;

/// 手工「新增媒体」与设备回传共用的写入请求。
///
/// 设备回传（`/api/dogInternal/media/upload`）会带上 `deviceCode` / `planId` /
/// `pointIndex` / `capturedAt` / `sha256` 这些**来源字段**；手工新增不填，
/// 页面按「手工上传」展示（与迁移 31 之前的存量行一致）。
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CreateMediaRequest {
    pub media_name: String,
    pub media_type: Option<String>,
    pub media_path: Option<String>,
    pub media_size: Option<i64>,
    pub remarks: Option<String>,
    pub device_code: Option<String>,
    pub park_code: Option<String>,
    pub plan_id: Option<String>,
    pub point_index: Option<i32>,
    pub captured_at: Option<String>,
    pub source: Option<String>,
    pub sha256: Option<String>,
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

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaPageQuery {
    pub media_name: Option<String>,
    pub media_type: Option<String>,
    pub device_code: Option<String>,
    pub plan_id: Option<String>,
    pub source: Option<String>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    pub current: i64,
    pub size: i64,
}

impl MediaPageQuery {
    /// `default` 反序列化会把 `current/size` 落成 0，这里统一收敛成合法分页。
    pub fn normalized(mut self) -> Self {
        if self.current < 1 {
            self.current = 1;
        }
        if self.size < 1 {
            self.size = 10;
        }
        self
    }
}

/// 设备回传一张图片时的描述（业务平台 `/api/media/ingest` 与中台
/// `/api/dogInternal/media/upload` 共用同一份 JSON）。
///
/// 字段全部可选：缺什么就用文件本身推断（文件名 → 时间、扩展名 → 类型），
/// 这样板端/客户端升级不同步也不会传不上去。
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct UploadMediaRequest {
    pub device_code: Option<String>,
    pub park_code: Option<String>,
    pub plan_id: Option<String>,
    pub plan_name: Option<String>,
    pub point_index: Option<i32>,
    pub point_name: Option<String>,
    pub captured_at: Option<String>,
    pub source: Option<String>,
    pub remarks: Option<String>,
    /// 图片字节的 SHA-256（十六进制，小写）。客户端没算时服务端自己算。
    pub sha256: Option<String>,
    /// 期望的媒体名称（缺省由服务端拼「设备 + 航点 + 时间」）
    pub media_name: Option<String>,
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
    pub device_code: Option<String>,
    pub park_code: Option<String>,
    pub plan_id: Option<String>,
    pub point_index: Option<i32>,
    pub captured_at: Option<String>,
    pub source: Option<String>,
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
            device_code: m.device_code,
            park_code: m.park_code,
            plan_id: m.plan_id,
            point_index: m.point_index,
            captured_at: m.captured_at,
            source: m.source,
            create_time: format_datetime(m.create_time),
            update_time: m.update_time.map(format_datetime),
        }
    }
}

/// `GET /api/dogMedia/{id}/image` — 图片内容（data URL）、类型与大小。
///
/// 和地图预览（`/api/dogMap/{id}/image`）同一套约定：返回 JSON + base64，
/// 前端 `<img>` 直接用，不必处理二进制流与鉴权头。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MediaImageVO {
    pub id: i64,
    pub media_name: String,
    pub content_type: String,
    pub size: i64,
    pub image: String,
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
            device_code: ActiveValue::set(self.device_code.clone()),
            park_code: ActiveValue::set(self.park_code.clone()),
            plan_id: ActiveValue::set(self.plan_id.clone()),
            point_index: ActiveValue::set(self.point_index),
            captured_at: ActiveValue::set(self.captured_at.clone()),
            source: ActiveValue::set(self.source.clone()),
            sha256: ActiveValue::set(self.sha256.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_create_request_defaults_origin_fields() {
        let req: CreateMediaRequest =
            serde_json::from_str(r#"{"mediaName":"手工图"}"#).expect("手工新增只要求名称");
        assert!(req.device_code.is_none());
        assert!(req.sha256.is_none());
        assert_eq!(req.media_name, "手工图");
    }

    #[test]
    fn upload_request_parses_camel_case_and_tolerates_missing() {
        let req: UploadMediaRequest = serde_json::from_str(
            r#"{"deviceCode":"RD-1","planId":"T-9","pointIndex":2,"capturedAt":"2026-11-20 10:00:00"}"#,
        )
        .expect("设备回传字段可缺省");
        assert_eq!(req.device_code.as_deref(), Some("RD-1"));
        assert_eq!(req.point_index, Some(2));
        assert!(req.sha256.is_none());
    }

    #[test]
    fn page_query_normalizes_zero_pagination() {
        let query = MediaPageQuery::default().normalized();
        assert_eq!(query.current, 1);
        assert_eq!(query.size, 10);
    }
}
