-- 巡检录像（航点「录像采集」/ 航段「全程录像」）
--
-- 板端 Agent 用 ffmpeg 落 mp4（见 `business/backpack_dashboard/backend/record.py`），
-- 设备客户端上传成功才删板端文件；平台按任务留存，任务详情里可直接播放 / 下载。
--
-- 为什么不进中台「媒体资源」：那边是 AI 存证**训练图**（JPG/PNG、按 sha256 去重、
-- 批量导出打 zip），视频是任务执行结果，看的人、留存策略、体积都不是一路东西。
CREATE TABLE IF NOT EXISTS task_recordings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 冗余任务号：客户端只认 taskCode（任务表在平台侧，板端不知道自增 id）
    task_id INTEGER,
    task_code TEXT NOT NULL,
    device_id INTEGER,
    park_id INTEGER,
    device_code TEXT NOT NULL DEFAULT '',
    plan_id TEXT,
    -- -1 = 全程录像（没有具体航点）
    point_index INTEGER NOT NULL DEFAULT -1,
    point_name TEXT,
    -- point（航点录像） / task（航段全程录像）
    scope TEXT NOT NULL DEFAULT 'point',
    file_name TEXT NOT NULL,
    -- 相对 recording_dir 的路径，接口不回绝对目录
    file_path TEXT NOT NULL,
    file_size INTEGER NOT NULL DEFAULT 0,
    duration_s REAL NOT NULL DEFAULT 0,
    started_at TEXT,
    ended_at TEXT,
    -- 重传去重：同一段录像传两次不新增行
    sha256 TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_recordings_task
    ON task_recordings (task_id, created_at);
CREATE INDEX IF NOT EXISTS idx_task_recordings_code
    ON task_recordings (task_code, created_at);
-- 普通唯一索引（正式机 sqlite3 3.7 不认 partial index）
CREATE UNIQUE INDEX IF NOT EXISTS idx_task_recordings_sha
    ON task_recordings (sha256);
