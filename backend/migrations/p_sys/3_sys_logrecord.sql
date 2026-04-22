-- 日志表
CREATE TABLE IF NOT EXISTS p_sys_logrecord (
    id INTEGER PRIMARY KEY,
    tenant TEXT,
    type TEXT,
    sub_type TEXT,
    biz_no TEXT,
    operator TEXT,
    action TEXT,
    fail INTEGER DEFAULT 0,
    create_time TEXT,
    extra TEXT,
    code_variable TEXT,
    ip TEXT,
    is_deleted INTEGER DEFAULT 0
);
