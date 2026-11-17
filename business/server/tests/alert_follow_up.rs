//! 告警「跟单补推」回归（2026-11-17 现场）。
//!
//! 现场：同一航点配了「检查 3 个」，第一轮报「实际检出 1 个」→ 建单 + 推群；
//! 下一轮报「实际检出 2 个」→ 结构化去重把它当成同一件事挂回原单，
//! `notify::alert_created` 按「同类在办工单」直接 `return`，**群里什么都没有**，
//! 现场因此以为「数量核对没有生效」。
//!
//! 现在的规则：跟单默认静默，但**数量核对的检出数变了要补推一条**（不另外建单）。
//! 这个用例钉住 `alert_flow::on_alert` 返回的 `repeated` / `follow_up` 两个标志。

use robodog_ops_server::alert_flow::CountChange;
use robodog_ops_server::app::AppState;
use robodog_ops_server::config::Config;
use robodog_ops_server::{alert_flow, db, gateway, seed};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct TempDb {
    dir: PathBuf,
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn test_state() -> (AppState, TempDb) {
    let dir = std::env::temp_dir().join(format!(
        "robodog-ops-follow-up-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    let temp = TempDb { dir: dir.clone() };
    let config = Arc::new(Config {
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
    };
    (state, temp)
}

/// 插一条「AI 数量核对不一致」告警，带上结构化去重键，返回 alert_code
async fn insert_count_alert(state: &AppState, code: &str, found: i64) -> String {
    let device: (i64,) = sqlx::query_as("SELECT id FROM devices LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("播种数据里应至少有一台设备");
    sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, device_id, status, occurred_at,
            created_at, dedup_key)
         VALUES (?1, '警告', ?2, ?3, '未处理', '2026-11-17 20:15:42', '2026-11-17 20:15:42', ?4)",
    )
    .bind(code)
    .bind(format!(
        "AI 识别数量核对不一致：任务「办公区域例行巡检」航点 0「新航点 1」检查 3 个，实际检出 {found} 个"
    ))
    .bind(device.0)
    .bind("ai:RD-1:TK-1112-11:0:count")
    .execute(&state.db)
    .await
    .expect("插入告警失败");
    code.to_string()
}

async fn open_order_count(state: &AppState) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM work_orders WHERE source = '告警自动派单'")
        .fetch_one(&state.db)
        .await
        .expect("查工单失败")
}

#[tokio::test]
async fn count_change_attaches_but_flags_a_follow_up() {
    let (state, _temp) = test_state().await;

    // ① 第一轮：检出 1 个 → 新建工单并正常推群
    let first_code = insert_count_alert(&state, "AL-FU-1", 1).await;
    let first = alert_flow::on_alert(&state, &first_code)
        .await
        .expect("第一次告警应有派单结果");
    assert!(!first.repeated, "第一次不是跟单");
    assert!(first.follow_up.is_none(), "第一次不需要补推");
    let order_id = first.work_order_id.expect("第一次应自动建单");
    let order_code = first.work_order_code.clone().expect("应有工单号");
    assert_eq!(open_order_count(&state).await, 1);

    // ② 第二轮：检出 2 个 → 挂回原单，但要补推一条「跟单」
    let second_code = insert_count_alert(&state, "AL-FU-2", 2).await;
    let second = alert_flow::on_alert(&state, &second_code)
        .await
        .expect("第二次告警也应有派单结果");
    assert!(second.repeated, "第二次是跟单（不新建单）");
    assert_eq!(second.work_order_id, Some(order_id), "必须挂回原工单");
    assert_eq!(second.work_order_code.as_deref(), Some(order_code.as_str()));
    assert_eq!(
        second.follow_up,
        Some(CountChange {
            previous: 1,
            current: 2
        }),
        "检出数从 1 变 2，要补推"
    );
    assert_eq!(open_order_count(&state).await, 1, "不能再建一张单");

    // ③ 第三轮：还是 2 个 → 继续静默（每轮巡检都会报一遍，不能刷屏）
    let third_code = insert_count_alert(&state, "AL-FU-3", 2).await;
    let third = alert_flow::on_alert(&state, &third_code)
        .await
        .expect("第三次告警也应有派单结果");
    assert!(third.repeated);
    assert!(third.follow_up.is_none(), "数值没变就不该再推");
    assert_eq!(open_order_count(&state).await, 1);

    // ④ 第四轮：变成 3 个（符合要求）→ 不再有告警；但若再来 4 个仍算「变化」
    let fourth_code = insert_count_alert(&state, "AL-FU-4", 4).await;
    let fourth = alert_flow::on_alert(&state, &fourth_code)
        .await
        .expect("第四次告警也应有派单结果");
    assert_eq!(
        fourth.follow_up,
        Some(CountChange {
            previous: 2,
            current: 4
        })
    );
    assert_eq!(open_order_count(&state).await, 1);
}

/// 普通检出（带置信度）跟单不补推：置信度在 0.8~0.95 之间抖，没有信息量。
#[tokio::test]
async fn detection_confidence_does_not_trigger_follow_up() {
    let (state, _temp) = test_state().await;
    let device: (i64,) = sqlx::query_as("SELECT id FROM devices LIMIT 1")
        .fetch_one(&state.db)
        .await
        .expect("播种数据里应至少有一台设备");
    for (code, conf) in [("AL-DET-1", "0.81"), ("AL-DET-2", "0.93")] {
        sqlx::query(
            "INSERT INTO alerts (alert_code, level, content, device_id, status, occurred_at,
                created_at, dedup_key)
             VALUES (?1, '警告', ?2, ?3, '未处理', '2026-11-17 20:15:42', '2026-11-17 20:15:42', ?4)",
        )
        .bind(code)
        .bind(format!(
            "AI 识别：任务「办公区域例行巡检」航点 0「新航点 1」检出 extinguisher（置信度 {conf}）"
        ))
        .bind(device.0)
        .bind("ai:RD-1:TK-1112-11:0:extinguisher")
        .execute(&state.db)
        .await
        .expect("插入告警失败");
    }
    let first = alert_flow::on_alert(&state, "AL-DET-1").await.expect("应派单");
    assert!(!first.repeated);
    let second = alert_flow::on_alert(&state, "AL-DET-2").await.expect("应派单");
    assert!(second.repeated, "第二次同类检出是跟单");
    assert!(
        second.follow_up.is_none(),
        "置信度变化不该补推: {:?}",
        second.follow_up
    );
    assert_eq!(open_order_count(&state).await, 1);
}
