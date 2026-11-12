-- 地图归属：从「园区」改为「设备」
--
-- 现场口径：机器人把本体上的地图回传到业务平台时，只跟「哪台设备采的图」有关；
-- 设备绑定人（中台 dog_user_device）登录后创建园区时，再从自己的设备地图里挑一张
-- 绑定为园区基线。所以：
--   - `map_versions.device_code` 记录采集设备编号（与 devices.device_id 对应）；
--   - `map_versions.park_id` 可为空，表示这张图还没有归属园区；
--   - 设备未绑定园区也可以同步地图 / 自动回传，园区在创建后再挑图绑定。
--
-- SQLite 不支持删除列上的 NOT NULL，只能重建表；历史行保留原园区归属，
-- device_code 留空（设备地图去重按 source_ref 兜底，见 ingest / synced-sources）。

CREATE TABLE map_versions_new (
    id TEXT PRIMARY KEY,
    park_id INTEGER,
    device_code TEXT,
    map_name TEXT NOT NULL,
    version TEXT NOT NULL,
    frame_id TEXT,
    crs TEXT,
    origin TEXT,
    resolution TEXT,
    floors TEXT,
    road_count INTEGER NOT NULL DEFAULT 0,
    no_go_count INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT '已发布',
    map_hash TEXT,
    map_width INTEGER,
    map_height INTEGER,
    note TEXT,
    published_at TEXT,
    created_at TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'local',
    source_id TEXT,
    file_name TEXT,
    file_path TEXT,
    file_size INTEGER,
    owner_user_id INTEGER,
    owner_username TEXT,
    source_ref TEXT
);

INSERT INTO map_versions_new (id, park_id, device_code, map_name, version, frame_id, crs, origin,
    resolution, floors, road_count, no_go_count, status, map_hash, map_width, map_height, note,
    published_at, created_at, source, source_id, file_name, file_path, file_size,
    owner_user_id, owner_username, source_ref)
SELECT id, park_id, NULL, map_name, version, frame_id, crs, origin, resolution, floors,
    road_count, no_go_count, status, map_hash, map_width, map_height, note, published_at,
    created_at, source, source_id, file_name, file_path, file_size,
    owner_user_id, owner_username, source_ref
FROM map_versions;

DROP TABLE map_versions;
ALTER TABLE map_versions_new RENAME TO map_versions;

CREATE INDEX IF NOT EXISTS idx_map_versions_park ON map_versions(park_id);
CREATE INDEX IF NOT EXISTS idx_map_versions_source ON map_versions(source, source_id);
CREATE INDEX IF NOT EXISTS idx_map_versions_source_ref
    ON map_versions(park_id, source_ref);
-- 设备地图去重：同一台设备的本体路径只入库一次（自动回传 / 手动同步共用）
CREATE INDEX IF NOT EXISTS idx_map_versions_device_ref
    ON map_versions(device_code, source_ref);
CREATE INDEX IF NOT EXISTS idx_map_versions_owner ON map_versions(owner_user_id);
