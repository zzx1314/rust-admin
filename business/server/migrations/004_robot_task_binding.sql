-- 智航 Pro 本体对接：任务级机器人回执字段
--
-- 业务任务下发到客户端后，客户端会驱动智航本体执行（建图 / 航线 / 回充 / 出桩）。
-- 本体侧有自己的任务标识与阶段枚举，需要落库以便后台展示与排障。
ALTER TABLE tasks ADD COLUMN robot_task_id TEXT;
ALTER TABLE tasks ADD COLUMN robot_stage TEXT;
ALTER TABLE tasks ADD COLUMN robot_updated_at TEXT;

CREATE INDEX IF NOT EXISTS idx_tasks_robot ON tasks(robot_task_id);
