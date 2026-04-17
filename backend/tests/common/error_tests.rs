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
async fn test_database_error() {
    let sqlx_error = sqlx::Error::RowNotFound;
    let error = AppError::DatabaseError(sqlx_error);
    let response = error.into_response();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = to_bytes(response.into_body(), 1024).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["code"], FAIL_CODE);
    assert!(json["msg"].to_string().contains("Database error"));
    assert!(json["data"].is_null());
}
