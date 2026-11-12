-- @migration-name: backfill_map_version

-- 补齐历史地图的基线版本。
--
-- 机器人回传（4001 / 中台手动同步）与手工上传此前都不带版本，`dog_map.map_version`
-- 落成 NULL：地图列表的「基线版本」列一直空着，而编辑弹窗又把 NULL 显示成 v1.0，
-- 看起来像字段丢了。现在新建地图默认写 v1.0，这里把历史空值一并补齐，
-- 让列表、编辑弹窗与派生新版本（v1.0 → v1.1）的起点一致。
UPDATE dog_map
SET map_version = 'v1.0'
WHERE map_version IS NULL OR trim(map_version) = '';
