-- @migration-name: add_map_dimensions
-- @if-not-exists: column dog_map.map_width

-- 激光雷达建图包上传时由后端解析 PGM/YAML，回写栅格像素尺寸，
-- 供路线边界校验与前端展示；分辨率 / 原点 / map_hash 已在上一条迁移中。

ALTER TABLE dog_map ADD COLUMN map_width INTEGER;
ALTER TABLE dog_map ADD COLUMN map_height INTEGER;
