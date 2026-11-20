-- @migration-name: add_media_origin_fields
-- @if-not-exists: column dog_media.device_code

-- 媒体来源字段：让「媒体资源」页能看出这张图是哪台设备、哪次巡检计划的哪个航点
-- 采的，而不是只有一个孤零零的文件名。
--
-- 采集侧（板端 `/media/media-data/picture/<planId>/pt<n>/*.jpg`）由设备客户端在
-- 中台下发「同步媒体」指令后逐张回传：客户端 → 业务平台 → 中台
-- `POST /api/dogInternal/media/upload`。`sha256` 是去重键（同一张图重复回传只留一条），
-- 手工上传/历史数据的这些列留 NULL，页面按「手工上传」展示。
ALTER TABLE dog_media ADD COLUMN device_code TEXT;
ALTER TABLE dog_media ADD COLUMN park_code TEXT;
ALTER TABLE dog_media ADD COLUMN plan_id TEXT;
ALTER TABLE dog_media ADD COLUMN point_index INTEGER;
ALTER TABLE dog_media ADD COLUMN captured_at TEXT;
ALTER TABLE dog_media ADD COLUMN source TEXT;
ALTER TABLE dog_media ADD COLUMN sha256 TEXT;

CREATE INDEX IF NOT EXISTS idx_dog_media_device_code ON dog_media(device_code);
CREATE INDEX IF NOT EXISTS idx_dog_media_plan ON dog_media(plan_id, point_index);
CREATE UNIQUE INDEX IF NOT EXISTS uq_dog_media_sha256
    ON dog_media(sha256) WHERE sha256 IS NOT NULL AND is_deleted = 0;

-- 板端 AI 存证目录与每航点固定帧数：中台设备页「同步媒体」按钮据此组装指令参数
-- 下发给设备客户端（业务平台原样透传），改这里即改现场行为，不用登板子。
-- 板端 Agent 侧的 `MEDIA_DATA_DIR` / `DETECT_PLAN_SAVE_FRAMES` 是兜底默认值。
INSERT OR IGNORE INTO dog_config (id, config_name, config_value, description, create_time, update_time, is_deleted)
VALUES (9001, 'media_picture_dir', '/media/media-data/picture',
        '板端 AI 存证图片目录（设备「同步媒体」指令按此路径取图并回传中台）',
        strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'),
        strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0);

INSERT OR IGNORE INTO dog_config (id, config_name, config_value, description, create_time, update_time, is_deleted)
VALUES (9002, 'media_frames', '3',
        '每个航点固定保存/回传的帧数（1~10；板端不足时按实际帧数）',
        strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'),
        strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0);
