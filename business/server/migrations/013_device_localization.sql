-- 定位状态与当前生效地图：来自设备客户端心跳（智航 5009 `loc_state.status`
-- 与客户端 `active_map`）。
--
-- 平台以前只能从任务失败信息里间接猜到「定位丢了」，设备详情页看不出狗到底在哪张图上、
-- 定位是否收敛。有了这两列：
-- - `loc_status`：100 已收敛 / 121 重定位中 / 122 定位丢失 / 210 定位失败 …（255 未上报）
-- - `active_map_id`：机器人当前载入的业务地图版本号（如 `ROS-MAP-3@v1.0`），
--   下发「重新定位」时未显式指定地图就沿用它。
ALTER TABLE devices ADD COLUMN loc_status INTEGER NOT NULL DEFAULT 255;
ALTER TABLE devices ADD COLUMN active_map_id TEXT;
