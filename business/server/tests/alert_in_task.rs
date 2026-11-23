//! 「只有执行计划期间的告警才建单 + 推群」回归（`settings.alert_only_in_task`）。
//!
//! 现场口径：巡检/任务在跑的时候本体报的障碍、故障、AI 检出才值得派人处理，也才该进
//! 企业微信群；掉线、遥控器抢占、页面手动新建的告警**只落库**（告警中心照常能看），
//! 不再自动派生工单、也不再往群里刷屏。
//!
//! 判据是 `alert_flow::in_task_execution`：告警绑定的设备当时有 `执行中` 的任务。
//! 这些用例同时钉住开关打开（默认）与关掉（老口径）两侧。

use robodog_ops_server::alert_flow;
use robodog_ops_server::app::AppState;
use robodog_ops_server::config::Config;
use robodog_ops_server::notify::Notice;
use robodog_ops_server::notify::Notifier;
use robodog_ops_server::{db, gateway, seed};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct TempDb {
    dir: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-{tag}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

fn test_config(dir: &Path) -> Config {
    Config {
        port: 0,
        database_path: dir.join("test.db"),
        jwt_secret: "test-secret-at-least-32-bytes-long!!".to_string(),
        jwt_ttl_hours: 1,
        web_dist: dir.join("web-dist"),
        backend_db_path: dir.join("backend.db"),
        zentai_base_url: None,
        zentai_username: None,
        zentai_password: None,
        ingest_dir: dir.join("map_ingest"),
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
        wecom_enabled: true,
        wecom_webhook: Some(
            "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=test-key".to_string(),
        ),
        wecom_min_level: "警告".to_string(),
        wecom_mention_all: false,
    }
}

/// 有 webhook 但**不启动消费端**：只看「入没入队」，不发外网。
async fn build_state() -> (AppState, mpsc::Receiver<Notice>, TempDb) {
    let dir = temp_dir("alert-in-task");
    let temp = TempDb { dir: dir.clone() };
    let config = Arc::new(test_config(&dir));
    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
    seed::seed(&pool).await.expect("播种失败");
    let (notifier, rx) = Notifier::channel(&config, None);
    let state = AppState {
        db: pool,
        notifier,
        config,
        device_connections: gateway::new_device_connections(),
        command_results: gateway::new_command_result_waiters(),
        video_hub: Default::default(),
        agent_tunnel: Default::default(),
        ai_patrol: Default::default(),
        caches: Default::default(),
    };
    (state, rx.expect("部署开关开着时应有消费端"), temp)
}

/// 一条 `执行中` 任务所属的设备（= 正在执行计划）。
async fn device_in_task(state: &AppState) -> i64 {
    sqlx::query_scalar(
        "SELECT device_id FROM tasks
          WHERE task_status = '执行中' AND device_id IS NOT NULL
          ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&state.db)
    .await
    .expect("播种数据里应至少有一条执行中的任务")
}

/// 一台**没有**在执行计划的设备（掉线 / 抢占这类告警的来源）。
async fn device_idle(state: &AppState) -> i64 {
    sqlx::query_scalar(
        "SELECT id FROM devices
          WHERE id NOT IN (SELECT device_id FROM tasks
                            WHERE task_status = '执行中' AND device_id IS NOT NULL)
          ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&state.db)
    .await
    .expect("播种数据里应至少有一台空闲设备")
}

async fn insert_alert(state: &AppState, code: &str, device: Option<i64>, dedup: &str) {
    sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, device_id, status, occurred_at,
            created_at, dedup_key)
         VALUES (?1, '警告', ?2, ?3, '未处理', '2026-11-18 09:12:00', '2026-11-18 09:12:00', ?4)",
    )
    .bind(code)
    .bind(format!("测试告警 {code}"))
    .bind(device)
    .bind(dedup)
    .execute(&state.db)
    .await
    .expect("插入告警失败");
}

async fn auto_order_count(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM work_orders WHERE source = '告警自动派单'")
        .fetch_one(&state.db)
        .await
        .expect("查工单失败")
}

async fn alert_exists(state: &AppState, code: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM alerts WHERE alert_code = ?1")
        .bind(code)
        .fetch_one(&state.db)
        .await
        .expect("查告警失败")
        == 1
}

/// 掉线（`hb:`）这类「没人执行计划时报的告警」：只落库，不建单、不推群。
#[tokio::test]
async fn alert_outside_task_creates_no_order_and_no_push() {
    let (state, mut rx, _temp) = build_state().await;
    let device = device_idle(&state).await;
    insert_alert(&state, "AL-HB-1", Some(device), "hb:1").await;

    assert!(
        !alert_flow::in_task_execution(&state.db, "AL-HB-1").await,
        "空闲设备上的告警不算执行计划期间的告警"
    );
    robodog_ops_server::notify::alert_created(&state.notify(), "AL-HB-1").await;

    assert_eq!(auto_order_count(&state).await, 0, "不该自动建工单");
    assert!(rx.try_recv().is_err(), "不该推企业微信");
    // 告警本身照常落库：告警中心还能看到，人工可以自己决定要不要派单
    assert!(alert_exists(&state, "AL-HB-1").await);
    // 也没有派单信息（不找人、不回写 assignee）
    let assignee: Option<String> =
        sqlx::query_scalar("SELECT assignee FROM alerts WHERE alert_code = 'AL-HB-1'")
            .fetch_one(&state.db)
            .await
            .expect("查 assignee 失败");
    assert!(assignee.is_none(), "不该给告警派责任人: {assignee:?}");
}

/// 执行计划期间的告警照旧：建一张单、推一条群消息。
#[tokio::test]
async fn alert_during_task_creates_order_and_pushes() {
    let (state, mut rx, _temp) = build_state().await;
    let device = device_in_task(&state).await;
    insert_alert(&state, "AL-TASK-1", Some(device), "fault:1:12608").await;

    assert!(alert_flow::in_task_execution(&state.db, "AL-TASK-1").await);
    robodog_ops_server::notify::alert_created(&state.notify(), "AL-TASK-1").await;

    assert_eq!(auto_order_count(&state).await, 1, "执行计划期间应自动建单");
    let notice = rx.try_recv().expect("执行计划期间应推企业微信");
    assert!(
        notice.title.contains("已自动派单"),
        "消息应带派单结论: {}",
        notice.title
    );
    let order: Option<i64> =
        sqlx::query_scalar("SELECT work_order_id FROM alerts WHERE alert_code = 'AL-TASK-1'")
            .fetch_one(&state.db)
            .await
            .expect("查工单关联失败");
    assert!(order.is_some(), "告警应回写 work_order_id");
}

/// 未绑定设备的告警（人工记一笔）同样不建单不推群：连不上任何计划。
#[tokio::test]
async fn unbound_alert_is_skipped() {
    let (state, mut rx, _temp) = build_state().await;
    insert_alert(&state, "AL-NONE-1", None, "").await;

    assert!(!alert_flow::in_task_execution(&state.db, "AL-NONE-1").await);
    robodog_ops_server::notify::alert_created(&state.notify(), "AL-NONE-1").await;

    assert_eq!(auto_order_count(&state).await, 0);
    assert!(rx.try_recv().is_err());
    assert!(alert_exists(&state, "AL-NONE-1").await);
}

/// 开关关掉 = 老口径：所有告警都建单 + 推群。
#[tokio::test]
async fn switch_off_restores_dispatch_for_every_alert() {
    let (state, mut rx, _temp) = build_state().await;
    sqlx::query(
        "UPDATE settings SET setting_value = 'false' WHERE setting_key = 'alert_only_in_task'",
    )
    .execute(&state.db)
    .await
    .expect("关掉开关失败");

    let device = device_idle(&state).await;
    insert_alert(&state, "AL-HB-2", Some(device), "hb:2").await;
    assert!(
        alert_flow::should_dispatch(&state.db, "AL-HB-2").await,
        "关掉开关后不再按「执行计划期间」过滤"
    );

    robodog_ops_server::notify::alert_created(&state.notify(), "AL-HB-2").await;
    assert_eq!(auto_order_count(&state).await, 1, "老口径下应建单");
    assert!(rx.try_recv().is_ok(), "老口径下应推企业微信");
}
