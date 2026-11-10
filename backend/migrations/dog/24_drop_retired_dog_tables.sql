-- @migration-name: drop_retired_dog_tables

-- 园区 / 设备 / 路线 / 任务 / 用户设备绑定 / 工作流已移交 `business/`
-- 独立服务，根后台不再持有这些数据，直接删除表（不做软删除）。
-- 地图（dog_map）保留，但不再归属园区，因此重建表以移除 park_id。

DROP TABLE IF EXISTS dog_user_device;
DROP TABLE IF EXISTS dog_workflow;
DROP TABLE IF EXISTS dog_route_library;
DROP TABLE IF EXISTS dog_task;
DROP TABLE IF EXISTS dog_device;
DROP TABLE IF EXISTS dog_park;

DROP INDEX IF EXISTS idx_dog_map_park;

CREATE TABLE dog_map_new (
    id INTEGER PRIMARY KEY,
    map_name TEXT NOT NULL,
    map_type TEXT,
    map_data TEXT,
    user_id INTEGER NOT NULL,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0,
    file_name TEXT,
    file_path TEXT,
    file_format TEXT,
    file_size INTEGER,
    remarks TEXT,
    map_version TEXT,
    frame_id TEXT,
    crs TEXT,
    map_origin TEXT,
    resolution TEXT,
    floors TEXT,
    road_count INTEGER,
    nogo_count INTEGER,
    map_status TEXT NOT NULL DEFAULT '草稿',
    map_hash TEXT,
    publish_time TEXT,
    map_width INTEGER,
    map_height INTEGER
);

INSERT INTO dog_map_new (
    id,
    map_name,
    map_type,
    map_data,
    user_id,
    create_time,
    update_time,
    is_deleted,
    file_name,
    file_path,
    file_format,
    file_size,
    remarks,
    map_version,
    frame_id,
    crs,
    map_origin,
    resolution,
    floors,
    road_count,
    nogo_count,
    map_status,
    map_hash,
    publish_time,
    map_width,
    map_height
)
SELECT
    id,
    map_name,
    map_type,
    map_data,
    user_id,
    create_time,
    update_time,
    is_deleted,
    file_name,
    file_path,
    file_format,
    file_size,
    remarks,
    map_version,
    frame_id,
    crs,
    map_origin,
    resolution,
    floors,
    road_count,
    nogo_count,
    map_status,
    map_hash,
    publish_time,
    map_width,
    map_height
FROM dog_map;

DROP TABLE dog_map;

ALTER TABLE dog_map_new RENAME TO dog_map;

CREATE INDEX IF NOT EXISTS idx_dog_map_user ON dog_map(user_id);
CREATE INDEX IF NOT EXISTS idx_dog_map_status ON dog_map(map_status);
