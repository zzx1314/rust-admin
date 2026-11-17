-- 定时巡检：把「计划时间」从展示字段变成真正的调度依据。
--
-- 以前 plan_start / plan_end 只是两个手填字符串，任何下发路径（建任务即推、设备
-- 上线补发、手动开始）都不看它们，「定时巡检」实际等于「建好就立刻跑」。
-- 现在：
--   plan_start / plan_end   = 每天的执行窗口（`HH:MM`，允许跨零点）
--   schedule_days           = 周期日 `1,2,3,4,5`（1=周一 … 7=周日），空/NULL = 每天
--   last_dispatched_on      = 最近一次「到点下发」的日期（`YYYY-MM-DD`），
--                             同一窗口内不重复下发；也是「明天再跑一次」的依据
-- 时间窗判定与扫描见 `src/schedule.rs`。

ALTER TABLE tasks ADD COLUMN schedule_days TEXT;
ALTER TABLE tasks ADD COLUMN last_dispatched_on TEXT;

-- 扫描的是「待执行」的任务，按窗口挑到点的
CREATE INDEX IF NOT EXISTS idx_tasks_schedule ON tasks(task_status, execute_mode);

-- 历史数据里混着 `YYYY-MM-DD HH:MM(:SS)`，统一截成 `HH:MM`，否则时间窗解析不到。
-- （dashboard 的 `TASK_PLAN_DAY` 对 `HH:MM` 已经回退到 created_at，不受影响。）
UPDATE tasks
   SET plan_start = substr(plan_start, 12, 5)
 WHERE plan_start GLOB '____-__-__ __:__*';
UPDATE tasks
   SET plan_end = substr(plan_end, 12, 5)
 WHERE plan_end GLOB '____-__-__ __:__*';
