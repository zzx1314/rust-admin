-- @migration-name: add_map_baseline
-- @if-not-exists: column dog_map.map_version

-- 地图基线：地图从「文件台账」升级为「不可变坐标系版本」。
-- 每个版本记录 frame_id / CRS / 原点 / 分辨率 / 楼层 / 语义图层统计，
-- 发布后由 dog_park.map_id 指向当前生效版本；旧版本只归档不删除。

ALTER TABLE dog_map ADD COLUMN map_version TEXT;
ALTER TABLE dog_map ADD COLUMN frame_id TEXT;
ALTER TABLE dog_map ADD COLUMN crs TEXT;
ALTER TABLE dog_map ADD COLUMN map_origin TEXT;
ALTER TABLE dog_map ADD COLUMN resolution TEXT;
ALTER TABLE dog_map ADD COLUMN floors TEXT;
ALTER TABLE dog_map ADD COLUMN road_count INTEGER;
ALTER TABLE dog_map ADD COLUMN nogo_count INTEGER;
ALTER TABLE dog_map ADD COLUMN map_status TEXT NOT NULL DEFAULT '草稿';
ALTER TABLE dog_map ADD COLUMN map_hash TEXT;
ALTER TABLE dog_map ADD COLUMN publish_time TEXT;

CREATE INDEX IF NOT EXISTS idx_dog_map_status ON dog_map(map_status);
