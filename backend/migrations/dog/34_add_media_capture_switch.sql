-- @migration-name: add_media_capture_switch

-- 航点自动采集总开关（`media_capture_enabled`，默认 **关**）。
--
-- 以前巡检到点在命中时除了写告警证据图，还会把 N 帧原始图写进
-- `media_picture_dir`（`media_frames`，训练数据），没有开关：只要跑计划就在攒图，
-- 板子磁盘和「同步媒体」都得跟着处理。这里补一个开关，默认关：
--
-- - 关（默认）：板端只写 `detect_jobs/<计划>/pt<n>/` 告警证据图，数量核对、
--   企业微信推送、AI 告警全部照旧，只是不再往媒体归档目录落图；
-- - 开：恢复原行为（每次命中固定存 `media_frames` 帧到 `media_picture_dir`），
--   要攒训练数据时在「参数配置」页改成 `1` / `true` 即可，最多 1 分钟生效。
--
-- 只加配置行，不动历史数据：已经回传到中台的图照旧留在 `dog_media` 里。
INSERT OR IGNORE INTO dog_config (id, config_name, config_value, description, create_time, update_time, is_deleted)
VALUES (9003, 'media_capture_enabled', '0',
        '巡检到点是否自动采集原图到媒体归档目录（1/true=开，0=false=关；默认关，关了不影响告警证据图与数量核对）',
        strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'),
        strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0);
