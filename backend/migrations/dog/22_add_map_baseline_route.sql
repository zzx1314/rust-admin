-- @migration-name: add_map_baseline
-- @if-not-exists: column dog_route_library.map_id

-- 路线创建时冻结地图版本引用：map_id + map_version + map_frame 快照。
-- 地图发布新版本不会自动改写历史路线坐标，必须人工做影响分析后再迁移。

ALTER TABLE dog_route_library ADD COLUMN map_id INTEGER;
ALTER TABLE dog_route_library ADD COLUMN map_version TEXT;
ALTER TABLE dog_route_library ADD COLUMN map_frame TEXT;

CREATE INDEX IF NOT EXISTS idx_dog_route_library_map ON dog_route_library(map_id);
