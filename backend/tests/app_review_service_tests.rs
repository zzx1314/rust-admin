use sea_orm::ConnectionTrait;
use std::sync::Arc;

use x_rust::app_review::domain::{CreateReviewRequest, ReviewStatus};
use x_rust::app_review::repository::SeaOrmAppReviewRepository;
use x_rust::app_review::service::AppReviewService;
use x_rust::harbor::client::HarborClient;
use x_rust::harbor::service::HarborService;
use x_rust::config::HarborConfig;

struct TestDb {
    url: String,
    path: String,
}

impl TestDb {
    async fn new() -> Self {
        let id = uuid::Uuid::new_v4();
        let data_dir = "/home/zhangzexin/IdeaProjects/rust-admin/backend/data";
        std::fs::create_dir_all(data_dir).expect("Failed to create data directory");
        let path = format!("{}/app_review_{}.db", data_dir, id);
        let url = format!("sqlite:{}", path);

        if std::path::Path::new(&path).exists() {
            std::fs::remove_file(&path).ok();
        }
        std::fs::write(&path, "").ok();

        let conn = sea_orm::Database::connect(&url)
            .await
            .expect("Failed to connect to test database");

        let schema_sql = std::fs::read_to_string(
            "/home/zhangzexin/IdeaProjects/rust-admin/backend/migrations/p_sys/5_app_review.sql",
        )
        .expect("Failed to read app review schema SQL file");
        conn.execute_unprepared(&schema_sql)
            .await
            .expect("Failed to create app review table");

        Self { url, path }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

async fn create_service() -> (AppReviewService, SeaOrmAppReviewRepository, TestDb) {
    let test_db = TestDb::new().await;
    let conn = sea_orm::Database::connect(&test_db.url)
        .await
        .expect("Failed to connect");
    let conn = Arc::new(conn);

    let harbor_client = Arc::new(HarborClient::new(&HarborConfig {
        url: String::new(),
        username: String::new(),
        password: String::new(),
        staging_project: "staging-project".to_string(),
        production_project: "production-project".to_string(),
        registry_endpoint_id: None,
        registry_insecure: None,
        webhook_secret: None,
        replication_timeout_secs: 30,
    }));
    let harbor_service = Arc::new(HarborService::new(harbor_client));

    let repo = SeaOrmAppReviewRepository::new(conn);
    let service = AppReviewService::new(repo.clone(), harbor_service);

    (service, repo, test_db)
}

fn create_request(tag: &str, digest: Option<&str>) -> CreateReviewRequest {
    CreateReviewRequest {
        src_project: "staging-project".to_string(),
        dest_project: "production-project".to_string(),
        repository_name: "app/redis".to_string(),
        tag: tag.to_string(),
        digest: digest.map(|d| d.to_string()),
        artifact_id: None,
        reviewer_comment: None,
    }
}

#[tokio::test]
async fn test_create_review_success() {
    let (service, _repo, _db) = create_service().await;

    let req = create_request("v1.0", Some("sha256:abc123"));
    let review = service.create_review(req, Some(1)).await.unwrap();

    assert_eq!(review.status, ReviewStatus::Pending.as_str());
    assert_eq!(review.tag, "v1.0");
}

#[tokio::test]
async fn test_create_review_with_dedup_skips_identical_approved_artifact() {
    let (service, repo, _db) = create_service().await;

    // First push: create a pending review
    let req = create_request("v1.0", Some("sha256:abc123"));
    let review = service.create_review(req.clone(), Some(1)).await.unwrap();

    // Mark the review as approved directly (avoids calling Harbor replication in tests)
    repo.update_status(review.id, ReviewStatus::Approved.as_str(), 2, None)
        .await
        .unwrap();

    // Re-push the same artifact with identical digest
    let result = service.create_review_with_dedup(req, None).await.unwrap();

    assert!(result.is_none(), "Should skip creating a duplicate review for an already approved artifact with the same digest");
}

#[tokio::test]
async fn test_create_review_with_dedup_creates_new_review_for_different_digest() {
    let (service, repo, _db) = create_service().await;

    // First push: create and approve a review
    let req = create_request("v1.0", Some("sha256:abc123"));
    let review = service.create_review(req, Some(1)).await.unwrap();
    repo.update_status(review.id, ReviewStatus::Approved.as_str(), 2, None)
        .await
        .unwrap();

    // Re-push the same tag with a different digest
    let new_req = create_request("v1.0", Some("sha256:def456"));
    let new_review = service.create_review_with_dedup(new_req, None).await.unwrap();

    assert!(new_review.is_some(), "Should create a new review when digest differs");
    assert_eq!(new_review.unwrap().status, ReviewStatus::Pending.as_str());
}

#[tokio::test]
async fn test_create_review_with_dedup_creates_review_when_no_digest() {
    let (service, repo, _db) = create_service().await;

    // Approve a review without digest
    let req = create_request("v1.0", None);
    let review = service.create_review(req, Some(1)).await.unwrap();
    repo.update_status(review.id, ReviewStatus::Approved.as_str(), 2, None)
        .await
        .unwrap();

    // Re-push without digest should create a new review because we cannot deduplicate without a digest
    let new_req = create_request("v1.0", None);
    let new_review = service.create_review_with_dedup(new_req, None).await.unwrap();

    assert!(new_review.is_some(), "Should create a new review when no digest is provided");
}

#[tokio::test]
async fn test_create_review_with_dedup_creates_review_when_digest_changes_after_approval() {
    let (service, repo, _db) = create_service().await;

    let req = create_request("v1.0", Some("sha256:abc123"));
    let review = service.create_review(req.clone(), Some(1)).await.unwrap();
    repo.update_status(review.id, ReviewStatus::Approved.as_str(), 2, None)
        .await
        .unwrap();

    // Push a different artifact under the same tag
    let new_req = create_request("v1.0", Some("sha256:xyz789"));
    let new_review = service.create_review_with_dedup(new_req, None).await.unwrap();

    assert!(new_review.is_some(), "Different digest should trigger a new review");
}
