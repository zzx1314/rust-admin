-- 地图来源：区分本地基线、从根 backend 地图库导入的 ROS 地图等
ALTER TABLE map_versions ADD COLUMN source TEXT NOT NULL DEFAULT 'local';
ALTER TABLE map_versions ADD COLUMN source_id TEXT;
ALTER TABLE map_versions ADD COLUMN file_name TEXT;
ALTER TABLE map_versions ADD COLUMN file_path TEXT;
ALTER TABLE map_versions ADD COLUMN file_size INTEGER;

CREATE INDEX IF NOT EXISTS idx_map_versions_source ON map_versions(source, source_id);
