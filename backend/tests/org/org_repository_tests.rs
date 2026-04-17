use sea_orm::Database;
use uuid::Uuid;
use x_rust::common::traits::OrgRepository;
use x_rust::org::domain::{CreateOrgRequest, OrgTreeQuery, UpdateOrgRequest};
use x_rust::org::repository::SeaOrmOrgRepository;

fn test_db_path() -> String {
    let id = Uuid::new_v4();
    format!(
        "/home/zhangzexin/IdeaProjects/rust-admin/backend/data/org_test_{}.db",
        id
    )
}

struct TestDb {
    conn: sea_orm::DatabaseConnection,
    path: String,
}

impl TestDb {
    async fn new() -> Self {
        let db_path = test_db_path();
        let url = format!("sqlite:{}", db_path);

        if std::path::Path::new(&db_path).exists() {
            std::fs::remove_file(&db_path).ok();
        }
        std::fs::write(&db_path, "").ok();

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool.close().await;

        let conn = Database::connect(&url).await.unwrap();
        Self {
            conn,
            path: db_path,
        }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).ok();
    }
}

#[tokio::test]
async fn test_org_repo_create_and_find() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    let req = CreateOrgRequest {
        name: "Engineering".to_string(),
        sort: Some(1),
        parent_id: None,
        parent_name: None,
        org_duty: Some("Development".to_string()),
        desrc: Some("Engineering department".to_string()),
        r#type: Some("department".to_string()),
        is_out: Some(false),
        remarks: None,
    };

    let created_org = repo.create(&req, "org-1").await.unwrap();

    let found_org = repo.find_by_id("org-1").await.unwrap().unwrap();

    assert_eq!(found_org.id, "org-1");
    assert_eq!(found_org.name, "Engineering");
    assert_eq!(found_org.sort, Some(1));
    assert_eq!(found_org.r#type, Some("department".to_string()));
    assert_eq!(found_org.create_time, created_org.create_time);
    assert_eq!(found_org.update_time, created_org.update_time);
}

#[tokio::test]
async fn test_org_repo_find_all() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    let req1 = CreateOrgRequest {
        name: "Org 1".to_string(),
        sort: Some(1),
        parent_id: None,
        parent_name: None,
        org_duty: None,
        desrc: None,
        r#type: None,
        is_out: None,
        remarks: None,
    };
    repo.create(&req1, "o1").await.unwrap();

    let req2 = CreateOrgRequest {
        name: "Org 2".to_string(),
        sort: Some(2),
        parent_id: None,
        parent_name: None,
        org_duty: None,
        desrc: None,
        r#type: None,
        is_out: None,
        remarks: None,
    };
    repo.create(&req2, "o2").await.unwrap();

    let orgs = repo.find_all().await.unwrap();

    assert_eq!(orgs.len(), 2);
    assert!(orgs.iter().any(|o| o.id == "o1" && o.name == "Org 1"));
    assert!(orgs.iter().any(|o| o.id == "o2" && o.name == "Org 2"));
}

#[tokio::test]
async fn test_org_repo_find_tree() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    let req1 = CreateOrgRequest {
        name: "Root".to_string(),
        sort: Some(1),
        parent_id: None,
        parent_name: None,
        org_duty: None,
        desrc: None,
        r#type: None,
        is_out: None,
        remarks: None,
    };
    repo.create(&req1, "root").await.unwrap();

    let req2 = CreateOrgRequest {
        name: "Child".to_string(),
        sort: Some(2),
        parent_id: Some("root".to_string()),
        parent_name: Some("Root".to_string()),
        org_duty: None,
        desrc: None,
        r#type: None,
        is_out: None,
        remarks: None,
    };
    repo.create(&req2, "child").await.unwrap();

    let tree = repo.find_tree().await.unwrap();
    assert_eq!(tree.len(), 2);
}

#[tokio::test]
async fn test_org_repo_find_tree_with_filter_no_filter() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    repo.create(
        &CreateOrgRequest {
            name: "Org A".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: Some("type1".to_string()),
            is_out: None,
            remarks: None,
        },
        "a",
    )
    .await
    .unwrap();

    repo.create(
        &CreateOrgRequest {
            name: "Org B".to_string(),
            sort: Some(2),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: Some("type2".to_string()),
            is_out: None,
            remarks: None,
        },
        "b",
    )
    .await
    .unwrap();

    let query = OrgTreeQuery {
        name: None,
        r#type: None,
    };
    let result = repo.find_tree_with_filter(&query).await.unwrap();
    assert_eq!(result.len(), 2);
}

#[tokio::test]
async fn test_org_repo_find_tree_with_filter_name() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    repo.create(
        &CreateOrgRequest {
            name: "Engineering".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        "eng",
    )
    .await
    .unwrap();

    repo.create(
        &CreateOrgRequest {
            name: "Sales".to_string(),
            sort: Some(2),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        "sales",
    )
    .await
    .unwrap();

    let query = OrgTreeQuery {
        name: Some("Eng".to_string()),
        r#type: None,
    };
    let result = repo.find_tree_with_filter(&query).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "Engineering");
}

#[tokio::test]
async fn test_org_repo_find_tree_with_filter_type() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    repo.create(
        &CreateOrgRequest {
            name: "Company".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: Some("company".to_string()),
            is_out: None,
            remarks: None,
        },
        "c",
    )
    .await
    .unwrap();

    repo.create(
        &CreateOrgRequest {
            name: "Dept".to_string(),
            sort: Some(2),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: Some("department".to_string()),
            is_out: None,
            remarks: None,
        },
        "d",
    )
    .await
    .unwrap();

    let query = OrgTreeQuery {
        name: None,
        r#type: Some("department".to_string()),
    };
    let result = repo.find_tree_with_filter(&query).await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "Dept");
}

#[tokio::test]
async fn test_org_repo_find_tree_with_filter_combined() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    repo.create(
        &CreateOrgRequest {
            name: "Engineering".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: Some("department".to_string()),
            is_out: None,
            remarks: None,
        },
        "eng",
    )
    .await
    .unwrap();

    repo.create(
        &CreateOrgRequest {
            name: "Sales".to_string(),
            sort: Some(2),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: Some("department".to_string()),
            is_out: None,
            remarks: None,
        },
        "sales",
    )
    .await
    .unwrap();

    repo.create(
        &CreateOrgRequest {
            name: "Marketing".to_string(),
            sort: Some(3),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: Some("team".to_string()),
            is_out: None,
            remarks: None,
        },
        "mkt",
    )
    .await
    .unwrap();

    let query = OrgTreeQuery {
        name: Some("Sales".to_string()),
        r#type: Some("team".to_string()),
    };
    let result = repo.find_tree_with_filter(&query).await.unwrap();
    assert_eq!(result.len(), 0);
}

#[tokio::test]
async fn test_org_repo_find_by_parent_id() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    repo.create(
        &CreateOrgRequest {
            name: "Parent".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        "parent",
    )
    .await
    .unwrap();

    repo.create(
        &CreateOrgRequest {
            name: "Child 1".to_string(),
            sort: Some(2),
            parent_id: Some("parent".to_string()),
            parent_name: Some("Parent".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        "child1",
    )
    .await
    .unwrap();

    repo.create(
        &CreateOrgRequest {
            name: "Child 2".to_string(),
            sort: Some(3),
            parent_id: Some("parent".to_string()),
            parent_name: Some("Parent".to_string()),
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        "child2",
    )
    .await
    .unwrap();

    let children = repo.find_by_parent_id(Some("parent")).await.unwrap();
    assert_eq!(children.len(), 2);
}

#[tokio::test]
async fn test_org_repo_update() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    repo.create(
        &CreateOrgRequest {
            name: "Original".to_string(),
            sort: Some(1),
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        "org-1",
    )
    .await
    .unwrap();

    let update_req = UpdateOrgRequest {
        name: Some("Updated".to_string()),
        sort: Some(10),
        parent_id: None,
        parent_name: None,
        org_duty: None,
        desrc: None,
        r#type: Some("department".to_string()),
        is_out: None,
        remarks: None,
    };

    let updated_org = repo.update("org-1", &update_req).await.unwrap().unwrap();

    assert_eq!(updated_org.name, "Updated");
    assert_eq!(updated_org.sort, Some(10));
    assert_eq!(updated_org.r#type, Some("department".to_string()));
}

#[tokio::test]
async fn test_org_repo_update_not_found() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    let update_req = UpdateOrgRequest {
        name: Some("Updated".to_string()),
        sort: None,
        parent_id: None,
        parent_name: None,
        org_duty: None,
        desrc: None,
        r#type: None,
        is_out: None,
        remarks: None,
    };

    let result = repo.update("nonexistent", &update_req).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_org_repo_delete() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    repo.create(
        &CreateOrgRequest {
            name: "Delete Me".to_string(),
            sort: None,
            parent_id: None,
            parent_name: None,
            org_duty: None,
            desrc: None,
            r#type: None,
            is_out: None,
            remarks: None,
        },
        "org-del",
    )
    .await
    .unwrap();

    let deleted = repo.delete("org-del").await.unwrap();
    assert!(deleted);

    let found = repo.find_by_id("org-del").await.unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_org_repo_delete_not_found() {
    let test_db = TestDb::new().await;
    let repo = SeaOrmOrgRepository::new(test_db.conn.clone().into());

    let deleted = repo.delete("nonexistent").await.unwrap();
    assert!(!deleted);
}
