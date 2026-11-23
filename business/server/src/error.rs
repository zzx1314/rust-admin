use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    NotFound(String),
    /// 已登录但没有权限（例如只有管理员能查看凭据原文）
    #[error("{0}")]
    Forbidden(String),
    /// 上游不可用（中台连不上 / 返回了无法理解的内容）
    ///
    /// 与 `Internal` 分开是为了**登录**这类场景：账号密码错是中台给出的 401，
    /// 中台本身连不上不该报成 500「服务器错误」，否则现场会以为业务平台崩了。
    #[error("{0}")]
    Upstream(String),
    #[error("{0}")]
    Internal(String),
}

/// 数据库错误 → 对外错误。
///
/// **原始错误只进日志**：`sqlx` / `SQLite` 的英文原文（`no such column: xxx`、
/// `UNIQUE constraint failed: tasks.task_code`、连接池超时…）不该出现在接口响应里，
/// `IntoResponse` 会把 `Internal` 的文案直接回给前端，以前等于把库结构泄露给浏览器。
/// 需要对外说明的场合请显式写中文文案（`Internal("…")` / `Upstream("…")`）。
impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::NotFound("记录不存在".to_string()),
            // 唯一键冲突是**可预期的并发结果**（编号被手填占用、两个请求同时建单、
            // 账号已经绑过责任人…），按 400 让调用方自己改，而不是 500「服务器错误」。
            // 注意文案不能只说「编号」：unique 约束还有 `users.username`、
            // `staff.user_id`、`devices.device_id` 这些不是编号的列。
            sqlx::Error::Database(database) if database.is_unique_violation() => {
                tracing::warn!(error = %database, "database unique violation");
                Self::BadRequest("记录已存在（编号或唯一绑定重复），请检查后重试".to_string())
            }
            other => {
                tracing::error!(error = %other, "database error");
                Self::Internal("数据库操作失败".to_string())
            }
        }
    }
}

impl From<bcrypt::BcryptError> for AppError {
    fn from(error: bcrypt::BcryptError) -> Self {
        Self::Internal(error.to_string())
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(_: jsonwebtoken::errors::Error) -> Self {
        Self::Unauthorized("登录状态已失效，请重新登录".to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, 10400),
            Self::Unauthorized(_) => (StatusCode::UNAUTHORIZED, 10401),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, 10404),
            Self::Forbidden(_) => (StatusCode::FORBIDDEN, 10403),
            Self::Upstream(_) => (StatusCode::SERVICE_UNAVAILABLE, 10503),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, 10500),
        };
        let message = self.to_string();
        match status {
            StatusCode::INTERNAL_SERVER_ERROR => {
                tracing::error!(error = %message, "request failed")
            }
            // 上游不可用是部署/网络问题，按 warn 记，别刷成 error 噪音
            StatusCode::SERVICE_UNAVAILABLE => {
                tracing::warn!(error = %message, "upstream unavailable")
            }
            _ => {}
        }
        (
            status,
            Json(json!({
                "code": code,
                "msg": message,
                "data": null
            })),
        )
            .into_response()
    }
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: u16,
    pub msg: String,
    pub data: T,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Json<Self> {
        Json(Self {
            code: 10200,
            msg: "success".to_string(),
            data,
        })
    }
}
