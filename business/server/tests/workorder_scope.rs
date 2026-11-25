//! 工单可见性回归：**系统自动派单的工单不能被「只看自己建的」挡在门外**。
//!
//! 2026-11-17 现场：AI 识别告警确实生成了 `WO-1117-29`（`alerts.work_order_id=29`、
//! 派给张泽新、状态「待接单」），但运营账号 `zhangzexin`（中台 role=common，
//! 落在业务平台里 `is_admin=false`）打开「工单管理」是一片空白 —— 告警中心看得到告警，
//! 工单却全被 `created_by = 当前用户` 这一行过滤掉（自动派单的 `created_by` 为空，
//! 派单人写的是「系统」）。
//!
//! 现在非管理员看到：`自己建的 单` + `系统自动派单的单`；别人手工建的单仍然隔离。

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use jsonwebtoken::{EncodingKey, Header, encode};
use robodog_ops_server::app::{AppState, build_router};
use robodog_ops_server::auth::Claims;
use robodog_ops_server::config::Config;
use robodog_ops_server::{db, gateway, seed};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct TempDb {
    dir: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const JWT_SECRET: &str = "test-secret-at-least-32-bytes-long!!";

async fn test_state() -> (AppState, TempDb) {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-workorder-scope-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    let temp = TempDb { dir: dir.clone() };
    let config = Arc::new(Config {
        port: 0,
        database_path: dir.join("test.db"),
        jwt_secret: JWT_SECRET.to_string(),
        jwt_ttl_hours: 1,
        web_dist: dir.join("web-dist"),
        backend_db_path: dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: dir.join("map_ingest"),
        recording_dir: dir.join("recordings"),
        photo_dir: dir.join("photos"),
        recording_keep_days: 30,
        photo_keep_days: 90,
        service_token: None,
        zentai_ws_url: None,
        device_timeout_secs: 90,
        task_timeout_secs: 120,
        task_stuck_secs: 600,
        backpack_base_url: None,
        backpack_webrtc_url: None,
        backpack_browser_url: None,
        mediamtx_url: None,
        backpack_tunnel_device: None,
        point_ai: "auto".to_string(),
        ai_window_max_secs: 300,
        public_url: None,
        wecom_enabled: false,
        wecom_webhook: None,
        wecom_min_level: "警告".to_string(),
        wecom_mention_all: false,
    });
    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");
    let state = AppState {
        db: pool,
        notifier: robodog_ops_server::notify::Notifier::disabled(),
        config,
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
        phone_audio: Default::default(),
        ai_patrol: Default::default(),
        caches: Default::default(),
    };
    (state, temp)
}

/// 插一张工单（`created_by` 为 `None` = 系统自动派单，与 `alert_flow::create_work_order` 一致）
/// 返回自增 id
async fn insert_order(state: &AppState, code: &str, title: &str, created_by: Option<i64>) -> i64 {
    sqlx::query(
        "INSERT INTO work_orders (order_code, title, order_type, source, priority, assignee,
            status, progress, created_at, updated_at, created_by)
         VALUES (?1, ?2, '告警处置', '告警自动派单', '高', '张泽新', '待接单', 0,
            '2026-11-17 19:19:09', '2026-11-17 19:19:09', ?3)",
    )
    .bind(code)
    .bind(title)
    .bind(created_by)
    .execute(&state.db)
    .await
    .expect("插入工单失败");
    sqlx::query_scalar("SELECT id FROM work_orders WHERE order_code = ?1")
        .bind(code)
        .fetch_one(&state.db)
        .await
        .expect("读工单 id 失败")
}

/// 现场账号（中台 role=common → 业务平台 `is_admin=false`，`zentaiUserId` 非空）
fn operator_token() -> String {
    let claims = Claims {
        sub: 2,
        username: "zhangzexin".to_string(),
        role: "operator".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize,
        zentai_user_id: Some(1794470856325),
        zentai_username: Some("zhangzexin".to_string()),
        is_admin: false,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("签发测试令牌失败")
}

async fn list_titles(app: &axum::Router, token: &str) -> Vec<String> {
    let request = Request::get("/api/workorders")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "查工单列表失败");
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    body["data"]
        .as_array()
        .expect("工单列表应是数组")
        .iter()
        .filter_map(|item| item["title"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn system_dispatched_orders_are_visible_to_operator() {
    let (state, _temp) = test_state().await;
    // 播种只建了 admin（id=1）；建一个运营账号（id=2）当「现场登录的人」
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, display_name, role, created_at)
         VALUES (2, 'zhangzexin', 'x', '张泽新', 'operator', '2026-11-17 09:00:00')",
    )
    .execute(&state.db)
    .await
    .expect("插入运营账号失败");
    let auto_id = insert_order(
        &state,
        "WO-SCOPE-AUTO",
        "【自动派单】AI 识别：电量检查",
        None,
    )
    .await;
    insert_order(&state, "WO-SCOPE-MINE", "【自己建】手工巡检复核", Some(2)).await;
    let other_id = insert_order(&state, "WO-SCOPE-OTHER", "【别人建】管理员的单", Some(1)).await;

    let app = build_router(state.clone());
    let titles = list_titles(&app, &operator_token()).await;
    assert!(
        titles.iter().any(|title| title.contains("自动派单")),
        "系统自动派单的工单必须对现场账号可见（现场就是「告警有了、工单管理空白」）: {titles:?}"
    );
    assert!(
        titles.iter().any(|title| title.contains("自己建")),
        "自己建的工单当然要看得见: {titles:?}"
    );
    assert!(
        !titles.iter().any(|title| title.contains("别人建")),
        "别人手工建的单仍然隔离: {titles:?}"
    );

    // 详情 / 处置：系统派单必须能打开、能接单（现场「列表能看到、点开说工单不存在」）
    let token = operator_token();
    assert_eq!(
        status_of(&app, &token, "GET", &format!("/api/workorders/{auto_id}")).await,
        StatusCode::OK,
        "系统派单的详情必须能打开"
    );

    // 详情要带出「关联告警」：同类告警都挂在这张单上，现场的历次结论/证据路径靠它回看
    sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, status, occurred_at, created_at,
            work_order_id, dedup_key)
         VALUES ('AL-WO-1', '警告',
            'AI 识别数量核对不一致：…检查 3 个，实际检出 2 个（证据：/x/dogcam_1.jpg）',
            '未处理', '2026-11-17 20:15:42', '2026-11-17 20:15:42', ?1,
            'ai:RD-AI-1:TK-1:0:count')",
    )
    .bind(auto_id)
    .execute(&state.db)
    .await
    .expect("插入关联告警失败");
    let request = Request::get(format!("/api/workorders/{auto_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(
            app.clone().oneshot(request).await.unwrap().into_body(),
            1024 * 1024,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let alerts = body["data"]["alerts"]
        .as_array()
        .unwrap_or_else(|| panic!("详情应带关联告警: {body}"));
    assert_eq!(alerts.len(), 1, "应回 1 条关联告警: {alerts:?}");
    assert_eq!(alerts[0]["alertCode"], "AL-WO-1");
    assert!(
        alerts[0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("证据：/x/dogcam_1.jpg"),
        "告警正文要能指到证据图: {alerts:?}"
    );
    assert_eq!(
        status_of(&app, &token, "GET", &format!("/api/workorders/{other_id}")).await,
        StatusCode::NOT_FOUND,
        "别人手工建的单仍然看不到"
    );
    let request = Request::put(format!("/api/workorders/{auto_id}/status"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"status":"处理中"}"#))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "被派单的人要能接单（状态流转）"
    );

    // 统计卡片同样算上系统派单（否则看板顶部永远是 0）
    let request = Request::get("/api/workorders/stats")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        body["data"]["total"].as_i64().unwrap_or(0) >= 2,
        "统计应包含系统派单 + 自己建的单: {body}"
    );

    // 删除仍然只给管理员：系统单是告警的存证链
    assert_eq!(
        status_of(
            &app,
            &token,
            "DELETE",
            &format!("/api/workorders/{auto_id}")
        )
        .await,
        StatusCode::NOT_FOUND,
        "非管理员不能删系统派单"
    );
}

/// 只关心状态码的简易请求
async fn status_of(app: &axum::Router, token: &str, method: &str, path: &str) -> StatusCode {
    let builder = match method {
        "GET" => Request::get(path),
        "DELETE" => Request::delete(path),
        other => panic!("不支持的方法 {other}"),
    };
    let request = builder
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

/// 责任人绑定了平台账号之后：
/// - 派给他的系统单只有他（+管理员）能看能处置；
/// - 派给「没绑账号的责任人」的系统单仍是公共待办（现场外包 / 临时工没有账号）；
/// - `?mine=true` 只返回派给自己的单；
/// - 账号没绑责任人时保持老口径（自己建的 + 全部系统派单），不会一升级就空白。
#[tokio::test]
async fn bound_assignee_scopes_orders_and_actions() {
    let (state, _temp) = test_state().await;
    for (id, username) in [(2_i64, "zhangzexin"), (3, "qxs")] {
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, display_name, role, created_at)
             VALUES (?1, ?2, 'x', ?2, 'operator', '2026-11-17 09:00:00')",
        )
        .bind(id)
        .bind(username)
        .execute(&state.db)
        .await
        .expect("插入账号失败");
    }
    // 责任人：8 绑 zhangzexin、9 绑 qxs、10 没绑账号
    for (id, code, name, user) in [
        (8_i64, "EMP-08", "张泽新", Some(2_i64)),
        (9, "EMP-09", "钱小帅", Some(3)),
        (10, "EMP-10", "临时工", None),
    ] {
        sqlx::query(
            "INSERT INTO staff (id, staff_code, name, role, team, phone, color, status,
                user_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, '巡检', '巡检一班', '', '#0284c7', '在岗', ?4,
                '2026-11-17 09:00:00', '2026-11-17 09:00:00')",
        )
        .bind(id)
        .bind(code)
        .bind(name)
        .bind(user)
        .execute(&state.db)
        .await
        .expect("插入责任人失败");
    }

    let mine_id = insert_order(&state, "WO-B-MINE", "【派给我】AI 识别：灭火器", None).await;
    let other_id = insert_order(&state, "WO-B-OTHER", "【派给别人】导航指令非法", None).await;
    let pool_id = insert_order(&state, "WO-B-POOL", "【未绑账号】感知延迟", None).await;
    for (id, staff_id) in [(mine_id, 8_i64), (other_id, 9), (pool_id, 10)] {
        sqlx::query("UPDATE work_orders SET staff_id = ?1 WHERE id = ?2")
            .bind(staff_id)
            .bind(id)
            .execute(&state.db)
            .await
            .expect("回填责任人失败");
    }

    let app = build_router(state.clone());
    let token = operator_token();
    let titles = list_titles(&app, &token).await;
    assert!(
        titles.iter().any(|title| title.contains("派给我")),
        "派给自己的单要看得见: {titles:?}"
    );
    assert!(
        titles.iter().any(|title| title.contains("未绑账号")),
        "派给没账号的责任人 = 公共待办，仍要看得见: {titles:?}"
    );
    assert!(
        !titles.iter().any(|title| title.contains("派给别人")),
        "派给已绑定别人的责任人的单不该出现在我这里: {titles:?}"
    );

    // ?mine=true：只要派给自己的
    let request = Request::get("/api/workorders?mine=true")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let mine: Vec<&str> = body["data"]
        .as_array()
        .expect("应是数组")
        .iter()
        .filter_map(|item| item["title"].as_str())
        .collect();
    assert_eq!(mine.len(), 1, "只看我负责的应只有一条: {mine:?}");
    assert!(mine[0].contains("派给我"), "{mine:?}");

    // 处置权限：自己的可以、别人的不行、公共待办可以
    assert_eq!(
        status_of(&app, &token, "GET", &format!("/api/workorders/{mine_id}")).await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(&app, &token, "GET", &format!("/api/workorders/{other_id}")).await,
        StatusCode::NOT_FOUND,
        "别人的单点开应 404"
    );
    let request = Request::put(format!("/api/workorders/{other_id}/status"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"status":"处理中"}"#))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(request).await.unwrap().status(),
        StatusCode::NOT_FOUND,
        "不能处置别人的单"
    );
    assert_eq!(
        status_of(&app, &token, "GET", &format!("/api/workorders/{pool_id}")).await,
        StatusCode::OK,
        "没绑账号的责任人的单是公共待办"
    );
}

/// 一个账号只能绑一个责任人（否则「我的工单」不唯一）；换绑/解绑要走得通。
#[tokio::test]
async fn one_account_binds_one_staff() {
    let (state, _temp) = test_state().await;
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, display_name, role, created_at)
         VALUES (2, 'zhangzexin', 'x', '张泽新', 'operator', '2026-11-17 09:00:00')",
    )
    .execute(&state.db)
    .await
    .expect("插入账号失败");

    let app = build_router(state.clone());
    let token = operator_token();
    // 新建绑定用户 2 的责任人
    let request = Request::post("/api/staff")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"name":"张泽新","team":"巡检一班","userId":2}"#,
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "首次绑定应成功");
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let first_id = body["data"]["id"].as_i64().expect("应返回责任人 id");
    assert_eq!(body["data"]["userId"], 2, "接口要回显绑定关系");
    assert_eq!(body["data"]["userName"], "zhangzexin");

    // 再绑一个：应当被拒（400 + 能看懂的原因）
    let request = Request::post("/api/staff")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"另一个人","userId":2}"#))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "同一账号不能绑两个责任人"
    );

    // 解绑：传 userId = null
    let request = Request::put(format!("/api/staff/{first_id}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            r#"{"name":"张泽新","team":"巡检一班","userId":null}"#,
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "解绑应成功");
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(
        body["data"]["userId"].is_null(),
        "解绑后 user_id 应为空: {body}"
    );
}
