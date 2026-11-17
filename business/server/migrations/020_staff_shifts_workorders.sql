-- 排班（计划层）与工单（执行层）以及两者共用的责任人库。
--
-- 原型 `designprototype/robodog-management-system.html` 的「排班管理 / 工单管理」两页。
-- 一条巡检计划（tasks）在计划层被排成某个人某个班次的班次（shifts），
-- 执行层出现的异常与人工任务走工单（work_orders）派发到同一个责任人库（staff）。

-- 责任人库：排班与工单共用的处理人
CREATE TABLE IF NOT EXISTS staff (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    staff_code TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    role TEXT,
    team TEXT,
    phone TEXT,
    -- 头像底色（前端 initials 圆形头像用）
    color TEXT NOT NULL DEFAULT '#0284c7',
    -- 在岗 / 休息 / 请假
    status TEXT NOT NULL DEFAULT '在岗',
    skills TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 排班：日期 + 班次 + 责任人，关联巡检计划与园区/区域
CREATE TABLE IF NOT EXISTS shifts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    shift_code TEXT NOT NULL UNIQUE,
    -- YYYY-MM-DD；周视图按真实日期分列，不写死「11-17」这种演示日期
    shift_date TEXT NOT NULL,
    -- 早班 / 中班 / 夜班
    slot TEXT NOT NULL,
    time_range TEXT NOT NULL,
    staff_id INTEGER REFERENCES staff(id),
    -- 冗余一份姓名：责任人库改名/删除后排班仍能显示当时的人
    owner_name TEXT NOT NULL,
    park_id INTEGER REFERENCES parks(id),
    zone_id INTEGER REFERENCES zones(id),
    task_id INTEGER REFERENCES tasks(id),
    task_name TEXT,
    -- 已发布 / 待确认 / 草案
    status TEXT NOT NULL DEFAULT '草案',
    note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    created_by INTEGER REFERENCES users(id)
);

-- 同一责任人同一天同一班次只能有一条排班（新建/修改时后端据此返回冲突原因）
CREATE UNIQUE INDEX IF NOT EXISTS idx_shifts_unique_slot
    ON shifts(shift_date, slot, owner_name);
CREATE INDEX IF NOT EXISTS idx_shifts_date ON shifts(shift_date);

-- 工单：待接单 → 处理中 → 待验收 → 已完成
CREATE TABLE IF NOT EXISTS work_orders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    order_code TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    -- 告警处置 / 巡检复核 / 设备维护 / 地图补采 / 临时任务
    order_type TEXT NOT NULL DEFAULT '告警处置',
    -- 来源描述（手动创建 / AL-001 / TK-1109-02 …）
    source TEXT NOT NULL DEFAULT '手动创建',
    -- 来源单据编号，告警转工单时保留原始 AL-/TK- 编号
    source_ref TEXT,
    -- 紧急 / 高 / 中 / 低
    priority TEXT NOT NULL DEFAULT '中',
    staff_id INTEGER REFERENCES staff(id),
    assignee TEXT NOT NULL,
    park_id INTEGER REFERENCES parks(id),
    device_id INTEGER REFERENCES devices(id),
    -- YYYY-MM-DD HH:MM（前端按是否当天决定显示 HH:MM 还是 MM-DD HH:MM）
    deadline TEXT,
    status TEXT NOT NULL DEFAULT '待接单',
    progress INTEGER NOT NULL DEFAULT 0,
    description TEXT,
    -- 由告警转入时记录告警 id，便于反查
    alert_id INTEGER REFERENCES alerts(id),
    closed_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    created_by INTEGER REFERENCES users(id)
);

CREATE INDEX IF NOT EXISTS idx_work_orders_status ON work_orders(status);
CREATE INDEX IF NOT EXISTS idx_work_orders_created ON work_orders(created_at);

-- 工单处理轨迹（详情页时间线）
CREATE TABLE IF NOT EXISTS work_order_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    work_order_id INTEGER NOT NULL REFERENCES work_orders(id) ON DELETE CASCADE,
    -- 操作人：系统 / 管理员 / 责任人姓名
    actor TEXT NOT NULL,
    -- created / accept / progress / submit / close / status / comment
    action TEXT NOT NULL DEFAULT 'comment',
    content TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_work_order_logs_order
    ON work_order_logs(work_order_id, id);
