-- 计划级「自动开灯」：整个计划期间开哪几盏灯（补光灯 / LED 灯带）。
--
-- 为什么要加这两列：现场要的是「按计划开灯」——计划里勾了自动开灯，就**不再**按
-- 航点动作 `lightFill` / `lightLed` 到点开、离开关，而是从任务开始一直亮到任务结束
-- （终态 / 取消）。原来的 `light_night_only` 只是「到点的那几盏灯只在夜间开」，
-- 表达不了「计划没勾自动开灯」和「勾了但不限夜间」的区别（两者都是 `0`）。
--
-- 两层语义（客户端 `ActiveTask::light_at` 一处收口）：
--
-- 1. `light_fill` / `light_led` 任一为 1 → **以计划为准**：航点上的开灯动作被忽略，
--    任务开始就开、终态 / 取消才关；`light_night_only` 仍然管「只在夜间开」。
-- 2. 两列都是 0 → 保持老行为：按航点动作到点开 / 离开关（可配合 `light_night_only`）。
--
-- 回填：已经勾了「仅夜间自动开灯」的计划，语义是「这些灯夜里要亮」，按「两盏都开」
-- 落库，避免升级后现场那条计划突然不亮灯。老计划（`light_night_only = 0`）保持 0，
-- 即继续按航点动作走，行为不变。
ALTER TABLE tasks ADD COLUMN light_fill INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN light_led INTEGER NOT NULL DEFAULT 0;

UPDATE tasks SET light_fill = 1, light_led = 1 WHERE light_night_only = 1;
