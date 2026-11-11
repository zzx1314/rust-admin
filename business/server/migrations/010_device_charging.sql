-- 充电状态：来自本体 1315 `kRobotStatus` 的 `charging_pile_connect` /
-- 电池 `power_supply_status1|2`（1=充电中 2=充满），由设备客户端心跳上报。
--
-- 业务平台以前用「电量 ≤ 10% 就判充电中」的猜测，会出现「没插电也显示充电中」
-- 或「真在充电但不显示」；有了这一列，「充电中」状态改为以本体上报为准。
ALTER TABLE devices ADD COLUMN charging INTEGER NOT NULL DEFAULT 0;
