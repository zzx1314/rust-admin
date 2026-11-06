use crate::business::dog::route_library::entity::ActiveModel as RouteActiveModel;
use crate::business::dog::route_library::entity::Model as RouteModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type DogRouteLibrary = RouteModel;

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CreateRouteRequest {
    pub route_name: String,
    pub route_type: Option<String>,
    pub route_status: Option<String>,
    pub route_description: Option<String>,
    /// 所属园区（可选，与 `dog_park` 关联）。
    pub park_id: Option<i64>,
    pub model_id: i64,
    pub route_data: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRouteRequest {
    pub route_name: Option<String>,
    pub route_type: Option<String>,
    pub route_status: Option<String>,
    pub route_description: Option<String>,
    pub park_id: Option<i64>,
    pub model_id: Option<i64>,
    pub route_data: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RoutePageQuery {
    pub route_name: Option<String>,
    pub route_type: Option<String>,
    pub route_status: Option<String>,
    pub park_id: Option<i64>,
    pub begin_time: Option<String>,
    pub end_time: Option<String>,
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RouteVO {
    pub id: i64,
    pub route_name: String,
    pub route_type: Option<String>,
    pub route_status: Option<String>,
    pub route_description: Option<String>,
    pub park_id: Option<i64>,
    /// 所属园区名称，按 `park_id` 关联 `dog_park` 填充。
    pub park_name: Option<String>,
    pub model_id: i64,
    pub route_data: Option<String>,
    pub create_time: String,
    pub update_time: Option<String>,
}

impl From<DogRouteLibrary> for RouteVO {
    fn from(r: DogRouteLibrary) -> Self {
        Self {
            id: r.id,
            route_name: r.route_name,
            route_type: r.route_type,
            route_status: r.route_status,
            route_description: r.route_description,
            park_id: r.park_id,
            park_name: None,
            model_id: r.model_id,
            route_data: r.route_data,
            create_time: format_datetime(r.create_time),
            update_time: r.update_time.map(format_datetime),
        }
    }
}

impl CreateRouteRequest {
    pub fn to_active_model(&self, id: i64, now: DateTime<Utc>) -> RouteActiveModel {
        RouteActiveModel {
            id: ActiveValue::set(id),
            route_name: ActiveValue::set(self.route_name.clone()),
            route_type: ActiveValue::set(self.route_type.clone()),
            route_status: ActiveValue::set(self.route_status.clone()),
            route_description: ActiveValue::set(self.route_description.clone()),
            park_id: ActiveValue::set(self.park_id),
            model_id: ActiveValue::set(self.model_id),
            route_data: ActiveValue::set(self.route_data.clone()),
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
