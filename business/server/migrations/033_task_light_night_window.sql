-- 计划级「仅夜间」的自定义时段：`light_night_start` / `light_night_end`（HH:MM）。
--
-- 以前夜间窗只能改板端环境变量（`LIGHT_NIGHT_START` / `LIGHT_NIGHT_END`，默认
-- 18:00-06:00），那是「这台板子所有计划共用一个窗」。现场按季节/园区作息要改时段时
-- 只能登板改配置、重启 Agent，没法按计划区分（例如夜巡 19:00-05:00、园区照明
-- 22:00-06:00）。
--
-- 分层不变：**平台只存时段，判夜仍在板端**（`control.is_night`，板子本地时间）。
-- 客户端把这两个值随 `POST /api/lights` 的 `nightStart`/`nightEnd` 传给 Agent；
-- 两个都为空 = 用板端默认（老计划 / 老客户端行为完全不变）。
--
-- 允许跨零点（22:00-06:00）；起止相同在接口层就 400（板端对这种值当「关闭夜间判定」，
-- 静默接受会让现场以为配了时段却永远不开灯）。
ALTER TABLE tasks ADD COLUMN light_night_start TEXT;
ALTER TABLE tasks ADD COLUMN light_night_end TEXT;
