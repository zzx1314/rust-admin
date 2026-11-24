-- @migration-name: publish_partial_maps

-- 「部分标定」的地图只要点过发布（`publish_time` 有值），状态改记为「已发布」。
--
-- 2026-11-24 改语义：发布 = 人工确认「这张图可以当基线用」，草稿 / 部分标定
-- 发布后都是「已发布」（以前发布对部分标定只写 publish_time、状态不变，现场看到
-- 页面还是部分标定，以为发布没生效）。
-- 这次把历史上「已经点过发布的部分标定」一并补成已发布，业务平台随
-- `sync_map_statuses` 同步为已发布 / 园区基线显示已标定。
-- 只认 publish_time 有值的行：没发布过的部分标定仍然是部分标定。
UPDATE dog_map
SET map_status = '已发布'
WHERE map_status = '部分标定'
  AND publish_time IS NOT NULL
  AND trim(publish_time) <> '';
