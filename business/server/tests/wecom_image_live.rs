//! **真实**企业微信群机器人联调：把「AI 数量核对不一致」卡片 + 真实抓拍图推到群里。
//!
//! 和 `alert_image.rs` 的分工：
//! - `alert_image.rs` 用最小 JPEG 钉住「入队的 Notice 带不带 image」，不碰外网；
//! - 本文件跑**真的发送链路**（`Notifier::enqueue` → 消费端 → `deliver` → `msgtype=image`），
//!   发到真实群，用来验证「图片消息企业微信收不收」——md5 算错、base64 带换行、
//!   图片超 2MB 这类问题只有真发一次才暴露。
//!
//! 默认 `#[ignore]`：会往外网发消息，`cargo test` 不该碰。手动跑法见文件末尾。

use robodog_ops_server::app::AppState;
use robodog_ops_server::config::Config;
use robodog_ops_server::notify::{self, Level, Notice, Notifier};
use robodog_ops_server::{db, gateway};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("robodog-wecom-live-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

fn test_config(dir: &Path, webhook: String) -> Config {
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
        wecom_webhook: Some(webhook),
        wecom_min_level: "警告".to_string(),
        wecom_mention_all: false,
    }
}

/// 真实发送：卡片 + 图片各一条消息。只看 DB 里的 sent / failed 与日志里的「证据图推送成功」。
///
/// 环境变量：
/// - `ROBODOG_LIVE_WEBHOOK`：群机器人地址（必填，**不要**写进仓库/脚本）
/// - `ROBODOG_LIVE_EVIDENCE`：证据图文件（板端 `detect_jobs/<plan>/pt<n>/*.jpg`）
/// - `ROBODOG_LIVE_BODY`：卡片正文（可选，默认复述现场那条数量告警）
#[tokio::test]
#[ignore = "会真的往企业微信群发消息，手动跑"]
async fn push_real_alert_card_with_evidence_image() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(
            "info,robodog_ops_server=debug",
        ))
        .try_init();

    let webhook = std::env::var("ROBODOG_LIVE_WEBHOOK").expect("必须设置 ROBODOG_LIVE_WEBHOOK");
    let evidence = std::env::var("ROBODOG_LIVE_EVIDENCE").expect("必须设置 ROBODOG_LIVE_EVIDENCE");
    let body = std::env::var("ROBODOG_LIVE_BODY").unwrap_or_else(|_| {
        "AI 识别数量核对不一致：任务「办公区域例行巡检」航点「新航点 1」\
         检查 5 个，实际检出 2 个（证据：/root/261113/backpack_dashboard/backend/\
         detect_jobs/TK-1112-11/pt0/dogcam_20261118_105828.jpg）"
            .to_string()
    });

    // ① 真读盘 + 真实校验（≤2MB、JPG/PNG、base64/md5 由 `evidence_image` 算）
    let raw = std::fs::read(&evidence).expect("证据图读不到");
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
    let image = notify::evidence_image(&encoded).expect("证据图没通过企业微信的硬约束");
    println!(
        "证据图 {}：{} 字节，md5={}",
        evidence, image.bytes, image.md5
    );

    // ② 装配 + 启动真实消费端（地址来自参数，不走设置表）
    let dir = temp_dir("image");
    let config = Arc::new(test_config(&dir, webhook));
    let pool = db::connect(&config).await.expect("连接测试库失败");
    db::migrate(&pool).await.expect("迁移失败");
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
    notify::spawn(state.notify(), rx.expect("部署开关开着时应有消费端"));

    // ③ 手工造等价于线上那条「执行计划期间的数量告警」的 Notice 并入队
    let notice = Notice {
        level: Level::Warning,
        ref_type: "alert",
        ref_id: None,
        dedupe_key: format!("live-image-test:{}", chrono::Local::now().timestamp()),
        dedupe_window: None,
        title: "设备异常告警（证据图联调，可忽略）".to_string(),
        fields: vec![
            ("设备", "0202222672400230".to_string()),
            ("园区", "金澳国际".to_string()),
            ("关联计划", "办公区域例行巡检".to_string()),
            (
                "当班责任人",
                "张泽新（巡检一班） · 早班 08:00 - 16:00（SH-014）".to_string(),
            ),
            ("工单", "WO-1117-35（原工单，未新建）".to_string()),
            ("时间", "2026-11-18 10:58:30".to_string()),
            ("告警码", "AL-541".to_string()),
            (
                "说明",
                "本轮为证据图链路联调：正文之后应紧跟一张真实抓拍，勿派单".to_string(),
            ),
        ],
        body: Some(body),
        action: Some(
            "若群里只看到卡片没有图，查服务端日志「证据图推送失败」的 errcode".to_string(),
        ),
        link: None,
        mention_all: false,
        mentions: Vec::new(),
        image: Some(image),
        image_only: false,
        cleanup: None,
        bypass_min_level: false,
    };
    assert!(state.notifier.enqueue(notice), "入队失败（队列满/未配置）");

    // ④ 等两张消息发完，看落库结果（图片失败只打日志、不影响 sent）
    tokio::time::sleep(Duration::from_secs(8)).await;
    let rows: Vec<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT status, attempts, last_error FROM notifications ORDER BY id DESC LIMIT 1",
    )
    .fetch_all(&state.db)
    .await
    .expect("查推送记录失败");
    for (status, attempts, last_error) in &rows {
        println!("notifications: status={status} attempts={attempts} last_error={last_error:?}");
    }
    assert_eq!(rows.len(), 1, "应有一条卡片推送记录");
    assert_eq!(rows[0].0, "sent", "卡片没发出去，先看 webhook 是否有效");
    println!("✅ 已发送：请到群里确认「卡片 + 图片」两条消息");
    let _ = std::fs::remove_dir_all(&dir);
}

// 用法（在本目录）：
//   ROBODOG_LIVE_WEBHOOK='https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=xxx' \
//   ROBODOG_LIVE_EVIDENCE=/tmp/dogcam_20261118_105828.jpg \
//   cargo test --test wecom_image_live -- --ignored --nocapture
