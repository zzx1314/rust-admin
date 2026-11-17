//! 告警「跟单」回归（2026-11-17 现场 + 「每次任务都要推」反馈）。
//!
//! 现场：同一航点配了「检查 3 个」，第一轮报「实际检出 1 个」→ 建单 + 推群；
//! 下一轮报「实际检出 2 个」→ 结构化去重把它当成同一件事挂回原单。
//!
//! 规则（现行）：
//! 1. **跟单默认照推**（`wecom_alert_repeat_push`，缺行 = 开）。一次任务执行就是
//!    一次告警，周期计划第二天再跑还是要在群里看到；只有 5 分钟窗口内的连刷会合并。
//! 2. 「数量核对」的检出数变了要明确写出「上次 1 个 → 本次 2 个」，否则现场以为没告警。
//! 3. 关掉开关就退回老口径（24 小时内同一问题只推第一条）。
//!
//! 这些用例同时钉住 `alert_flow::on_alert` 返回的 `repeated` / `follow_up` 两个标志
//! 和 `notify::alert_created` 的入队行为（用假 webhook 只验入队，不发外网）。

use robodog_ops_server::notify::Notice;
use robodog_ops_server::alert_flow::CountChange;
use robodog_ops_server::app::AppState;
use robodog_ops_server::config::Config;
use robodog_ops_server::notify::Notifier;
use robodog_ops_server::{alert_flow, db, gateway, seed};
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

async fn test_state() -> (AppState, TempDb) {
    let (state, _rx, temp) = build_state(None).await;
    (state, temp)
}

/// 装配一个「有地址但不会真发」的 `Notifier`：消费端不启动（不跑 `notify::spawn`），
/// 所以这里只验证「入没入队」，不会碰外网。
async fn test_state_with_webhook() -> (AppState, mpsc::Receiver<Notice>, TempDb) {
    const URL: &str = "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=test-key";
    let (state, rx, temp) = build_state(Some(URL)).await;
    (state, rx.expect("部署开关开着时应有消费端"), temp)
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

fn test_config(dir: &Path, webhook: Option<&str>) -> Config {
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
        backpack_tunnel_device: None,
        point_ai: "auto".to_string(),
        ai_window_max_secs: 300,
        public_url: None,
        wecom_enabled: webhook.is_some(),
        wecom_webhook: webhook.map(str::to_string),
        wecom_min_level: "警告".to_string(),
        wecom_mention_all: false,
    }
}

async fn build_state(webhook: Option<&str>) -> (AppState, Option<mpsc::Receiver<Notice>>, TempDb) {
    let dir = temp_dir("follow-up");
    let temp = TempDb { dir: dir.clone() };
    let config = Arc::new(test_config(&dir, webhook));
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
    };
    (state, rx, temp)
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

/// 插一条「AI 检出」告警（同航点同类别 = 同一个结构化去重键，跨轮次也会命中老单）
async fn insert_detect_alert(state: &AppState, code: &str, conf: &str) {
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
        "AI 识别：任务「办公区域例行巡检」航点 0「新航点 1」检出 extinguisher（置信度 {conf}）"
    ))
    .bind(device.0)
    .bind("ai:RD-1:TK-1112-11:0:extinguisher")
    .execute(&state.db)
    .await
    .expect("插入告警失败");
}

/// 现场要的：**每次任务执行都要在群里出现一条**。
///
/// 周期计划跑完会回到「待执行」重跑（`schedule::rearm_finished`），`task_code` 不变，
/// 所以第二天同一航点的去重键完全相同 —— 老口径直接 `return`，群里一片安静，
/// 现场以为「巡检没跑 / 没告警」。现在跟单消息默认照推。
#[tokio::test]
async fn repeated_alert_is_pushed_every_round() {
    let (state, mut rx, _temp) = test_state_with_webhook().await;

    // ① 第一轮：建单 + 推一条「已自动派单」
    insert_detect_alert(&state, "AL-RP-1", "0.81").await;
    robodog_ops_server::notify::alert_created(&state, "AL-RP-1").await;
    let first = rx.try_recv().expect("第一条应入队");
    assert!(
        first.title.contains("已自动派单"),
        "第一条应是正常派单消息: {}",
        first.title
    );

    // ② 第二轮（下一次任务执行）：同键挂回原单，但消息照样推
    insert_detect_alert(&state, "AL-RP-2", "0.88").await;
    robodog_ops_server::notify::alert_created(&state, "AL-RP-2").await;
    let second = rx.try_recv().expect("跟单消息也应入队（每次都推）");
    assert!(
        second.title.contains("同一问题第 2 次"),
        "跟单消息要说清是第几次: {}",
        second.title
    );
    assert!(
        second
            .fields
            .iter()
            .any(|(name, value)| *name == "工单" && value.contains("原工单，未新建")),
        "跟单消息要说清没新建单: {:?}",
        second.fields
    );
    assert!(
        second.mentions.is_empty(),
        "跟单不再 @ 人（第一条已经 @ 过）"
    );
    assert_eq!(open_order_count(&state).await, 1, "跟单不能新建工单");

    // ③ 第三轮：仍然推，且在 5 分钟去重窗口内共用同一个键（不刷屏）
    insert_detect_alert(&state, "AL-RP-3", "0.92").await;
    robodog_ops_server::notify::alert_created(&state, "AL-RP-3").await;
    assert!(
        rx.try_recv().is_err(),
        "同一问题的连刷仍受 5 分钟窗口保护"
    );
}

/// 关掉开关就退回老口径（24 小时内同一问题只推第一条），给嫌吵的部署留后路。
#[tokio::test]
async fn repeat_push_can_be_switched_off() {
    let (state, mut rx, _temp) = test_state_with_webhook().await;
    sqlx::query("UPDATE settings SET setting_value = 'false' WHERE setting_key = 'wecom_alert_repeat_push'")
        .execute(&state.db)
        .await
        .expect("关掉重复推送开关失败");

    insert_detect_alert(&state, "AL-OFF-1", "0.81").await;
    robodog_ops_server::notify::alert_created(&state, "AL-OFF-1").await;
    assert!(rx.try_recv().is_ok(), "第一条仍要推");

    insert_detect_alert(&state, "AL-OFF-2", "0.83").await;
    robodog_ops_server::notify::alert_created(&state, "AL-OFF-2").await;
    assert!(
        rx.try_recv().is_err(),
        "关掉开关后跟单不推（老口径）"
    );
    assert_eq!(open_order_count(&state).await, 1);
}
