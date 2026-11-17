-- 班次自定义：早班 / 中班 / 夜班原来是写死在 Rust 里的三条常量（`SLOT_RANGES`），
-- 现场作息不一样时（两班倒、「白班 + 晚高峰班」、园区不同班次）既加不了也改不了。
-- 这里把班次定义搬进表，名称与时段由「排班管理 → 班次设置」维护，可增删改、可停用。
--
-- shifts.slot       仍存班次**名**（历史数据就是这么存的，改名时同步更新老排班）
-- shifts.time_range 是该条排班写入时的时段快照，改班次时段时由服务端一起刷新
--                   （同班次不应该在列表里出现两种时段）

CREATE TABLE IF NOT EXISTS shift_slots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 班次名，排班里的 slot 就是它；全局唯一
    name TEXT NOT NULL UNIQUE,
    -- `HH:MM`；end <= start 表示跨零点（如 22:00 - 06:00 / 16:00 - 00:00）
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL,
    -- 周视图左侧标签与列表标签配色
    color TEXT NOT NULL DEFAULT '#0284c7',
    -- 周视图从上到下的行顺序，数字小的在上
    sort_order INTEGER NOT NULL DEFAULT 0,
    -- 0 = 已停用：不再出现在「新建排班」下拉与周视图行里，历史排班照旧显示
    active INTEGER NOT NULL DEFAULT 1,
    note TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_shift_slots_order ON shift_slots(sort_order, id);

-- 三个默认班次：只插入表里还没有的同名班次，重复执行迁移或用户改过之后都不会被覆盖
INSERT INTO shift_slots (name, start_time, end_time, color, sort_order, note,
    created_at, updated_at)
SELECT '早班', '08:00', '16:00', '#0284c7', 1, '默认班次',
       datetime('now', 'localtime'), datetime('now', 'localtime')
 WHERE NOT EXISTS (SELECT 1 FROM shift_slots WHERE name = '早班');

INSERT INTO shift_slots (name, start_time, end_time, color, sort_order, note,
    created_at, updated_at)
SELECT '中班', '16:00', '00:00', '#7c3aed', 2, '默认班次',
       datetime('now', 'localtime'), datetime('now', 'localtime')
 WHERE NOT EXISTS (SELECT 1 FROM shift_slots WHERE name = '中班');

INSERT INTO shift_slots (name, start_time, end_time, color, sort_order, note,
    created_at, updated_at)
SELECT '夜班', '00:00', '08:00', '#0f766e', 3, '默认班次',
       datetime('now', 'localtime'), datetime('now', 'localtime')
 WHERE NOT EXISTS (SELECT 1 FROM shift_slots WHERE name = '夜班');

-- 老库里如果有表外班次名（例如手工写库 / 早期演示数据），补成停用班次，
-- 保证「班次设置」能看到它们、也能被改名合并，而不是躲在排班里点不开。
INSERT INTO shift_slots (name, start_time, end_time, color, sort_order, active, note,
    created_at, updated_at)
SELECT DISTINCT s.slot,
       COALESCE(NULLIF(substr(TRIM(s.time_range), 1, 5), ''), '00:00'),
       COALESCE(NULLIF(substr(TRIM(s.time_range), 9, 5), ''), '00:00'),
       '#64748b',
       COALESCE((SELECT MAX(sort_order) FROM shift_slots), 0) + 1,
       0,
       '由历史排班补录，请核对时段',
       datetime('now', 'localtime'), datetime('now', 'localtime')
  FROM shifts s
 WHERE s.slot IS NOT NULL AND TRIM(s.slot) <> ''
   AND NOT EXISTS (SELECT 1 FROM shift_slots ss WHERE ss.name = s.slot);
