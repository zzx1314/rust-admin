-- 任务进度时间戳：只在 progress 数值真正变化时刷新。
--
-- 任务看门狗原来只看 `updated_at`（任何一次进度上报都会刷新），客户端卡在
-- 0% 时仍然每 30s 报一次「执行中」，任务就永远不会被判超时。有了这一列，
-- 看门狗可以区分「有条上报但进度不动」和「正常在推进」。
ALTER TABLE tasks ADD COLUMN progress_updated_at TEXT;

-- 历史行按最后一次更新时间回填，避免升级后第一次扫描把在跑的任务全判异常
UPDATE tasks SET progress_updated_at = updated_at;
