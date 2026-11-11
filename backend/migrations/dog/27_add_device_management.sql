-- @migration-name: add_device_management

-- 中台设备台账：业务平台（business/）把设备同步到这里，中台按设备做用户分配，
-- 业务平台用同一个中台账号登录后只看到自己被分配的设备。
--
-- `device_id` 与业务库 `devices.device_id` 对齐，是跨服务关联键；园区只存
-- 编码/名称冗余，不建外键（中台不持有园区表）。
-- 分配关系落在 `dog_user_device`（多对多）：一个设备可分配给多个用户，
-- 一个用户也可以管辖多台设备。

CREATE TABLE IF NOT EXISTS dog_device (
    id INTEGER PRIMARY KEY,
    device_id TEXT NOT NULL,
    device_name TEXT,
    model TEXT,
    status TEXT NOT NULL DEFAULT '离线',
    firmware TEXT,
    device_ip TEXT,
    park_code TEXT,
    park_name TEXT,
    remarks TEXT,
    -- 创建者（中台账号），非管理员创建的设备默认分配给创建者
    user_id INTEGER,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_dog_device_code ON dog_device(device_id);
CREATE INDEX IF NOT EXISTS idx_dog_device_status ON dog_device(status);
CREATE INDEX IF NOT EXISTS idx_dog_device_park ON dog_device(park_code);

CREATE TABLE IF NOT EXISTS dog_user_device (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    device_id INTEGER NOT NULL,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_dog_user_device ON dog_user_device(user_id, device_id);
CREATE INDEX IF NOT EXISTS idx_dog_user_device_user ON dog_user_device(user_id);
CREATE INDEX IF NOT EXISTS idx_dog_user_device_device ON dog_user_device(device_id);
