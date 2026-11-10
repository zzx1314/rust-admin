-- 地图基线：园区建档时绑定的坐标系定义，版本不可变
CREATE TABLE IF NOT EXISTS map_versions (
    id TEXT PRIMARY KEY,
    park_id INTEGER NOT NULL,
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
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_map_versions_park ON map_versions(park_id);

-- 园区只持有一个「当前生效基线」指针
ALTER TABLE parks ADD COLUMN map_version_id TEXT;
ALTER TABLE parks ADD COLUMN map_status TEXT NOT NULL DEFAULT '待标定';

-- 路线创建时冻结一个地图版本引用，之后不随园区当前基线变化
ALTER TABLE routes ADD COLUMN map_version_id TEXT;
CREATE INDEX IF NOT EXISTS idx_routes_map ON routes(map_version_id);
