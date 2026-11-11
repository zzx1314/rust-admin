//! 控制面测试：中台设备指令 / 任务下发 / 运行态。
//!
//! 中台不持有设备 WebSocket，所有「对设备做动作」的请求都转发给业务平台。
//! 这里用一个假的业务平台 HTTP 服务验证四件事：
//!   1. 转发到正确的内部接口（路径带设备编号与后缀）；
//!   2. 同一 `requestId` 重试不会让业务平台重复下发（幂等）；
//!   3. 指令白名单、设备可见性（`dog_user_device`）生效；
//!   4. 业务平台的 4xx 消息原样透出、5xx 收敛成内部错误。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::Path as AxumPath;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use x_rust::api::AppState;
use x_rust::api::middleware::RequestUser;
use x_rust::app::App;
use x_rust::business::dog::device::audit;
use x_rust::business::dog::device::domain::{
    CreateDeviceRequest, DeviceCommandRequest, DispatchTaskRequest,
};
use x_rust::business::dog::device::handlers::{
    IdParam, device_runtime_handler, dispatch_device_task_handler, send_device_command_handler,
    sync_device_maps_handler,
};
use x_rust::common::data_scope::DataScope;
use x_rust::common::error::AppError;
use x_rust::common::traits::{DynFuture, TokenStore};

const JWT_SECRET: &str = "test-secret";

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

/// 假业务平台的调用计数，用来断言「没有重复下发」。
#[derive(Clone, Default)]
struct Hits {
    command: Arc<AtomicUsize>,
    task: Arc<AtomicUsize>,
    runtime: Arc<AtomicUsize>,
}

impl Hits {
    fn command(&self) -> usize {
        self.command.load(Ordering::SeqCst)
    }

    fn task(&self) -> usize {
        self.task.load(Ordering::SeqCst)
    }

    fn runtime(&self) -> usize {
        self.runtime.load(Ordering::SeqCst)
    }
}

/// 启动一个只为测试存在的业务平台，返回地址与调用计数。
async fn spawn_fake_business() -> (String, Hits) {
    let hits = Hits::default();
    let command_hits = hits.command.clone();
    let task_hits = hits.task.clone();
    let runtime_hits = hits.runtime.clone();

    let app = Router::new()
        .route(
            "/api/zentai/devices/{code}/command",
            post(
                move |AxumPath(code): AxumPath<String>, Json(body): Json<Value>| {
                    let counter = command_hits.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(code, "RD-100");
                        assert_eq!(body["command"], "召回充电");
                        Json(json!({
                            "code": 10200,
                            "msg": "success",
                            "data": {
                                "duplicated": false,
                                "result": {"wsDelivered": true, "deviceId": code},
                            },
                        }))
                    }
                },
            ),
        )
        .route(
            "/api/zentai/devices/{code}/tasks",
            post(
                move |AxumPath(code): AxumPath<String>, Json(body): Json<Value>| {
                    let counter = task_hits.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(code, "RD-100");
                        assert_eq!(body["routeId"], 7);
                        Json(json!({
                            "code": 10200,
                            "msg": "success",
                            "data": {
                                "duplicated": false,
                                "result": {"taskId": 42, "taskCode": "TK-1"},
                            },
                        }))
                    }
                },
            ),
        )
        .route(
            "/api/zentai/devices/{code}/runtime",
            get(move |AxumPath(code): AxumPath<String>| {
                let counter = runtime_hits.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Json(json!({
                        "code": 10200,
                        "msg": "success",
                        "data": {"deviceId": code, "online": true, "battery": 88},
                    }))
                }
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake business server");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    (format!("http://{addr}"), hits)
}

async fn test_state(base_url: &str) -> AppState {
    let app = App::new("sqlite::memory:")
        .await
        .expect("in-memory database must migrate");
    let mut state = app.build_state_with_token_store(Arc::new(FakeTokenStore::new()), JWT_SECRET);
    state.business_api_url = Some(base_url.to_string());
    state.business_api_token = Some("test-service-token".to_string());
    state
}

async fn create_device(state: &AppState, code: &str) -> i64 {
    state
        .dog_device_service
        .create(
            CreateDeviceRequest {
                device_id: code.to_string(),
                device_name: Some("测试狗".to_string()),
                model: None,
                status: Some("在线待命".to_string()),
                firmware: None,
                device_ip: None,
                park_code: None,
                park_name: None,
                remarks: None,
                user_ids: None,
            },
            &DataScope::new(1, true),
        )
        .await
        .expect("device must be created")
        .id
}

fn admin() -> RequestUser {
    RequestUser {
        user_id: 1,
        username: "admin".to_string(),
        is_admin: true,
    }
}

fn command_request(command: &str, request_id: &str) -> DeviceCommandRequest {
    DeviceCommandRequest {
        command: command.to_string(),
        params: None,
        request_id: Some(request_id.to_string()),
    }
}

#[tokio::test]
async fn forwards_command_and_audits_it() {
    let (base, hits) = spawn_fake_business().await;
    let state = test_state(&base).await;
    let device_id = create_device(&state, "RD-100").await;

    let response = send_device_command_handler(
        axum::extract::State(state.clone()),
        axum::Extension(admin()),
        AxumPath(IdParam { id: device_id }),
        Json(command_request("召回充电", "req-command-1")),
    )
    .await
    .expect("command must succeed")
    .0
    .data;

    assert_eq!(hits.command(), 1);
    assert_eq!(response["duplicated"], false);
    assert_eq!(response["action"], "召回充电");
    assert_eq!(response["result"]["result"]["wsDelivered"], true);

    // 审计必须落库：谁、哪台设备、什么指令、结果如何
    let audit = state
        .dog_device_service
        .find_command("req-command-1")
        .await
        .expect("audit lookup must work")
        .expect("audit row must exist");
    assert_eq!(audit.status, audit::STATUS_SUCCEEDED);
    assert!(audit.result.unwrap_or_default().contains("wsDelivered"));
}

#[tokio::test]
async fn retried_command_is_not_dispatched_twice() {
    let (base, hits) = spawn_fake_business().await;
    let state = test_state(&base).await;
    let device_id = create_device(&state, "RD-100").await;

    for attempt in 0..2 {
        let response = send_device_command_handler(
            axum::extract::State(state.clone()),
            axum::Extension(admin()),
            AxumPath(IdParam { id: device_id }),
            Json(command_request("召回充电", "req-command-retry")),
        )
        .await
        .expect("retry must succeed")
        .0
        .data;
        assert_eq!(response["duplicated"], attempt == 1);
    }

    // 第二次是幂等回放：业务平台只被调用一次，设备不会收到两条指令
    assert_eq!(hits.command(), 1);
}

#[tokio::test]
async fn rejects_command_outside_whitelist() {
    let (base, hits) = spawn_fake_business().await;
    let state = test_state(&base).await;
    let device_id = create_device(&state, "RD-100").await;

    let error = send_device_command_handler(
        axum::extract::State(state),
        axum::Extension(admin()),
        AxumPath(IdParam { id: device_id }),
        Json(command_request("关机", "req-command-bad")),
    )
    .await
    .expect_err("unknown command must be rejected");

    assert!(matches!(error, AppError::BadRequest(_)));
    assert_eq!(hits.command(), 0);
}

#[tokio::test]
async fn user_without_assignment_cannot_command_device() {
    let (base, hits) = spawn_fake_business().await;
    let state = test_state(&base).await;
    let device_id = create_device(&state, "RD-100").await;

    let error = send_device_command_handler(
        axum::extract::State(state),
        axum::Extension(RequestUser {
            user_id: 99,
            username: "nobody".to_string(),
            is_admin: false,
        }),
        AxumPath(IdParam { id: device_id }),
        Json(command_request("召回充电", "req-command-denied")),
    )
    .await
    .expect_err("unassigned user must be rejected");

    assert!(matches!(error, AppError::NotFound(_)));
    assert_eq!(hits.command(), 0);
}

#[tokio::test]
async fn dispatches_task_and_reports_to_business() {
    let (base, hits) = spawn_fake_business().await;
    let state = test_state(&base).await;
    let device_id = create_device(&state, "RD-100").await;

    let response = dispatch_device_task_handler(
        axum::extract::State(state.clone()),
        axum::Extension(admin()),
        AxumPath(IdParam { id: device_id }),
        Json(DispatchTaskRequest {
            task_name: "A 区夜间巡检".to_string(),
            route_id: 7,
            plan_start: None,
            plan_end: None,
            execute_mode: None,
            level: None,
            description: None,
            request_id: Some("req-task-1".to_string()),
        }),
    )
    .await
    .expect("task dispatch must succeed")
    .0
    .data;

    assert_eq!(hits.task(), 1);
    assert_eq!(response["result"]["result"]["taskCode"], "TK-1");

    let audit = state
        .dog_device_service
        .find_command("req-task-1")
        .await
        .expect("audit lookup must work")
        .expect("audit row must exist");
    assert_eq!(audit.status, audit::STATUS_SUCCEEDED);
}

#[tokio::test]
async fn rejects_task_without_route() {
    let (base, hits) = spawn_fake_business().await;
    let state = test_state(&base).await;
    let device_id = create_device(&state, "RD-100").await;

    let error = dispatch_device_task_handler(
        axum::extract::State(state),
        axum::Extension(admin()),
        AxumPath(IdParam { id: device_id }),
        Json(DispatchTaskRequest {
            task_name: "A 区夜间巡检".to_string(),
            route_id: 0,
            plan_start: None,
            plan_end: None,
            execute_mode: None,
            level: None,
            description: None,
            request_id: None,
        }),
    )
    .await
    .expect_err("missing route must be rejected");

    assert!(matches!(error, AppError::BadRequest(_)));
    assert_eq!(hits.task(), 0);
}

#[tokio::test]
async fn reads_runtime_without_auditing() {
    let (base, hits) = spawn_fake_business().await;
    let state = test_state(&base).await;
    let device_id = create_device(&state, "RD-100").await;

    let data = device_runtime_handler(
        axum::extract::State(state.clone()),
        axum::Extension(admin()),
        AxumPath(IdParam { id: device_id }),
    )
    .await
    .expect("runtime must be readable")
    .0
    .data;

    assert_eq!(hits.runtime(), 1);
    assert_eq!(data["online"], true);
    assert_eq!(data["battery"], 88);
}

#[tokio::test]
async fn surfaces_business_rejection_message() {
    let state = test_state("http://127.0.0.1:1").await;
    let device_id = create_device(&state, "RD-100").await;

    // 业务平台不可达：必须是内部错误（对外收敛成通用提示），而不是「指令已下发」
    let error = send_device_command_handler(
        axum::extract::State(state),
        axum::Extension(admin()),
        AxumPath(IdParam { id: device_id }),
        Json(command_request("召回充电", "req-command-offline")),
    )
    .await
    .expect_err("unreachable business platform must fail");

    assert!(matches!(error, AppError::InternalError(_)));
}

/// 反向控制信道（方案 B）：业务平台主动拨入中台，指令走 WS 而不是 HTTP。
///
/// 这一组用例刻意**不配置** `business_api_url`：中台完全没有能力连业务平台，
/// 全部靠业务平台拨入的连接，正是内网部署的真实约束。
mod control_channel_transport {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;
    use x_rust::api::routes::create_router;

    const TOKEN: &str = "test-service-token";

    async fn state_without_http() -> AppState {
        let app = App::new("sqlite::memory:")
            .await
            .expect("in-memory database must migrate");
        let mut state =
            app.build_state_with_token_store(Arc::new(FakeTokenStore::new()), JWT_SECRET);
        // 只配令牌（信道鉴权用），不配业务平台地址：中台无法主动连出去
        state.business_api_url = None;
        state.business_api_token = Some(TOKEN.to_string());
        state
    }

    /// 启动一个真实监听的中台服务，返回控制信道地址。
    async fn spawn_zentai(state: AppState) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind zentai");
        let addr = listener.local_addr().expect("local addr");
        let router = create_router(state);
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("ws://{addr}/api/dogDevice/control-plane/ws")
    }

    async fn dial(
        url: &str,
        token: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let mut request = url.into_client_request().expect("build ws request");
        request
            .headers_mut()
            .insert("x-service-token", HeaderValue::from_str(token).unwrap());
        tokio_tungstenite::connect_async(request)
            .await
            .expect("ws connect")
            .0
    }

    async fn wait_online(state: &AppState) {
        for _ in 0..200 {
            if state.control_channel.online().await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("控制信道未建立");
    }

    #[tokio::test]
    async fn command_is_delivered_over_the_control_channel() {
        let state = state_without_http().await;
        let device_id = create_device(&state, "RD-100").await;
        let url = spawn_zentai(state.clone()).await;
        let mut socket = dial(&url, TOKEN).await;
        wait_online(&state).await;

        // 业务平台侧：收到 request 帧后回执
        let business = tokio::spawn(async move {
            while let Some(message) = socket.next().await {
                let Ok(Message::Text(text)) = message else {
                    continue;
                };
                let frame: Value = serde_json::from_str(&text).expect("frame is json");
                if frame["type"] != "request" {
                    continue;
                }
                assert_eq!(frame["payload"]["deviceCode"], "RD-100");
                assert_eq!(frame["payload"]["kind"], "command");
                assert_eq!(frame["payload"]["body"]["command"], "召回充电");
                assert_eq!(frame["payload"]["operatorId"], 1);
                let request_id = frame["payload"]["requestId"].as_str().unwrap().to_string();
                let reply = json!({
                    "type": "result",
                    "payload": {
                        "requestId": request_id,
                        "ok": true,
                        "data": {"duplicated": false, "result": {"wsDelivered": true}},
                    },
                });
                socket
                    .send(Message::Text(reply.to_string()))
                    .await
                    .expect("send result");
                return;
            }
            panic!("没有收到控制请求");
        });

        let response = send_device_command_handler(
            axum::extract::State(state.clone()),
            axum::Extension(admin()),
            AxumPath(IdParam { id: device_id }),
            Json(command_request("召回充电", "req-ws-1")),
        )
        .await
        .expect("command over channel must succeed")
        .0
        .data;

        business.await.expect("business side must finish");
        // 形状与 HTTP 链路完全一致：调用方不需要知道走的是哪条链路
        assert_eq!(response["result"]["result"]["wsDelivered"], true);

        let audit = state
            .dog_device_service
            .find_command("req-ws-1")
            .await
            .expect("audit lookup")
            .expect("audit row");
        assert_eq!(audit.status, audit::STATUS_SUCCEEDED);
    }

    #[tokio::test]
    async fn runtime_is_served_over_the_control_channel() {
        let state = state_without_http().await;
        let device_id = create_device(&state, "RD-100").await;
        let url = spawn_zentai(state.clone()).await;
        let mut socket = dial(&url, TOKEN).await;
        wait_online(&state).await;

        let business = tokio::spawn(async move {
            while let Some(message) = socket.next().await {
                let Ok(Message::Text(text)) = message else {
                    continue;
                };
                let frame: Value = serde_json::from_str(&text).expect("frame is json");
                if frame["type"] != "request" {
                    continue;
                }
                assert_eq!(frame["payload"]["kind"], "runtime");
                let request_id = frame["payload"]["requestId"].as_str().unwrap().to_string();
                let reply = json!({
                    "type": "result",
                    "payload": {
                        "requestId": request_id,
                        "ok": true,
                        "data": {"deviceId": "RD-100", "online": false, "battery": 42},
                    },
                });
                socket
                    .send(Message::Text(reply.to_string()))
                    .await
                    .expect("send result");
                return;
            }
            panic!("没有收到运行态请求");
        });

        let data = device_runtime_handler(
            axum::extract::State(state.clone()),
            axum::Extension(admin()),
            AxumPath(IdParam { id: device_id }),
        )
        .await
        .expect("runtime over channel must succeed")
        .0
        .data;

        business.await.expect("business side must finish");
        assert_eq!(data["online"], false);
        assert_eq!(data["battery"], 42);
    }

    #[tokio::test]
    async fn business_rejection_is_surfaced_to_the_operator() {
        let state = state_without_http().await;
        let device_id = create_device(&state, "RD-100").await;
        let url = spawn_zentai(state.clone()).await;
        let mut socket = dial(&url, TOKEN).await;
        wait_online(&state).await;

        let business = tokio::spawn(async move {
            while let Some(message) = socket.next().await {
                let Ok(Message::Text(text)) = message else {
                    continue;
                };
                let frame: Value = serde_json::from_str(&text).expect("frame is json");
                if frame["type"] != "request" {
                    continue;
                }
                let request_id = frame["payload"]["requestId"].as_str().unwrap().to_string();
                let reply = json!({
                    "type": "result",
                    "payload": {
                        "requestId": request_id,
                        "ok": false,
                        "error": "设备 RD-100 当前未连接（状态：离线），指令未送达",
                    },
                });
                socket
                    .send(Message::Text(reply.to_string()))
                    .await
                    .expect("send result");
                return;
            }
            panic!("没有收到控制请求");
        });

        let error = send_device_command_handler(
            axum::extract::State(state.clone()),
            axum::Extension(admin()),
            AxumPath(IdParam { id: device_id }),
            Json(command_request("召回充电", "req-ws-reject")),
        )
        .await
        .expect_err("rejection must fail the request");

        business.await.expect("business side must finish");
        assert!(matches!(error, AppError::BadRequest(message) if message.contains("未送达")));

        // 被拒绝的指令必须记成 failed，不能显示「已下发」
        let audit = state
            .dog_device_service
            .find_command("req-ws-reject")
            .await
            .expect("audit lookup")
            .expect("audit row");
        assert_eq!(audit.status, audit::STATUS_FAILED);
    }

    #[tokio::test]
    async fn sync_maps_is_delivered_over_the_control_channel() {
        let state = state_without_http().await;
        let device_id = create_device(&state, "RD-100").await;
        let url = spawn_zentai(state.clone()).await;
        let mut socket = dial(&url, TOKEN).await;
        wait_online(&state).await;

        let business = tokio::spawn(async move {
            while let Some(message) = socket.next().await {
                let Ok(Message::Text(text)) = message else {
                    continue;
                };
                let frame: Value = serde_json::from_str(&text).expect("frame is json");
                if frame["type"] != "request" {
                    continue;
                }
                // 内网部署时「同步地图」也必须走信道，否则这个按钮会静默失效
                assert_eq!(frame["payload"]["kind"], "sync-maps");
                assert_eq!(frame["payload"]["body"]["mode"], "manual");
                let request_id = frame["payload"]["requestId"].as_str().unwrap().to_string();
                let reply = json!({
                    "type": "result",
                    "payload": {
                        "requestId": request_id,
                        "ok": true,
                        "data": {"deviceId": "RD-100", "dispatched": true},
                    },
                });
                socket
                    .send(Message::Text(reply.to_string()))
                    .await
                    .expect("send result");
                return;
            }
            panic!("没有收到同步地图请求");
        });

        let data = sync_device_maps_handler(
            axum::extract::State(state.clone()),
            axum::Extension(admin()),
            AxumPath(IdParam { id: device_id }),
        )
        .await
        .expect("sync maps over channel must succeed")
        .0
        .data;

        business.await.expect("business side must finish");
        assert_eq!(data["dispatched"], true);
    }

    #[tokio::test]
    async fn control_channel_requires_service_token() {
        let state = state_without_http().await;
        let url = spawn_zentai(state).await;

        let mut request = url.into_client_request().expect("build ws request");
        request
            .headers_mut()
            .insert("x-service-token", HeaderValue::from_static("wrong-token"));
        let result = tokio_tungstenite::connect_async(request).await;
        assert!(result.is_err(), "错误令牌不应建立控制信道");
    }
}
