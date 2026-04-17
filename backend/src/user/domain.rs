use crate::user::entity::ActiveModel as UserActiveModel;
use crate::user::entity::Model as UserModel;
use chrono::{DateTime, Utc};
use sea_orm::ActiveValue;
use serde::{Deserialize, Serialize};

pub type User = UserModel;

#[derive(Debug, Deserialize, Clone)]
pub struct CreateUserRequest {
    pub username: String,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub real_name: Option<String>,
    pub password: Option<String>,
    pub org_id: Option<String>,
    pub remarks: Option<String>,
    pub card: Option<String>,
    pub sex: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub real_name: Option<String>,
    pub password: Option<String>,
    pub org_id: Option<String>,
    pub remarks: Option<String>,
    pub card: Option<String>,
    pub is_show: Option<i32>,
    pub enable: Option<i32>,
    pub sex: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct UserPageQuery {
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
    pub username: Option<String>,
    pub real_name: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub org_id: Option<String>,
    pub enable: Option<i32>,
}

impl UserPageQuery {
    pub fn page(&self) -> i64 {
        self.current.max(1)
    }

    pub fn size(&self) -> i64 {
        self.size.max(1)
    }

    pub fn offset(&self) -> i64 {
        (self.page() - 1) * self.size()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserVO {
    pub id: String,
    pub username: String,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub real_name: Option<String>,
    pub org_id: Option<String>,
    pub org_name: Option<String>,
    pub lock_time: Option<DateTime<Utc>>,
    pub last_login_time: Option<DateTime<Utc>>,
    pub try_count: Option<i32>,
    pub lock_flag: Option<i32>,
    pub create_time: DateTime<Utc>,
    pub update_time: DateTime<Utc>,
    pub remarks: Option<String>,
    pub pass_update_time: Option<DateTime<Utc>>,
    pub card: Option<String>,
    pub is_show: Option<i32>,
    pub enable: Option<i32>,
    pub first_login: Option<i32>,
    pub sex: Option<String>,
    pub role_names: Option<String>,
}

impl From<UserModel> for UserVO {
    fn from(m: UserModel) -> Self {
        Self {
            id: m.id,
            username: m.username,
            phone: m.phone,
            email: m.email,
            real_name: m.real_name,
            org_id: m.org_id,
            org_name: None,
            lock_time: m.lock_time,
            last_login_time: m.last_login_time,
            try_count: m.try_count,
            lock_flag: m.lock_flag,
            create_time: m.create_time,
            update_time: m.update_time,
            remarks: m.remarks,
            pass_update_time: m.pass_update_time,
            card: m.card,
            is_show: m.is_show,
            enable: m.enable,
            first_login: m.first_login,
            sex: m.sex,
            role_names: None,
        }
    }
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}

impl CreateUserRequest {
    pub fn to_active_model(&self, id: String, now: DateTime<Utc>) -> UserActiveModel {
        UserActiveModel {
            id: ActiveValue::set(id),
            username: ActiveValue::set(self.username.clone()),
            phone: set_opt_string(self.phone.clone()),
            email: set_opt_string(self.email.clone()),
            real_name: set_opt_string(self.real_name.clone()),
            password: set_opt_string(self.password.clone()),
            org_id: set_opt_string(self.org_id.clone()),
            lock_time: ActiveValue::set(None),
            last_login_time: ActiveValue::set(None),
            try_count: ActiveValue::set(Some(0)),
            lock_flag: ActiveValue::set(Some(1)),
            create_time: ActiveValue::set(now),
            update_time: ActiveValue::set(now),
            is_deleted: ActiveValue::set(0),
            remarks: set_opt_string(self.remarks.clone()),
            pass_update_time: ActiveValue::set(None),
            card: set_opt_string(self.card.clone()),
            is_show: ActiveValue::set(Some(1)),
            enable: ActiveValue::set(Some(1)),
            first_login: ActiveValue::set(Some(1)),
            sex: set_opt_string(self.sex.clone()),
        }
    }
}

impl UpdateUserRequest {
    pub fn to_active_model(&self, id: String) -> UserActiveModel {
        UserActiveModel {
            id: ActiveValue::unchanged(id),
            username: set_string(self.username.clone()),
            phone: set_opt_string(self.phone.clone()),
            email: set_opt_string(self.email.clone()),
            real_name: set_opt_string(self.real_name.clone()),
            password: set_opt_string(self.password.clone()),
            org_id: set_opt_string(self.org_id.clone()),
            remarks: set_opt_string(self.remarks.clone()),
            card: set_opt_string(self.card.clone()),
            is_show: set_opt_i32(self.is_show),
            enable: set_opt_i32(self.enable),
            sex: set_opt_string(self.sex.clone()),
            update_time: ActiveValue::set(Utc::now()),
            is_deleted: ActiveValue::unchanged(0),
            ..Default::default()
        }
    }
}

fn set_string(opt: Option<String>) -> ActiveValue<String> {
    match opt {
        Some(v) => ActiveValue::set(v),
        None => ActiveValue::not_set(),
    }
}

fn set_opt_string(opt: Option<String>) -> ActiveValue<Option<String>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}

fn set_opt_i32(opt: Option<i32>) -> ActiveValue<Option<i32>> {
    match opt {
        Some(v) => ActiveValue::set(Some(v)),
        None => ActiveValue::not_set(),
    }
}
