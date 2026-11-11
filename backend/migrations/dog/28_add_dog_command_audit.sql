-- @migration-name: add_dog_command_audit

-- 设备指令审计台账。
--
-- 中台承担「控制面」但不持有设备连接：指令实际由业务平台下发。因此中台必须
-- 自己留下「谁、什么时候、对哪台设备、下了什么、结果如何」，否则控制权无法追溯。
--
-- `request_id` 同时是幂等键：唯一索引保证同一次中台操作重试时只落一条记录，
-- 配合 service 层的「已存在则回放上次结果」实现不重复下发。
--
-- 指令结果（业务平台返回的 data）存 JSON 文本，便于回放与排障。

CREATE TABLE IF NOT EXISTS dog_command_audit (
    id INTEGER PRIMARY KEY,
    -- 操作人（中台账号）
    user_id INTEGER,
    username TEXT,
    -- 目标设备编号，与 dog_device.device_id 对齐（不建外键，沿用 dog 域惯例）
    device_code TEXT NOT NULL,
    -- 指令名或任务下发标记，例如「召回充电」/「下发任务」
    action TEXT NOT NULL,
    request_id TEXT NOT NULL,
    -- 请求参数（JSON 文本）
    params TEXT,
    -- pending / succeeded / failed
    status TEXT NOT NULL DEFAULT 'pending',
    -- 业务平台返回值（JSON 文本），供幂等回放
    result TEXT,
    message TEXT,
    latency_ms INTEGER,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_dog_command_audit_request
    ON dog_command_audit(request_id);
CREATE INDEX IF NOT EXISTS idx_dog_command_audit_device
    ON dog_command_audit(device_code, create_time);
