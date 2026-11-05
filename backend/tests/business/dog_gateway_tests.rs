//! End-to-end tests for the device gateway: a `dog-device-client` instance
//! authenticates, registers, opens the WebSocket link, receives a task pushed
//! by the server, reports the result back and uploads a file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dog_device_client::{CommandResult, DeviceClient, DeviceRegistration, Heartbeat};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use sea_orm_migration::MigratorTrait;
use tokio::net::TcpListener;

use x_rust::api::routes::create_router;
use x_rust::app::App;
use x_rust::business::dog::command::entity::{Column as CommandColumn, Entity as CommandEntity};
use x_rust::business::dog::device::entity::{Column as DeviceColumn, Entity as DeviceEntity};
use x_rust::business::dog::media::entity::{Column as MediaColumn, Entity as MediaEntity};
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore, UserRepository};
use x_rust::common::util::{encrypt_password, md5_encrypt};
use x_rust::migration::Migrator;
use x_rust::system::sys_user::domain::CreateUserRequest;
use x_rust::system::sys_user::repository::SeaOrmUserRepository;

const DEVICE_ID: &str = "dog-e2e-0001";
const USERNAME: &str = "gateway_tester";
const PASSWORD: &str = "password123";

// ---------------------------------------------------------------------------
// test harness
// ---------------------------------------------------------------------------

struct FakeTokenStore {
    store: Arc<Mutex<HashMap<String, String>>>,
}

impl FakeTokenStore {
    fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl TokenStore for FakeTokenStore {
    fn set_token(
        &self,
        user_id: &str,
        token: &str,
        _ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        let token = token.to_string();
        Box::pin(async move {
            store.lock().unwrap().insert(key, token);
            Ok(())
        })
    }

    fn get_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        Box::pin(async move { Ok(store.lock().unwrap().get(&key).cloned()) })
    }

    fn delete_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:token:{}", user_id);
        Box::pin(async move {
            store.lock().unwrap().remove(&key);
            Ok(())
        })
    }

    fn set_refresh_token(
        &self,
        user_id: &str,
        token: &str,
        _ttl_secs: u64,
    ) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        let token = token.to_string();
        Box::pin(async move {
            store.lock().unwrap().insert(key, token);
            Ok(())
        })
    }

    fn get_refresh_token(&self, user_id: &str) -> DynFuture<Result<Option<String>, AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        Box::pin(async move { Ok(store.lock().unwrap().get(&key).cloned()) })
    }

    fn delete_refresh_token(&self, user_id: &str) -> DynFuture<Result<(), AppError>> {
        let store = self.store.clone();
        let key = format!("auth:refresh:{}", user_id);
        Box::pin(async move {
            store.lock().unwrap().remove(&key);
            Ok(())
        })
    }
}

struct TestServer {
    base_url: String,
    conn: Arc<DatabaseConnection>,
    upload_dir: PathBuf,
    db_path: PathBuf,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
        std::fs::remove_file(&self.db_path).ok();
        std::fs::remove_dir_all(&self.upload_dir).ok();
    }
}

impl TestServer {
    async fn start() -> Self {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let db_path = std::env::temp_dir().join(format!("dog_gateway_{suffix}.db"));
        let upload_dir = std::env::temp_dir().join(format!("dog_gateway_uploads_{suffix}"));
        let database_url = format!("sqlite:{}?mode=rwc", db_path.display());

        let conn = sea_orm::Database::connect(&database_url)
            .await
            .expect("failed to connect test db");
        <Migrator as MigratorTrait>::up(&conn, None)
            .await
            .expect("failed to migrate test db");

        let app = App::from_connection_with_upload_dir(conn.clone(), &upload_dir);
        let state =
            app.build_state_with_token_store(Arc::new(FakeTokenStore::new()), "test-secret");

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
        let addr = listener.local_addr().expect("no local addr");
        let router = create_router(state.clone());
        let host_port = addr.to_string();
        let handle = tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .expect("test server failed");
        });

        Self {
            base_url: format!("http://{host_port}"),
            conn: Arc::new(conn),
            upload_dir,
            db_path,
            handle,
        }
    }

    async fn login(&self) -> String {
        SeaOrmUserRepository::new(self.conn.clone())
            .create(
                &CreateUserRequest {
                    username: USERNAME.to_string(),
                    phone: None,
                    email: None,
                    real_name: Some("Gateway Tester".to_string()),
                    password: Some(md5_encrypt(PASSWORD)),
                    org_id: 1,
                    remarks: None,
                    card: None,
                    sex: None,
                    role: None,
                },
                &999_000_001i64,
            )
            .await
            .expect("failed to create test user");

        let encrypted = encrypt_password(PASSWORD);
        let response = reqwest::Client::new()
            .post(format!("{}/api/token", self.base_url))
            .form(&[
                ("username", USERNAME),
                ("password", encrypted.as_str()),
                ("grant_type", "password"),
                ("scope", "server"),
            ])
            .send()
            .await
            .expect("login request failed");

        let body: serde_json::Value = response.json().await.expect("login body is not json");
        body["access_token"]
            .as_str()
            .expect("login response without access_token")
            .to_string()
    }
}

async fn wait_device(
    conn: &Arc<DatabaseConnection>,
    check: impl Fn(&x_rust::business::dog::device::entity::Model) -> bool,
) -> x_rust::business::dog::device::entity::Model {
    for _ in 0..100 {
        if let Some(device) = DeviceEntity::find()
            .filter(DeviceColumn::DeviceId.eq(DEVICE_ID))
            .one(&**conn)
            .await
            .expect("device query failed")
            && check(&device)
        {
            return device;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("device condition not met in time");
}

async fn wait_command(
    conn: &Arc<DatabaseConnection>,
    id: i64,
    check: impl Fn(&x_rust::business::dog::command::entity::Model) -> bool,
) -> x_rust::business::dog::command::entity::Model {
    for _ in 0..100 {
        if let Some(command) = CommandEntity::find()
            .filter(CommandColumn::Id.eq(id))
            .one(&**conn)
            .await
            .expect("command query failed")
            && check(&command)
        {
            return command;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("command condition not met in time");
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_device_gateway_end_to_end() {
    let server = TestServer::start().await;
    let user_token = server.login().await;

    // ---- device side: authenticate + register ------------------------------
    let mut client = DeviceClient::new(server.base_url.clone(), DEVICE_ID);
    assert!(client.connect().await.is_err(), "connect must need a token");

    let token = client.authenticate().await.expect("device auth failed");
    assert!(!token.is_empty());

    let registered = client
        .register(&DeviceRegistration {
            device_id: DEVICE_ID.to_string(),
            device_ip: Some("192.168.1.20".to_string()),
            device_type: Some("X1".to_string()),
            ..DeviceRegistration::default()
        })
        .await
        .expect("device register failed");
    assert_eq!(registered.device_id, DEVICE_ID);
    assert_eq!(registered.status, "offline");

    // ---- link + heartbeat --------------------------------------------------
    let mut link = client.connect().await.expect("websocket connect failed");
    let ack = link
        .heartbeat_with_ack(
            &Heartbeat::online(DEVICE_ID).with_telemetry(dog_device_client::Telemetry {
                battery: Some("88".to_string()),
                location: Some("31.23,121.47".to_string()),
                ..dog_device_client::Telemetry::default()
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("heartbeat was not acknowledged");
    assert_eq!(ack["status"], "online");

    let online = wait_device(&server.conn, |device| device.status == "online").await;
    assert!(online.comm_time.is_some());
    assert_eq!(online.battery.as_deref(), Some("88"));
    assert_eq!(online.location.as_deref(), Some("31.23,121.47"));

    // ---- admin side: create a command through the REST API -----------------
    let response = reqwest::Client::new()
        .post(format!("{}/api/dogCommand", server.base_url))
        .bearer_auth(&user_token)
        .json(&serde_json::json!({
            "commandName": "巡检任务",
            "commandType": "task",
            "deviceId": DEVICE_ID,
            "commandParams": "{\"head\":\"SCAN\",\"speed\":\"1.2\"}",
        }))
        .send()
        .await
        .expect("create command request failed");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.expect("command body is not json");
    let command_id = body["data"]["id"].as_i64().expect("command id missing");
    // being online, the command is pushed immediately and marked as sent
    assert_eq!(body["data"]["status"], "sent");

    // ---- device receives the task -----------------------------------------
    let task = link
        .next_command(Duration::from_secs(5))
        .await
        .expect("receiving task failed")
        .expect("no task pushed by the server");
    assert_eq!(task.id(), Some(command_id));
    assert_eq!(task.command_name.as_deref(), Some("巡检任务"));
    assert_eq!(task.command_type.as_deref(), Some("task"));
    assert_eq!(task.params["head"], "SCAN");
    assert_eq!(task.params["speed"], "1.2");

    // ---- device reports the result ----------------------------------------
    link.report(&CommandResult::success(command_id, "scan finished"))
        .await
        .expect("report failed");

    let finished = wait_command(&server.conn, command_id, |command| {
        command.status == "success"
    })
    .await;
    assert_eq!(finished.return_value.as_deref(), Some("scan finished"));

    // the REST API exposes the reported result as well
    let detail: serde_json::Value = reqwest::Client::new()
        .get(format!("{}/api/dogCommand/{command_id}", server.base_url))
        .bearer_auth(&user_token)
        .send()
        .await
        .expect("command detail request failed")
        .json()
        .await
        .expect("command detail body is not json");
    assert_eq!(detail["data"]["status"], "success");
    assert_eq!(detail["data"]["returnValue"], "scan finished");

    // ---- file upload -------------------------------------------------------
    let upload_file = server.upload_dir.join("sample.txt");
    tokio::fs::create_dir_all(&server.upload_dir)
        .await
        .expect("failed to create upload dir");
    let payload = b"dog gateway upload payload";
    tokio::fs::write(&upload_file, payload)
        .await
        .expect("failed to write sample file");

    let ack = link
        .upload_file(&upload_file, Some("image"), None, Duration::from_secs(10))
        .await
        .expect("upload failed");
    assert!(ack.is_success(), "upload not acknowledged: {ack:?}");

    let media = MediaEntity::find()
        .filter(MediaColumn::MediaName.eq("sample.txt"))
        .one(&*server.conn)
        .await
        .expect("media query failed")
        .expect("uploaded file was not recorded as media");
    assert_eq!(media.media_size, Some(payload.len() as i64));
    let stored_path = media.media_path.expect("media path missing");
    assert!(
        std::path::Path::new(&stored_path).exists(),
        "uploaded file missing at {stored_path}"
    );
    assert_eq!(
        tokio::fs::read(&stored_path)
            .await
            .expect("failed to read stored upload"),
        payload
    );

    // ---- disconnect marks the device offline ------------------------------
    link.close().await.expect("closing link failed");
    wait_device(&server.conn, |device| device.status == "offline").await;
}

#[tokio::test]
async fn test_device_gateway_rejects_invalid_credentials() {
    let server = TestServer::start().await;
    let http = reqwest::Client::new();

    // register without a token
    let response = http
        .post(format!("{}/api/dogGateway/register", server.base_url))
        .json(&serde_json::json!({ "deviceId": DEVICE_ID }))
        .send()
        .await
        .expect("register request failed");
    assert_eq!(response.status(), 401);

    // register with an unknown token
    let response = http
        .post(format!("{}/api/dogGateway/register", server.base_url))
        .header("Authorization", "not-a-token")
        .json(&serde_json::json!({ "deviceId": DEVICE_ID }))
        .send()
        .await
        .expect("register request failed");
    assert_eq!(response.status(), 401);

    // websocket handshake with a token issued for another device
    let mut client = DeviceClient::new(server.base_url.clone(), DEVICE_ID);
    let token = client.authenticate().await.expect("device auth failed");
    let response = http
        .get(format!(
            "{}/api/notice/another-device/{token}",
            server.base_url
        ))
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .expect("upgrade request failed");
    assert_eq!(response.status(), 401);

    // admin endpoints still require a user JWT
    let response = http
        .get(format!("{}/api/dogGateway/connections", server.base_url))
        .send()
        .await
        .expect("connections request failed");
    assert_eq!(response.status(), 401);
}
