-- 企业微信群机器人推送记录（outbox + 审计）。
--
-- 为什么不只打日志：现场问得最多的两个问题是「告警到底推没推出去」和
-- 「哪个 webhook 配错了」。事件流里翻不到外网 HTTP 的结果，必须落库。
--
-- `dedupe_key` 只用于排查（去重本身在内存里做，见 `src/notify.rs`）；
-- `payload` 存**实际发出**的 markdown（已截断），比存中间结构更容易对齐现场。
CREATE TABLE IF NOT EXISTS notifications (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 渠道：目前只有 wecom；留列是为了以后加钉钉 / 邮件时不用改表
    channel TEXT NOT NULL,
    -- 业务来源：alert（告警）/ task（任务异常）/ summary（溢出汇总）/ test（手动测试）
    ref_type TEXT NOT NULL,
    ref_id INTEGER,
    level TEXT NOT NULL,
    dedupe_key TEXT,
    payload TEXT NOT NULL,
    -- sent / failed：failed 行 `last_error` 带企业微信的 errcode+errmsg
    status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TEXT NOT NULL,
    sent_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_notifications_created ON notifications(created_at);
CREATE INDEX IF NOT EXISTS idx_notifications_status ON notifications(status);
