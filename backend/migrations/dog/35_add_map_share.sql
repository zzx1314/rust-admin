-- @migration-name: add_map_share

-- 地图分享：一张中台地图可以共享给多台设备使用，不必为每台设备重新建图。
--
-- `dog_map.device_code` 仍然只记「上报（采集）这张图的设备」，分享关系落在
-- `dog_map_share`（多对多）。分享后：
-- - 分配给目标设备的中台用户也能在地图列表里看到这张图（不用重新上传）；
-- - 「下发到狗体」可以把这张图下发到被分享的设备；
-- - 「设备编号」筛选同时匹配上报设备与分享设备。
--
-- 唯一索引覆盖软删除行：再次分享同一台设备时恢复旧行，而不是插入重复记录
-- （与 `dog_user_device` 的做法一致）。
CREATE TABLE IF NOT EXISTS dog_map_share (
    id INTEGER PRIMARY KEY,
    map_id INTEGER NOT NULL,
    device_code TEXT NOT NULL,
    user_id INTEGER,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_dog_map_share ON dog_map_share(map_id, device_code);
CREATE INDEX IF NOT EXISTS idx_dog_map_share_device ON dog_map_share(device_code);
