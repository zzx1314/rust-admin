use crate::db::{now, today};
use crate::geo::route_metrics;
use bcrypt::{DEFAULT_COST, hash};
use chrono::{Duration, Local};
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
            "设备异常告警推送",
        ),
        (
            "notification",
            "task_timeout_notice",
            "true",
            "bool",
            "任务超时提醒",
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
                owner_name, park_type, status, color, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            "INSERT INTO zones (id, park_id, zone_name, zone_type, frequency, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
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
            4,
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
            4,
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
            3,
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
            4,
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
            4,
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
            2,
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
            4,
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
            3,
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
            4,
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
            4,
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
            3,
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
         VALUES ('RD-013', 'X30-Pro', NULL, NULL, '待绑定', 100, 4, 32.0, 0, 0, NULL, '—',
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
                dispatch_policy, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
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
        let model = route["model"].as_str();
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
            "INSERT INTO routes (route_code, route_name, park_id, zone_id, model, status, version,
                loop_mode, default_speed, route_description, route_data, point_count, distance_m,
                duration_s, battery_pct, published_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        )
        .bind(route_code)
        .bind(route_name)
        .bind(park_id)
        .bind(zone_id)
        .bind(model)
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
