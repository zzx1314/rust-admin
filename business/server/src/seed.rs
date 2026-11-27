use crate::db::{now, today};
use crate::geo::route_metrics;
use crate::notify;
use crate::video;
use bcrypt::{DEFAULT_COST, hash};
use chrono::{Datelike, Duration, Local};
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn seed(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    ensure_admin(pool).await?;
    ensure_settings(pool).await?;

    let park_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parks")
        .fetch_one(pool)
        .await?;
    if park_count == 0 {
        seed_business_data(pool).await?;
    }
    seed_maps(pool).await?;
    // 排班 / 工单 / 责任人库默认不播种：上线后由运营自己建人排班，
    // 只有显式设置 ROBODOG_SEED_DEMO=1 的本地 / 演示库才写入示例数据。
    if demo_seed_enabled() {
        seed_staff_schedules(pool).await?;
    }

    Ok(())
}

async fn ensure_admin(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE username = 'admin'")
        .fetch_optional(pool)
        .await?;
    if exists.is_none() {
        let password_hash = hash("admin123", DEFAULT_COST).unwrap_or_default();
        sqlx::query(
            "INSERT INTO users (username, password_hash, display_name, role, created_at)
             VALUES (?1, ?2, ?3, 'admin', ?4)",
        )
        .bind("admin")
        .bind(password_hash)
        .bind("系统管理员")
        .bind(now())
        .execute(pool)
        .await?;
        tracing::info!("seeded default admin account: admin / admin123");
    }
    Ok(())
}

async fn ensure_settings(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let settings = [
        (
            "platform",
            "platform_name",
            "机器狗巡检业务平台",
            "string",
            "平台名称",
        ),
        (
            "platform",
            "refresh_interval",
            "10",
            "number",
            "数据刷新频率（秒）",
        ),
        ("platform", "default_park_id", "1", "number", "默认地图园区"),
        (
            "platform",
            "battery_threshold",
            "20",
            "number",
            "低电量阈值（%）",
        ),
        (
            "platform",
            "task_timeout_minutes",
            "10",
            "number",
            "任务超时容忍（分钟）",
        ),
        (
            "platform",
            "coordinate_system",
            "WGS-84",
            "string",
            "坐标系统",
        ),
        (
            "notification",
            "alert_push",
            "true",
            "bool",
            "设备异常告警推送（站内）",
        ),
        (
            "notification",
            "wecom_alert_push",
            "true",
            "bool",
            "设备异常告警推送到企业微信群",
        ),
        (
            "notification",
            "wecom_alert_repeat_push",
            "true",
            "bool",
            "同一问题已有在办工单时，重复告警也推到企业微信群",
        ),
        (
            "notification",
            "wecom_alert_image",
            "true",
            "bool",
            "告警证据图（板端 AI 识别抓拍）作为图片消息推到企业微信群",
        ),
        (
            "notification",
            "alert_auto_workorder",
            "true",
            "bool",
            "告警按当班排班自动派单并生成工单",
        ),
        (
            "notification",
            "alert_only_in_task",
            "true",
            "bool",
            "只在执行计划期间为告警建单并推群（其余告警只落库）",
        ),
        // 告警白名单：命中的故障码**不落告警、不建工单、不推企业微信**。
        // 预置现场明确不关心的高频噪音：12608 RS/lidar 时间不同步（每 ~90s 一条）、
        // 13440 导航指令非法（一定跟着 13897 能力占用，处理根因那条即可）。
        // 留空 = 全部告警照旧；填了就以平台配置为准（60s 内生效）。
        (
            "notification",
            "alert_ignore_fault_codes",
            "12608,13440",
            "string",
            "告警白名单（忽略的故障码，逗号分隔）：命中的不落告警、不建工单、不推企业微信",
        ),
        (
            "notification",
            "task_timeout_notice",
            "true",
            "bool",
            "任务超时提醒（站内）",
        ),
        (
            "notification",
            "wecom_task_abnormal",
            "true",
            "bool",
            "任务执行异常推送到企业微信群",
        ),
        (
            "notification",
            "low_battery_recharge",
            "true",
            "bool",
            "低电量自动回充",
        ),
        (
            "notification",
            "daily_report_mail",
            "false",
            "bool",
            "巡检日报邮件",
        ),
        (
            "notification",
            "night_silent",
            "false",
            "bool",
            "夜间静音模式",
        ),
        // 企业微信 webhook：地址即凭据，`value_type = 'secret'` 让它不出现在
        // 通用设置列表里，只通过「系统设置 → 企业微信群机器人」专用接口读写。
        // 留空 = 用环境变量 ROBODOG_WECOM_WEBHOOK；平台填了就以平台为准。
        (
            "notification",
            notify::WEBHOOK_SETTING_KEY,
            "",
            "secret",
            "企业微信群机器人 webhook 地址（平台内配置，留空则用环境变量）",
        ),
        // 平台中继默认档位：现场板子出公网主要走 4G 模块，720p/1000k 起流就会被反压，
        // 所以默认直接用省流档 640x360@12/500k，链路再差由板端自动降一档。
        // 改这里只影响「以后起流」；保存设置时会立刻 `video_config` 广播给在线板端。
        (
            "video",
            video::RELAY_PROFILE_SETTING,
            video::DEFAULT_RELAY_TIER,
            "string",
            "平台中继默认档位：light=高清 720p / degrade=省流 640x360（4G 推荐）/ degrade2=极省流 320x180",
        ),
        (
            "video",
            video::RELAY_AUTO_DEGRADE_SETTING,
            "true",
            "bool",
            "中继按上行带宽自动降档（关闭则锁死在默认档位，只保留断线重连保护）",
        ),
    ];

    for (group, key, value, value_type, description) in settings {
        sqlx::query(
            "INSERT OR IGNORE INTO settings
                (group_name, setting_key, setting_value, value_type, description, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(group)
        .bind(key)
        .bind(value)
        .bind(value_type)
        .bind(description)
        .bind(now())
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn seed_business_data(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let created = now();

    let parks = [
        (
            1,
            "PK-001",
            "张江智造园区",
            "总部一期 · 生产制造",
            "上海市浦东新区张江路 100 号",
            12.8,
            "陈志远",
            "生产制造",
            "运行中",
            "#0284c7",
        ),
        (
            2,
            "PK-002",
            "临港物流基地",
            "仓储物流 · 二期扩建",
            "上海市浦东新区临港大道 88 号",
            18.5,
            "李文博",
            "仓储物流",
            "运行中",
            "#0d9488",
        ),
        (
            3,
            "PK-003",
            "滨江研发中心",
            "研发办公 · 试点区域",
            "上海市徐汇区滨江大道 66 号",
            6.2,
            "周雨欣",
            "研发办公",
            "部分维护",
            "#7c3aed",
        ),
    ];
    for (id, code, name, desc, address, area, owner, park_type, status, color) in parks {
        sqlx::query(
            "INSERT INTO parks (id, park_code, park_name, park_description, park_address, area,
                owner_name, park_type, status, color, created_at, updated_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1)",
        )
        .bind(id)
        .bind(code)
        .bind(name)
        .bind(desc)
        .bind(address)
        .bind(area)
        .bind(owner)
        .bind(park_type)
        .bind(status)
        .bind(color)
        .bind(&created)
        .bind(&created)
        .execute(pool)
        .await?;
    }

    let zones = [
        (1, 1, "A区 · 生产车间", "生产区", "每 2 小时", "正常"),
        (2, 1, "B区 · 立体仓库", "仓储区", "每 4 小时", "正常"),
        (3, 1, "C区 · 外围周界", "周界", "每 1 小时", "正常"),
        (4, 2, "D区 · 装卸平台", "作业区", "每 2 小时", "正常"),
        (5, 2, "E区 · 冷链仓", "仓储区", "每 3 小时", "告警"),
        (6, 2, "F区 · 停车广场", "公共区", "每 6 小时", "正常"),
        (7, 3, "G区 · 研发楼", "办公区", "每 4 小时", "正常"),
        (8, 3, "H区 · 地下车库", "地下", "维护中", "维护中"),
    ];
    for (id, park_id, name, zone_type, frequency, status) in zones {
        sqlx::query(
            "INSERT INTO zones (id, park_id, zone_name, zone_type, frequency, status, created_at, updated_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
        )
        .bind(id)
        .bind(park_id)
        .bind(name)
        .bind(zone_type)
        .bind(frequency)
        .bind(status)
        .bind(&created)
        .bind(&created)
        .execute(pool)
        .await?;
    }

    let devices = [
        (
            1,
            "RD-001",
            "X30-Pro",
            1,
            1,
            "执行任务",
            86,
            88,
            38.6,
            266,
            12.4,
            "31.2014, 121.6023",
            "A区上午例行巡检",
            "09:15:32",
            "v2.6.1",
        ),
        (
            2,
            "RD-002",
            "X30-Pro",
            1,
            2,
            "在线待命",
            94,
            83,
            36.2,
            128,
            6.8,
            "31.2021, 121.6041",
            "—",
            "09:15:30",
            "v2.6.1",
        ),
        (
            3,
            "RD-003",
            "Lite-S",
            1,
            3,
            "执行任务",
            71,
            74,
            40.1,
            351,
            18.2,
            "31.1998, 121.6009",
            "周界夜间遗留巡检",
            "09:15:28",
            "v2.6.1",
        ),
        (
            4,
            "RD-004",
            "X30-Pro",
            1,
            1,
            "充电中",
            42,
            76,
            33.4,
            0,
            9.1,
            "充电座 P-01",
            "—",
            "09:15:21",
            "v2.6.1",
        ),
        (
            5,
            "RD-007",
            "Lite-S",
            2,
            4,
            "在线待命",
            88,
            90,
            37.0,
            102,
            4.3,
            "30.8912, 121.9234",
            "—",
            "09:15:18",
            "v2.6.1",
        ),
        (
            6,
            "RD-009",
            "X30-Pro",
            2,
            5,
            "故障",
            63,
            48,
            56.2,
            192,
            11.7,
            "30.8934, 121.9261",
            "—",
            "08:47:12",
            "v2.6.1",
        ),
        (
            7,
            "RD-011",
            "X30-Pro",
            2,
            6,
            "执行任务",
            79,
            81,
            39.3,
            154,
            8.6,
            "30.8901, 121.9208",
            "广场车流巡检",
            "09:15:11",
            "v2.6.1",
        ),
        (
            8,
            "RD-012",
            "Lite-S",
            2,
            4,
            "充电中",
            35,
            52,
            34.1,
            0,
            7.2,
            "充电座 P-04",
            "—",
            "09:14:58",
            "v2.6.1",
        ),
        (
            9,
            "RD-015",
            "X30-Pro",
            3,
            7,
            "在线待命",
            97,
            86,
            35.8,
            46,
            2.1,
            "31.1788, 121.5102",
            "—",
            "09:15:02",
            "v2.6.1",
        ),
        (
            10,
            "RD-018",
            "Lite-S",
            3,
            8,
            "离线",
            18,
            0,
            31.2,
            0,
            15.9,
            "信号丢失",
            "—",
            "07:22:41",
            "v2.6.1",
        ),
        (
            11,
            "RD-021",
            "X30-Pro",
            1,
            3,
            "执行任务",
            82,
            68,
            38.9,
            185,
            10.5,
            "31.2003, 121.5996",
            "周界围栏巡检",
            "09:14:47",
            "v2.6.1",
        ),
        (
            12,
            "RD-024",
            "Lite-S",
            3,
            7,
            "在线待命",
            90,
            61,
            36.6,
            72,
            3.7,
            "31.1792, 121.5116",
            "—",
            "09:14:39",
            "v2.6.1",
        ),
    ];
    for device in devices {
        sqlx::query(
            "INSERT INTO devices (id, device_id, model, park_id, zone_id, status, battery, signal,
                temperature, runtime_minutes, odometer, position, current_task_name, last_heartbeat,
                firmware, device_ip, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, NULL, ?16, ?17)",
        )
        .bind(device.0)
        .bind(device.1)
        .bind(device.2)
        .bind(device.3)
        .bind(device.4)
        .bind(device.5)
        .bind(device.6)
        .bind(device.7)
        .bind(device.8)
        .bind(device.9)
        .bind(device.10)
        .bind(device.11)
        .bind(device.12)
        .bind(device.13)
        .bind(device.14)
        .bind(&created)
        .bind(&created)
        .execute(pool)
        .await?;
    }

    // 演示「刚接入、尚未绑定园区」的设备：注册后需在设备管理里手动绑定园区
    sqlx::query(
        "INSERT INTO devices (device_id, model, park_id, zone_id, status, battery, signal,
            temperature, runtime_minutes, odometer, position, current_task_name, last_heartbeat,
            firmware, device_ip, created_at, updated_at)
         VALUES ('RD-013', 'X30-Pro', NULL, NULL, '待绑定', 100, 0, 32.0, 0, 0, NULL, '—',
            NULL, 'v2.6.1', NULL, ?1, ?1)",
    )
    .bind(&created)
    .execute(pool)
    .await?;

    seed_routes(pool, &created).await?;

    let tasks = [
        (
            1,
            "TK-1109-01",
            "A区上午例行巡检",
            "定时巡检",
            "周期任务",
            "常规",
            1,
            1,
            1,
            Some(1),
            "09:00",
            "09:40",
            "执行中",
            62,
            "按路线顺序执行",
            "覆盖产线 6 个关键点位，含配电柜红外测温与仪表读数识别",
        ),
        (
            2,
            "TK-1109-02",
            "周界围栏异常复核",
            "应急处置",
            "临时下发",
            "紧急",
            11,
            1,
            3,
            Some(2),
            "09:05",
            "09:30",
            "执行中",
            38,
            "跳过非必经点",
            "周界围栏异常复核，重点检查西北角与北侧中段",
        ),
        (
            3,
            "TK-1109-03",
            "立体仓库红外测温",
            "定点拍照",
            "周期任务",
            "常规",
            2,
            1,
            2,
            Some(1),
            "09:20",
            "09:50",
            "待执行",
            0,
            "按路线顺序执行",
            "立体仓库货架红外测温",
        ),
        (
            4,
            "TK-1109-04",
            "冷链仓夜间闭馆巡检",
            "夜巡任务",
            "周期任务",
            "常规",
            5,
            2,
            5,
            Some(3),
            "22:00",
            "22:40",
            "待执行",
            0,
            "按路线顺序执行",
            "夜间闭馆巡检，重点检测冷媒泄漏",
        ),
        (
            5,
            "TK-1109-05",
            "装卸平台早班巡查",
            "定时巡检",
            "周期任务",
            "常规",
            7,
            2,
            4,
            Some(1),
            "08:00",
            "08:35",
            "已完成",
            100,
            "按路线顺序执行",
            "装卸平台早班巡查",
        ),
        (
            6,
            "TK-1109-06",
            "研发楼消防通道检查",
            "定点拍照",
            "周期任务",
            "常规",
            9,
            3,
            7,
            Some(4),
            "08:30",
            "09:00",
            "已完成",
            100,
            "按路线顺序执行",
            "研发楼消防通道占用识别",
        ),
        (
            7,
            "TK-1109-07",
            "广场车流与人员密度采集",
            "定时巡检",
            "临时下发",
            "常规",
            7,
            2,
            6,
            Some(2),
            "09:10",
            "09:45",
            "待执行",
            0,
            "仅执行采集动作",
            "广场车流与人员密度采集",
        ),
        (
            8,
            "TK-1108-12",
            "冷链仓温度传感器校准",
            "设备维护",
            "周期任务",
            "重要",
            6,
            2,
            5,
            Some(3),
            "15:20",
            "16:00",
            "异常",
            47,
            "按路线顺序执行",
            "冷链仓温度传感器校准",
        ),
        (
            9,
            "TK-1108-09",
            "地下车库建图补采",
            "地图采集",
            "临时下发",
            "重要",
            10,
            3,
            8,
            None,
            "14:00",
            "14:40",
            "异常",
            23,
            "按路线顺序执行",
            "地下车库建图补采",
        ),
        (
            10,
            "TK-1109-08",
            "一期周界全景建图",
            "地图采集",
            "临时下发",
            "常规",
            3,
            1,
            3,
            Some(2),
            "10:00",
            "10:40",
            "待执行",
            0,
            "按路线顺序执行",
            "一期周界全景建图",
        ),
    ];
    for task in tasks {
        sqlx::query(
            "INSERT INTO tasks (id, task_code, task_name, task_type, execute_mode, level, device_id,
                park_id, zone_id, route_id, plan_start, plan_end, task_status, progress,
                dispatch_policy, description, created_at, updated_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, 1)",
        )
        .bind(task.0)
        .bind(task.1)
        .bind(task.2)
        .bind(task.3)
        .bind(task.4)
        .bind(task.5)
        .bind(task.6)
        .bind(task.7)
        .bind(task.8)
        .bind(task.9)
        .bind(task.10)
        .bind(task.11)
        .bind(task.12)
        .bind(task.13)
        .bind(task.14)
        .bind(task.15)
        .bind(&created)
        .bind(&created)
        .execute(pool)
        .await?;
    }

    let date = today();
    let alerts = [
        (
            1,
            "AL-001",
            "严重",
            "RD-009 电池温度异常（56.2℃）",
            6,
            2,
            "未处理",
            "09:12:04",
        ),
        (
            2,
            "AL-002",
            "警告",
            "RD-018 设备离线，信号丢失超 2 小时",
            10,
            3,
            "未处理",
            "08:47:31",
        ),
        (
            3,
            "AL-003",
            "警告",
            "E区冷链仓巡检任务超时未开始",
            5,
            2,
            "处理中",
            "08:31:16",
        ),
        (
            4,
            "AL-004",
            "提示",
            "RD-012 电量低于 40%，已自动回充",
            8,
            2,
            "已处理",
            "08:15:02",
        ),
        (
            5,
            "AL-005",
            "提示",
            "C区周界红外发现移动目标（已复核为人员）",
            11,
            1,
            "已处理",
            "07:56:44",
        ),
    ];
    for alert in alerts {
        sqlx::query(
            "INSERT INTO alerts (id, alert_code, level, content, device_id, park_id, status,
                occurred_at, handled_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(alert.0)
        .bind(alert.1)
        .bind(alert.2)
        .bind(alert.3)
        .bind(alert.4)
        .bind(alert.5)
        .bind(alert.6)
        .bind(format!("{date} {}", alert.7))
        .bind(if alert.6 == "已处理" {
            Some(format!("{date} 09:20:00"))
        } else {
            None
        })
        .bind(&created)
        .execute(pool)
        .await?;
    }

    let events = [
        (
            "success",
            "装卸平台早班巡查 已完成",
            "RD-011 · 采集照片 24 张，未发现异常",
            "RD-011",
            "TK-1109-05",
            "08:35",
        ),
        (
            "running",
            "A区上午例行巡检 开始执行",
            "RD-001 · 预计 09:40 完成",
            "RD-001",
            "TK-1109-01",
            "09:00",
        ),
        (
            "warning",
            "RD-009 触发温度告警",
            "已自动暂停任务并通知运维人员",
            "RD-009",
            "",
            "09:12",
        ),
        (
            "charging",
            "RD-012 进入自动回充",
            "充电座 P-04 · 当前电量 35%",
            "RD-012",
            "",
            "09:15",
        ),
    ];
    for (event_type, title, description, device_code, task_code, time) in events {
        let device_id: Option<i64> =
            sqlx::query_scalar("SELECT id FROM devices WHERE device_id = ?1")
                .bind(device_code)
                .fetch_optional(pool)
                .await?;
        let task_id: Option<i64> = if task_code.is_empty() {
            None
        } else {
            sqlx::query_scalar("SELECT id FROM tasks WHERE task_code = ?1")
                .bind(task_code)
                .fetch_optional(pool)
                .await?
        };
        sqlx::query(
            "INSERT INTO task_events (event_type, title, description, device_id, task_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(event_type)
        .bind(title)
        .bind(description)
        .bind(device_id)
        .bind(task_id)
        .bind(format!("{date} {time}:00"))
        .execute(pool)
        .await?;
    }

    let trend_plan = [16, 18, 15, 20, 19, 22, 18];
    let trend_done = [15, 17, 15, 19, 18, 21, 17];
    for index in 0..7 {
        let day = Local::now().date_naive() - Duration::days(6 - index as i64);
        sqlx::query(
            "INSERT INTO daily_stats (stat_date, plan_count, done_count) VALUES (?1, ?2, ?3)",
        )
        .bind(day.format("%Y-%m-%d").to_string())
        .bind(trend_plan[index])
        .bind(trend_done[index])
        .execute(pool)
        .await?;
    }

    tracing::info!("seeded demo data for robodog ops console");
    Ok(())
}

async fn seed_routes(pool: &SqlitePool, created: &str) -> Result<(), sqlx::Error> {
    let raw = include_str!("../seed/routes.json");
    let routes: Vec<Value> = serde_json::from_str(raw).expect("seed/routes.json is valid JSON");

    for route in routes {
        let route_code = route["routeCode"].as_str().unwrap_or("RT-000");
        let route_name = route["routeName"].as_str().unwrap_or("未命名路线");
        let park_id = route["parkId"].as_i64();
        let zone_id = route["zoneId"].as_i64();
        let status = route["status"].as_str().unwrap_or("草稿");
        let version = route["version"].as_str().unwrap_or("v0.1");
        let loop_mode = route["loopMode"].as_str().unwrap_or("闭环");
        let speed = route["defaultSpeed"].as_f64().unwrap_or(1.2);
        let description = route["routeDescription"].as_str();
        let route_data = route["routeData"].clone();
        let metrics = route_metrics(&route_data, speed);
        let published_at = if status == "已发布" {
            Some(created.to_string())
        } else {
            None
        };

        sqlx::query(
            "INSERT INTO routes (route_code, route_name, park_id, zone_id, status, version,
                loop_mode, default_speed, route_description, route_data, point_count, distance_m,
                duration_s, battery_pct, published_at, created_at, updated_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 1)",
        )
        .bind(route_code)
        .bind(route_name)
        .bind(park_id)
        .bind(zone_id)
        .bind(status)
        .bind(version)
        .bind(loop_mode)
        .bind(speed)
        .bind(description)
        .bind(route_data.to_string())
        .bind(metrics.point_count)
        .bind(metrics.distance_m)
        .bind(metrics.duration_s)
        .bind(metrics.battery_pct)
        .bind(published_at)
        .bind(created)
        .bind(created)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// 地图基线演示数据：只在地图版本库为空时写入，已有业务数据也会自动补齐。
async fn seed_maps(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let map_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM map_versions")
        .fetch_one(pool)
        .await?;
    if map_count > 0 {
        return Ok(());
    }
    let park_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM parks")
        .fetch_one(pool)
        .await?;
    if park_count == 0 {
        return Ok(());
    }

    let created = now();
    let maps = [
        (
            "MAP-ZJ-01@v2.1",
            1,
            "MAP-ZJ-01",
            "v2.1",
            "zj_map / 局部米制",
            "GCJ-02 锚点 + 局部 ENU",
            "31.2014, 121.6023",
            "0.05 m/px",
            "1F / 2F / B1",
            18,
            4,
            "已发布",
            Some("2026-11-06 10:20:00"),
            "c4f1a7d29b8e5306a1d3f70c2b9e6481d7a03f5c9e2b6a48c1d9f0e7b3a2546c",
            "一期全园区建图，标定充电座与配电房禁行区；当前生效基线",
        ),
        (
            "MAP-ZJ-01@v2.0",
            1,
            "MAP-ZJ-01",
            "v2.0",
            "zj_map / 局部米制",
            "GCJ-02 锚点 + 局部 ENU",
            "31.2014, 121.6023",
            "0.05 m/px",
            "1F / 2F",
            16,
            3,
            "已归档",
            Some("2026-09-18 14:02:00"),
            "8b27e0c4d6a1f9352e7c4b80d1a63f9e5c2d7b40a8f16e93c5b2d7a4f0e68139",
            "原点偏移修正前的版本，因仍有路线引用而保留，不可删除",
        ),
        (
            "MAP-LG-01@v1.4",
            2,
            "MAP-LG-01",
            "v1.4",
            "lg_map / 局部米制",
            "GCJ-02 锚点 + 局部 ENU",
            "30.8912, 121.9234",
            "0.06 m/px",
            "1F / B1",
            12,
            2,
            "已发布",
            Some("2026-10-28 16:45:00"),
            "3d9f6a1c8e47b2507c4d1e93a6f8b5d2e0c7a4f19b6d3e82c5a0f7b4d1e96823",
            "二期扩建后重标定，冷链仓低温区单独建模",
        ),
        (
            "MAP-BJ-01@v0.9",
            3,
            "MAP-BJ-01",
            "v0.9",
            "bj_map / 局部米制",
            "GCJ-02 锚点 + 局部 ENU",
            "31.1788, 121.5102",
            "0.05 m/px",
            "1F（B1 待建图）",
            7,
            1,
            "部分标定",
            Some("2026-11-08 14:30:00"),
            "7e2c9a4f1b6d8035c2e7f4a9d1b6c3e80f5a2d7b4c1e968f30a6d9b2e7c4f1a5",
            "仅 G 区研发楼可用；H 区地下车库建图补采中，该区域禁止编排航点",
        ),
    ];

    for (
        id,
        park_id,
        map_name,
        version,
        frame_id,
        crs,
        origin,
        resolution,
        floors,
        road_count,
        no_go_count,
        status,
        published_at,
        map_hash,
        note,
    ) in maps
    {
        sqlx::query(
            "INSERT INTO map_versions (id, park_id, map_name, version, frame_id, crs, origin,
                resolution, floors, road_count, no_go_count, status, map_hash, map_width,
                map_height, note, published_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 1600, 860, ?14, ?15, ?16)",
        )
        .bind(id)
        .bind(park_id)
        .bind(map_name)
        .bind(version)
        .bind(frame_id)
        .bind(crs)
        .bind(origin)
        .bind(resolution)
        .bind(floors)
        .bind(road_count)
        .bind(no_go_count)
        .bind(status)
        .bind(map_hash)
        .bind(note)
        .bind(published_at)
        .bind(&created)
        .execute(pool)
        .await?;
    }

    for (park_id, map_id, map_status) in [
        (1, "MAP-ZJ-01@v2.1", "已标定"),
        (2, "MAP-LG-01@v1.4", "已标定"),
        (3, "MAP-BJ-01@v0.9", "部分标定"),
    ] {
        sqlx::query("UPDATE parks SET map_version_id = ?1, map_status = ?2 WHERE id = ?3")
            .bind(map_id)
            .bind(map_status)
            .bind(park_id)
            .execute(pool)
            .await?;
    }

    for (route_code, map_id) in [
        ("RT-001", "MAP-ZJ-01@v2.1"),
        ("RT-002", "MAP-ZJ-01@v2.0"),
        ("RT-003", "MAP-LG-01@v1.4"),
        ("RT-004", "MAP-BJ-01@v0.9"),
    ] {
        sqlx::query("UPDATE routes SET map_version_id = ?1 WHERE route_code = ?2")
            .bind(map_id)
            .bind(route_code)
            .execute(pool)
            .await?;
    }

    sqlx::query(
        "UPDATE routes SET map_version_id = (
            SELECT id FROM map_versions m
            WHERE m.park_id = routes.park_id AND m.status IN ('已发布', '部分标定')
            ORDER BY m.published_at DESC, m.created_at DESC LIMIT 1
         )
         WHERE map_version_id IS NULL AND park_id IS NOT NULL",
    )
    .execute(pool)
    .await?;

    tracing::info!("seeded demo map baselines");
    Ok(())
}

/// 是否写入排班 / 工单演示数据（默认关闭，避免正式环境出现「陈志远」这类示例人员）。
fn demo_seed_enabled() -> bool {
    matches!(
        std::env::var("ROBODOG_SEED_DEMO")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// 排班 / 工单演示数据：责任人库为空时才写，已有真实数据不覆盖。
///
/// 班次按**当前真实周**生成（周一到周日），周视图打开就能看到人，
/// 而不是原型里写死的 11-17 ~ 11-23。
async fn seed_staff_schedules(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let staff_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM staff")
        .fetch_one(pool)
        .await?;
    if staff_count > 0 {
        return Ok(());
    }

    let created = now();
    let today = Local::now().date_naive();
    let monday = today - Duration::days(Local::now().weekday().num_days_from_monday() as i64);
    let day = |offset: i64| {
        (monday + Duration::days(offset))
            .format("%Y-%m-%d")
            .to_string()
    };

    let staff = [
        (
            1,
            "EMP-01",
            "陈志远",
            "巡检主管",
            "运维一组",
            "138****1203",
            "#0284c7",
            "在岗",
            "红外测温 / 应急处置",
        ),
        (
            2,
            "EMP-02",
            "李文博",
            "资深工程师",
            "运维二组",
            "139****8871",
            "#0d9488",
            "在岗",
            "设备维护 / 地图标定",
        ),
        (
            3,
            "EMP-03",
            "周雨欣",
            "运维工程师",
            "运维一组",
            "137****2245",
            "#7c3aed",
            "在岗",
            "AI 识别 / 数据分析",
        ),
        (
            4,
            "EMP-04",
            "王建国",
            "夜班班长",
            "运维夜班组",
            "136****5612",
            "#c2410c",
            "休息",
            "周界巡检 / 夜巡",
        ),
        (
            5,
            "EMP-05",
            "赵敏",
            "运维工程师",
            "运维二组",
            "135****9908",
            "#d97706",
            "在岗",
            "冷链仓 / 传感器校准",
        ),
        (
            6,
            "EMP-06",
            "孙浩",
            "现场技术员",
            "运维一组",
            "188****3347",
            "#2563eb",
            "在岗",
            "设备抢修 / 充电维护",
        ),
    ];
    for (id, code, name, role, team, phone, color, status, skills) in staff {
        sqlx::query(
            "INSERT INTO staff (id, staff_code, name, role, team, phone, color, status, skills,
                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        )
        .bind(id)
        .bind(code)
        .bind(name)
        .bind(role)
        .bind(team)
        .bind(phone)
        .bind(color)
        .bind(status)
        .bind(skills)
        .bind(&created)
        .execute(pool)
        .await?;
    }

    // (日期偏移, 班次, 责任人, 园区, 区域, 巡检计划编号或临时计划名, 状态)
    let shifts = [
        (0, "早班", "陈志远", 1, 1, "TK-1109-01", "已发布"),
        (0, "中班", "周雨欣", 1, 2, "TK-1109-03", "已发布"),
        (0, "夜班", "王建国", 1, 3, "C区周界围栏夜巡", "待确认"),
        (1, "早班", "李文博", 2, 4, "TK-1109-05", "已发布"),
        (1, "中班", "赵敏", 2, 5, "TK-1109-04", "已发布"),
        (2, "早班", "孙浩", 2, 6, "TK-1109-07", "草案"),
        (2, "夜班", "王建国", 3, 8, "地下车库建图补采", "待确认"),
        (3, "早班", "陈志远", 3, 7, "TK-1109-06", "已发布"),
        (4, "中班", "周雨欣", 1, 1, "一期周界全景建图", "已发布"),
    ];
    let mut shift_id = 0;
    for (offset, slot, owner, park_id, zone_id, task_ref, status) in shifts {
        shift_id += 1;
        // 演示数据写进已存在的库时，园区/区域 id 可能对不上（老库删过数据），
        // 外键开着，这里必须退化成 NULL，否则整个服务起不来
        let park_id = if ref_exists(pool, "parks", park_id).await? {
            Some(park_id)
        } else {
            None
        };
        let zone_id = if ref_exists(pool, "zones", zone_id).await? {
            Some(zone_id)
        } else {
            None
        };
        let time_range = match slot {
            "早班" => "08:00 - 16:00",
            "中班" => "16:00 - 00:00",
            _ => "00:00 - 08:00",
        };
        // 计划编号能对上就关联任务，否则当成临时计划名写进 task_name
        let task_id: Option<i64> = sqlx::query_scalar("SELECT id FROM tasks WHERE task_code = ?1")
            .bind(task_ref)
            .fetch_optional(pool)
            .await?;
        let task_name: String = if task_id.is_some() {
            sqlx::query_scalar("SELECT task_name FROM tasks WHERE task_code = ?1")
                .bind(task_ref)
                .fetch_one(pool)
                .await?
        } else {
            task_ref.to_string()
        };
        let staff_id: Option<i64> = sqlx::query_scalar("SELECT id FROM staff WHERE name = ?1")
            .bind(owner)
            .fetch_optional(pool)
            .await?;
        sqlx::query(
            "INSERT INTO shifts (id, shift_code, shift_date, slot, time_range, staff_id, owner_name,
                park_id, zone_id, task_id, task_name, status, note, created_at, updated_at,
                created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, ?13, ?13, 1)",
        )
        .bind(shift_id)
        .bind(format!("SH-{shift_id:03}"))
        .bind(day(offset))
        .bind(slot)
        .bind(time_range)
        .bind(staff_id)
        .bind(owner)
        .bind(park_id)
        .bind(zone_id)
        .bind(task_id)
        .bind(&task_name)
        .bind(status)
        .bind(&created)
        .execute(pool)
        .await?;
    }

    // 工单：告警转单 / 手动创建 / 验收完成各留一条，覆盖四种状态
    let workorders = [
        (
            "WO-0001",
            "RD-009 电池温度异常处置",
            "告警处置",
            "AL-001",
            Some("AL-001"),
            "紧急",
            "李文博",
            2,
            6,
            "处理中",
            55,
            "电池温度 56.2℃ 超过阈值，需现场检查散热与电池健康度，必要时停机检修。",
            Some(1_i64),
            (0, 9, 12),
            (0, 10, 12),
            vec![
                (
                    "09:12",
                    "系统",
                    "created",
                    "由告警 AL-001 自动生成工单，指派运维二组",
                ),
                (
                    "09:15",
                    "李文博",
                    "accept",
                    "已接单，携带热成像仪赶往 E 区冷链仓",
                ),
                (
                    "09:24",
                    "李文博",
                    "comment",
                    "到达现场，开始检测电池模组温度分布",
                ),
            ],
        ),
        (
            "WO-0002",
            "RD-018 离线设备信号排查",
            "告警处置",
            "AL-002",
            Some("AL-002"),
            "高",
            "孙浩",
            3,
            10,
            "待接单",
            0,
            "设备离线超 2 小时、信号丢失，需排查地下车库网关覆盖与设备供电。",
            Some(2_i64),
            (0, 8, 50),
            (0, 11, 50),
            vec![(
                "08:50",
                "系统",
                "created",
                "由告警 AL-002 自动生成工单，指派运维一组",
            )],
        ),
        (
            "WO-0003",
            "E区冷链仓任务超时复核",
            "巡检复核",
            "AL-003",
            Some("AL-003"),
            "中",
            "赵敏",
            2,
            5,
            "待接单",
            0,
            "冷链仓巡检任务超时未开始，需确认设备状态与任务下发链路。",
            Some(3_i64),
            (0, 8, 35),
            (0, 13, 0),
            vec![(
                "08:35",
                "系统",
                "created",
                "由告警 AL-003 转入工单，指派运维二组",
            )],
        ),
        (
            "WO-0004",
            "RD-012 电量异常回充检查",
            "设备维护",
            "手动创建",
            None,
            "低",
            "赵敏",
            2,
            8,
            "待验收",
            90,
            "设备频繁低电量回充，检查充电座 P-04 接触点与电池衰减情况。",
            None,
            (-1, 8, 20),
            (1, 18, 0),
            vec![
                ("08:20", "陈志远", "created", "创建工单并指派赵敏"),
                (
                    "10:15",
                    "赵敏",
                    "comment",
                    "更换充电触点，观察一个充放电周期",
                ),
                (
                    "09:40",
                    "赵敏",
                    "submit",
                    "处理完成，回充频率已恢复正常，提交验收",
                ),
            ],
        ),
        (
            "WO-0005",
            "C区周界红外误报标定",
            "巡检复核",
            "TK-1109-02",
            Some("TK-1109-02"),
            "中",
            "陈志远",
            1,
            11,
            "已完成",
            100,
            "周界红外频繁误报，需调整识别阈值与 ROI 区域。",
            None,
            (-2, 14, 10),
            (0, 18, 0),
            vec![
                ("14:10", "系统", "created", "由复核任务转入工单"),
                (
                    "15:30",
                    "陈志远",
                    "comment",
                    "已完成阈值标定，连续 24h 无误报",
                ),
                ("09:00", "陈志远", "close", "验收通过，工单关闭"),
            ],
        ),
    ];
    for (
        code,
        title,
        order_type,
        source,
        source_ref,
        priority,
        assignee,
        park_id,
        device_id,
        status,
        progress,
        description,
        alert_id,
        (created_off, created_h, created_m),
        (deadline_off, deadline_h, deadline_m),
        logs,
    ) in workorders
    {
        let staff_id: Option<i64> = sqlx::query_scalar("SELECT id FROM staff WHERE name = ?1")
            .bind(assignee)
            .fetch_optional(pool)
            .await?;
        // 告警 / 设备 / 园区都是外键：老库里 id 对不上就写 NULL，只保留来源文案
        let alert_id = match alert_id {
            Some(id) if ref_exists(pool, "alerts", id).await? => Some(id),
            _ => None,
        };
        let device_id = if ref_exists(pool, "devices", device_id).await? {
            Some(device_id)
        } else {
            None
        };
        let park_id = if ref_exists(pool, "parks", park_id).await? {
            Some(park_id)
        } else {
            None
        };
        let created_at = format!("{} {:02}:{:02}:00", day(created_off), created_h, created_m);
        let deadline = format!("{} {:02}:{:02}", day(deadline_off), deadline_h, deadline_m);
        let closed_at = if status == "已完成" {
            Some(now())
        } else {
            None
        };
        let order_id: i64 = sqlx::query_scalar(
            "INSERT INTO work_orders (order_code, title, order_type, source, source_ref, priority,
                staff_id, assignee, park_id, device_id, deadline, status, progress, description,
                alert_id, closed_at, created_at, updated_at, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?17, 1)
             RETURNING id",
        )
        .bind(code)
        .bind(title)
        .bind(order_type)
        .bind(source)
        .bind(source_ref)
        .bind(priority)
        .bind(staff_id)
        .bind(assignee)
        .bind(park_id)
        .bind(device_id)
        .bind(&deadline)
        .bind(status)
        .bind(progress)
        .bind(description)
        .bind(alert_id)
        .bind(&closed_at)
        .bind(&created_at)
        .fetch_one(pool)
        .await?;
        for (time, actor, action, content) in logs {
            let stamp = if time.len() == 5 {
                format!("{} {time}:00", day(created_off.max(0)))
            } else {
                format!("{} {time}", day(created_off.max(0)))
            };
            sqlx::query(
                "INSERT INTO work_order_logs (work_order_id, actor, action, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .bind(order_id)
            .bind(actor)
            .bind(action)
            .bind(content)
            .bind(stamp)
            .execute(pool)
            .await?;
        }
    }

    tracing::info!("seeded demo staff / shifts / work orders");
    Ok(())
}

/// 外键目标是否存在：演示数据写进已有库时用它把缺失的引用降级成 NULL。
async fn ref_exists(pool: &SqlitePool, table: &str, id: i64) -> Result<bool, sqlx::Error> {
    // table 只来自本文件里的常量字面量，不拼接外部输入
    let found: Option<i64> = sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE id = ?1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(found.is_some())
}
