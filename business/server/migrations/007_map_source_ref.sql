-- 地图同步去重
--
-- `source_ref` 保存机器人本体的地图路径（4001 的 map_path 或 4012 的
-- map_history 绝对路径）。自动回传与中台手动同步都用它做幂等判断：
-- 同一个园区下相同 source_ref 只入库一次，避免重复上传同一张地图。

ALTER TABLE map_versions ADD COLUMN source_ref TEXT;

CREATE INDEX IF NOT EXISTS idx_map_versions_source_ref
    ON map_versions(park_id, source_ref);
