-- 计划「发布状态」：草稿 / 已发布——只有已发布的计划才允许自动执行定时任务。
--
-- 背景：`task_status` 是**执行**状态（待执行 / 执行中 / 已暂停 / 已完成 / 异常），
-- 它回答的是「这条计划现在跑到哪了」，没法表达「这条计划还没定稿、先别自动跑」。
-- 以前建一条周期计划就等于让它从今天起每个执行日自动下发：想先存一份备稿，只能
-- 把执行模式改成「临时下发」（连带丢掉时间窗），或者干脆不在系统里建。
--
-- 现在补一个与路线（`routes.status`）同口径的发布状态：
--   '草稿'   = 只保存，不参与自动调度。建计划不再顺手下发、扫描器（`schedule::sweep`）
--              跳过、设备上线补发（`gateway::dispatch_pending_tasks`）也跳过；
--              人工「开始执行 / 重新下发」仍可跑（人工覆盖），事件流里会写明原因
--   '已发布' = 到点自动下发
--
-- 默认值取 '已发布'：历史计划保持原有行为。**默认改成草稿等于升级当天全量停跑**，
-- 那才是事故（见 README「定时巡检按计划时间窗执行」）。
--
-- `published_at` 记最近一次发布的时间（撤回不清空），用来回溯「这一版从什么时候
-- 开始自动跑」。判定与拦截见 `src/schedule.rs::is_published` / `DueState::Draft`。
ALTER TABLE tasks ADD COLUMN publish_status TEXT NOT NULL DEFAULT '已发布';
ALTER TABLE tasks ADD COLUMN published_at TEXT;

-- 扫描器按「发布状态 + 执行状态 + 执行模式」挑周期任务
CREATE INDEX IF NOT EXISTS idx_tasks_publish ON tasks(publish_status, task_status, execute_mode);
