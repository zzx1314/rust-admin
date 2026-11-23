//! 告警证据图 → 企业微信图片消息（`msgtype=image`）回归。
//!
//! 背景：板端 AI 识别的命中帧落在板端磁盘（`detect_jobs/<plan>/pt<n>/*.jpg`），
//! 平台读不到；企业微信的图片消息要的是**图片字节**（base64 + 原图 md5），
//! 不是路径。所以由设备客户端把图读出来随 `ai_alert` 上报，服务端校验后作为
//! **markdown 卡片之后的第二条消息**发出（群里先看结论再看图）。
//!
//! 这些用例钉住三件事：
//! 1. 有图 → 入队的 `Notice` 带 `image`（base64/md5 正确）；
//! 2. 开关 `wecom_alert_image = false` → 只发卡片；
//! 3. 图不合法（非图片/超 2MB）→ 校验层拒绝，卡片照发（告警不能因为图丢掉）。

use robodog_ops_server::alert_flow;
use robodog_ops_server::app::AppState;
use robodog_ops_server::config::Config;
use robodog_ops_server::notify::Notifier;
use robodog_ops_server::notify::{self, Notice};
use robodog_ops_server::{db, gateway, seed};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// 最小 JPEG（只看文件头 `FF D8 FF`）：md5 = cbde0fdf7fab6fefd9fe12fec4967cf9
const JPEG_BASE64: &str = "/9j/4AAQSkZJRgAB";
const JPEG_MD5: &str = "cbde0fdf7fab6fefd9fe12fec4967cf9";

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

/// 有 webhook 但不启动消费端：只看「入队的这条消息长什么样」，不碰外网。
async fn build_state() -> (AppState, mpsc::Receiver<Notice>, TempDb) {
    let dir = temp_dir("alert-image");
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
    };
    (state, rx.expect("部署开关开着时应有消费端"), temp)
}

/// 一条 `执行中` 任务所属的设备（新口径下只有执行计划期间的告警会推群）。
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

/// 插一条「AI 数量核对不一致」告警（执行计划期间），返回 alert_code
async fn insert_count_alert(state: &AppState, code: &str) -> String {
    let device = device_in_task(state).await;
    sqlx::query(
        "INSERT INTO alerts (alert_code, level, content, device_id, status, occurred_at,
            created_at, dedup_key)
         VALUES (?1, '警告', ?2, ?3, '未处理', '2026-11-18 09:20:00', '2026-11-18 09:20:00', ?4)",
    )
    .bind(code)
    .bind("AI 识别数量核对不一致：任务「A区上午例行巡检」航点「新航点 1」检查 3 个，实际检出 1 个")
    .bind(device)
    .bind(format!("ai:{device}:TK-1:0:count"))
    .execute(&state.db)
    .await
    .expect("插入告警失败");
    code.to_string()
}

#[tokio::test]
async fn alert_with_evidence_image_queues_an_image_message() {
    let (state, mut rx, _temp) = build_state().await;
    let code = insert_count_alert(&state, "AL-IMG-1").await;
    let image = notify::evidence_image(JPEG_BASE64).expect("样本图应通过校验");

    notify::alert_created_with_image(&state.notify(), &code, Some(image)).await;

    let notice = rx.try_recv().expect("卡片应先入队");
    assert!(
        notice.title.contains("已自动派单"),
        "执行计划期间的告警应自动建单: {}",
        notice.title
    );
    let image = notice.image.expect("证据图应随告警一起入队");
    assert_eq!(image.base64, JPEG_BASE64, "base64 不能带换行/空格");
    assert_eq!(image.md5, JPEG_MD5);
    assert_eq!(image.bytes, 12);
    // 图进了群，板端那一轮存证（画框 JPG + result.json）就没有第二次用途了：
    // 推送成功后下发 `ai_cleanup` 让客户端删（计划 / 航点从结构化键反推）
    let cleanup = notice.cleanup.as_ref().expect("带图的 AI 告警要带清理目标");
    assert_eq!(cleanup.plan_id, "TK-1", "计划 id 来自去重键的第二段");
    assert_eq!(cleanup.point_index, 0, "航点下标来自去重键的第三段");
    assert!(!cleanup.device_code.is_empty(), "要能定位到哪台设备的盘");
    // 只有一条 Note：图片是消费端在正文之后补发的第二条 HTTP 消息
    assert!(rx.try_recv().is_err());
}

/// 关掉开关就只发卡片（运维嫌图多时用）
#[tokio::test]
async fn image_can_be_switched_off() {
    let (state, mut rx, _temp) = build_state().await;
    sqlx::query(
        "UPDATE settings SET setting_value = 'false' WHERE setting_key = 'wecom_alert_image'",
    )
    .execute(&state.db)
    .await
    .expect("关掉证据图开关失败");

    let code = insert_count_alert(&state, "AL-IMG-2").await;
    let image = notify::evidence_image(JPEG_BASE64).expect("样本图应通过校验");
    notify::alert_created_with_image(&state.notify(), &code, Some(image)).await;

    let notice = rx.try_recv().expect("卡片仍应入队");
    assert!(notice.image.is_none(), "开关关掉后不该再带图");
    // 没带图就不删存证：图没进群，板端那几张图还能人工补推
    assert!(notice.cleanup.is_none(), "关掉证据图时不能删板端存证");
    assert!(alert_flow::should_dispatch(&state.db, &code).await);
}

/// 图不合法时不带图，但**告警本身照发**（不能因为一张图把告警丢了）
#[tokio::test]
async fn invalid_image_is_dropped_but_the_alert_still_goes_out() {
    let (state, mut rx, _temp) = build_state().await;
    let code = insert_count_alert(&state, "AL-IMG-3").await;

    // 客户端给的是坏图：校验层拒绝，调用方按 None 处理
    let image = notify::evidence_image("aGVsbG8=").ok();
    assert!(image.is_none(), "hello 不是图片，必须被拒");
    notify::alert_created_with_image(&state.notify(), &code, image).await;

    let notice = rx.try_recv().expect("卡片仍要入队");
    assert!(notice.image.is_none());
    assert!(
        notice.cleanup.is_none(),
        "图不合法（没进群）就不能删板端存证"
    );
    let orders: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM work_orders WHERE source = '告警自动派单'")
            .fetch_one(&state.db)
            .await
            .expect("查工单失败");
    assert_eq!(orders, 1, "图片坏了也不能影响自动建单");
}
