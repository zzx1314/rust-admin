-- 本体健康状态：来自设备客户端心跳（智航 1315 / 1207）。
--
-- 现场「平台显示在线待命、指令发出去车不动」的两个直接原因：
-- 1. `sdk_node_available = 0`：本体算法节点未就绪，1302/1304/5000 会被拒；
-- 2. `estop_software = 1`：软件急停未解除，所有运动指令会被拒。
--
-- 两列都允许 NULL：NULL = 客户端尚未上报（老版本客户端），0/1 = 明确状态。
ALTER TABLE devices ADD COLUMN estop_software INTEGER;
ALTER TABLE devices ADD COLUMN sdk_node_available INTEGER;
