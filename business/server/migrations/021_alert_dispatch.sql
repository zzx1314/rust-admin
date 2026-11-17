-- 告警自动派单：按「当班排班（shifts）」找到责任人，并把自动生成的工单回写到告警行。
--
-- 计划（tasks）本身没有「谁负责」的字段，责任人只存在于计划层排班里：
--   shifts(task_id / park_id / zone_id → staff_id)
-- 所以告警发生当天找当天那一班，就是「通过计划找到关联责任人」。
--
-- 为什么不新建表：派单结果就是告警的一次性派生数据，跟告警生命周期一致；
-- 独立表会让「告警列表」多一次 JOIN，而这里只是四个可空列。

-- 派到的责任人（责任人库外键；删人后置 NULL，assignee 里仍留姓名）
ALTER TABLE alerts ADD COLUMN staff_id INTEGER REFERENCES staff(id);
-- 责任人姓名快照：责任人库改名/删除后，告警里仍能显示当时派给了谁
ALTER TABLE alerts ADD COLUMN assignee TEXT;
-- 自动生成的工单；非空表示这条告警已经派过单（防重复建单）
ALTER TABLE alerts ADD COLUMN work_order_id INTEGER REFERENCES work_orders(id);
-- 派单依据：当班排班 SH-001（早班 08:00 - 16:00）/ 园区负责人 / 当天没有排班…
ALTER TABLE alerts ADD COLUMN assignee_note TEXT;

CREATE INDEX IF NOT EXISTS idx_alerts_work_order ON alerts(work_order_id);
