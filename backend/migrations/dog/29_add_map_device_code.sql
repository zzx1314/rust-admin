-- @migration-name: add_map_device_code
-- @if-not-exists: column dog_map.device_code

-- 上报地图的设备编号（业务库 `devices.device_id`）。
--
-- 设备端建图完成后由业务平台随 `mapInfo.deviceCode` 回传，中台只做冗余存储，
-- 不建外键约束（设备台账由业务平台维护，经 `/api/dogInternal/devices/upsert` 同步）。
-- 前端地图列表据此显示「这张地图是哪台机器狗采的」，并支持按设备编号检索；
-- 手工上传的地图没有设备编号，落为 NULL。
ALTER TABLE dog_map ADD COLUMN device_code TEXT;

CREATE INDEX IF NOT EXISTS idx_dog_map_device_code ON dog_map(device_code);
