CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    display_name TEXT,
    role TEXT NOT NULL DEFAULT 'admin',
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS parks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    park_code TEXT NOT NULL UNIQUE,
    park_name TEXT NOT NULL,
    park_description TEXT,
    park_address TEXT,
    area REAL NOT NULL DEFAULT 0,
    owner_name TEXT,
    park_type TEXT,
    status TEXT NOT NULL DEFAULT '运行中',
    color TEXT NOT NULL DEFAULT '#0284c7',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS zones (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    park_id INTEGER NOT NULL,
    zone_name TEXT NOT NULL,
    zone_type TEXT,
    frequency TEXT,
    status TEXT NOT NULL DEFAULT '正常',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS devices (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id TEXT NOT NULL UNIQUE,
    model TEXT,
    park_id INTEGER,
    zone_id INTEGER,
    status TEXT NOT NULL DEFAULT '离线',
    battery INTEGER NOT NULL DEFAULT 0,
    signal INTEGER NOT NULL DEFAULT 0,
    temperature REAL NOT NULL DEFAULT 0,
    runtime_minutes INTEGER NOT NULL DEFAULT 0,
    odometer REAL NOT NULL DEFAULT 0,
    position TEXT,
    current_task_name TEXT,
    last_heartbeat TEXT,
    firmware TEXT,
    device_ip TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS routes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    route_code TEXT NOT NULL UNIQUE,
    route_name TEXT NOT NULL,
    park_id INTEGER,
    zone_id INTEGER,
    model TEXT,
    status TEXT NOT NULL DEFAULT '草稿',
    version TEXT NOT NULL DEFAULT 'v0.1',
    loop_mode TEXT NOT NULL DEFAULT '闭环',
    default_speed REAL NOT NULL DEFAULT 1.2,
    route_description TEXT,
    route_data TEXT,
    point_count INTEGER NOT NULL DEFAULT 0,
    distance_m REAL NOT NULL DEFAULT 0,
    duration_s INTEGER NOT NULL DEFAULT 0,
    battery_pct INTEGER NOT NULL DEFAULT 0,
    published_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_code TEXT NOT NULL UNIQUE,
    task_name TEXT NOT NULL,
    task_type TEXT,
    execute_mode TEXT,
    level TEXT,
    device_id INTEGER,
    park_id INTEGER,
    zone_id INTEGER,
    route_id INTEGER,
    plan_start TEXT,
    plan_end TEXT,
    task_status TEXT NOT NULL DEFAULT '待执行',
    progress INTEGER NOT NULL DEFAULT 0,
    dispatch_policy TEXT,
    description TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS alerts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    alert_code TEXT NOT NULL UNIQUE,
    level TEXT NOT NULL,
    content TEXT NOT NULL,
    device_id INTEGER,
    park_id INTEGER,
    status TEXT NOT NULL DEFAULT '未处理',
    occurred_at TEXT NOT NULL,
    handled_at TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    group_name TEXT NOT NULL,
    setting_key TEXT NOT NULL UNIQUE,
    setting_value TEXT,
    value_type TEXT NOT NULL DEFAULT 'string',
    description TEXT,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    device_id INTEGER,
    task_id INTEGER,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS daily_stats (
    stat_date TEXT PRIMARY KEY,
    plan_count INTEGER NOT NULL DEFAULT 0,
    done_count INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_zones_park ON zones(park_id);
CREATE INDEX IF NOT EXISTS idx_devices_park ON devices(park_id);
CREATE INDEX IF NOT EXISTS idx_devices_zone ON devices(zone_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(task_status);
CREATE INDEX IF NOT EXISTS idx_tasks_device ON tasks(device_id);
CREATE INDEX IF NOT EXISTS idx_alerts_status ON alerts(status);
CREATE INDEX IF NOT EXISTS idx_alerts_occurred ON alerts(occurred_at);
