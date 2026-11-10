use axum::body::to_bytes;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json;
use x_rust::common::error::{AppError, FAIL_CODE};

#[tokio::test]
async fn test_not_found_error() {
    let error = AppError::NotFound("user 1".to_string());
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], FAIL_CODE);
    assert_eq!(json["msg"], "Not found: user 1");
    assert!(json["data"].is_null());
}

#[tokio::test]
async fn test_validation_error() {
    let error = AppError::ValidationError("bad email".to_string());
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], FAIL_CODE);
    assert_eq!(json["msg"], "Validation error: bad email");
    assert!(json["data"].is_null());
}

#[tokio::test]
async fn test_database_error_is_masked() {
    let db_error = sea_orm::DbErr::Custom("no such column: secret_column".to_string());
    let error = AppError::DatabaseError(db_error);
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], FAIL_CODE);

    let msg = json["msg"].as_str().unwrap_or_default();
    // Internals must not reach the client.
    assert!(!msg.contains("secret_column"), "leaked detail: {msg}");
    assert!(!msg.contains("Database error"), "leaked detail: {msg}");
    assert_eq!(msg, "服务内部错误，请稍后重试");
    assert!(json["data"].is_null());
}

#[tokio::test]
async fn test_internal_error_is_masked() {
    let error = AppError::InternalError("token generation failed: bad key".to_string());
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let msg = json["msg"].as_str().unwrap_or_default();
    assert!(!msg.contains("bad key"), "leaked detail: {msg}");
    assert_eq!(msg, "服务内部错误，请稍后重试");
}

#[tokio::test]
async fn test_unauthorized_message_is_preserved() {
    // Domain errors carry user-facing text and must pass through unchanged.
    let error = AppError::Unauthorized("用户名或密码错误".to_string());
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["msg"], "Unauthorized: 用户名或密码错误");
}
