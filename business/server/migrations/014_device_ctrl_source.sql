-- 控制源与运动状态：来自设备客户端心跳（智航 1315 `ctrl_source` / `motion_status`）。
--
-- 「用平台接管后摇杆却走不动」在字段上只有两种可能，以前页面上都看不见：
-- 1. `ctrl_source = 3`：硬件遥控器（或其它 SDK）仍压着控制权，平台的 1304 被忽略；
-- 2. `motion_status = lie_down`：本体卧倒，摇杆自然不动，得先站立。
ALTER TABLE devices ADD COLUMN ctrl_source INTEGER NOT NULL DEFAULT 0;
ALTER TABLE devices ADD COLUMN motion_status TEXT;
