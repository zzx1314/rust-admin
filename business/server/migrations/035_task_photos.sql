-- 航点拍照（路线动作「可见光拍照」）的回传与留存
--
-- 板端 Agent 在到达航点时用 ffmpeg 从本机 mediamtx 抓一张（或多张）JPEG
-- （见 `business/backpack_dashboard/backend/photo.py`），设备客户端上传成功才删板端文件；
-- 平台按任务留存，任务详情里直接看 / 下载。
--
-- 为什么不进中台「媒体资源」：那边是 AI 存证的**训练图**（按 sha256 去重、批量打 zip 导出），
-- 而巡检照片是任务执行结果：看的人、留存策略、删除时机都不一样（跟录像同一条理由）。
CREATE TABLE IF NOT EXISTS task_photos (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 冗余任务号：客户端只认 taskCode（板端不知道平台自增 id）
    task_id INTEGER,
    task_code TEXT NOT NULL,
    device_id INTEGER,
    park_id INTEGER,
    device_code TEXT NOT NULL DEFAULT '',
    plan_id TEXT,
    -- 航点下标（0 基，与 routeData.points / 5009 detail_index 一致）；-1 = 未知
    point_index INTEGER NOT NULL DEFAULT -1,
    point_name TEXT,
    -- 抓拍源：dogcam（机器狗前置可见光）/ cam0..cam3（背包四路 USB）
    camera TEXT NOT NULL DEFAULT 'dogcam',
    file_name TEXT NOT NULL,
    -- 相对 photo_dir 的路径，接口不回绝对目录
    file_path TEXT NOT NULL,
    file_size INTEGER NOT NULL DEFAULT 0,
    -- 板端抓拍时刻（板子本地时间，`YYYY-MM-DD HH:MM:SS`）
    captured_at TEXT,
    -- 重传去重：同一张图传两次不新增行
    sha256 TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_photos_task
    ON task_photos (task_id, created_at);
CREATE INDEX IF NOT EXISTS idx_task_photos_code
    ON task_photos (task_code, created_at);
-- 普通唯一索引（正式机 sqlite3 3.7 不认 partial index）
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_photos_sha
    ON task_photos (sha256);
