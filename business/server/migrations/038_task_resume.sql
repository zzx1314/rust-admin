-- 中断续跑（修复后从断点继续，已走过的航点不再执行）
--
-- `last_waypoint_index`：最近一次上报的「已到达航点」0 基下标（客户端 `waypointIndex`
-- = 5009 的 `detail_index`）。看门狗 / 客户端报异常时靠它算续跑起点。
--
-- `resume_index`：下次下发从这个航点（0 基，`routeData.points` 下标）开始。
-- NULL = 从起点跑（默认 / 上一次已跑完）。
--
-- 只有人工「从断点继续」（`POST /tasks/{id}/dispatch` 带 `resume`）才会把它带进
-- 下发载荷（`startIndex`）：自动调度 / 拖回待执行 / 重新下发一律从起点跑，
-- 免得「昨天没跑完」在无人值守时被自动接上（现场要先确认故障已修好）。
ALTER TABLE tasks ADD COLUMN last_waypoint_index INTEGER;
ALTER TABLE tasks ADD COLUMN resume_index INTEGER;
