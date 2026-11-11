-- 中台 → 业务平台内部调用的幂等记录。
--
-- 中台是控制面：它发起指令/任务时会带一个 `requestId`，重试时保持不变。
-- 业务平台在真正下发前先查这张表，命中就直接回放上次响应，避免设备收到
-- 两条相同指令（例如两次「召回充电」）。
--
-- 中台侧另有 `dog_command_audit` 做同样的幂等与审计；两边各自记录，
-- 任一服务重启或网络抖动都不会导致重复下发。

CREATE TABLE IF NOT EXISTS zentai_requests (
    -- 中台生成的幂等键
    request_id TEXT PRIMARY KEY,
    -- command / task
    kind TEXT NOT NULL,
    device_code TEXT,
    -- 首次成功响应（JSON 文本），用于重放
    response TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_zentai_requests_device
    ON zentai_requests(device_code, created_at);
