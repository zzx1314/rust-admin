-- 巡检录像增加「录像源相机」
--
-- 路线动作里的「录像采集 / 全程录像」现在可以多选相机（`recordCameras`）：
-- `dogcam`（机器狗前置 / 正前）、`cam0`~`cam3`（背包四路 USB：左前 / 右前 /
-- 右后 / 左后）、`dog-ipcam`（红外）。板端每一路一个 mp4，客户端逐段回传，
-- 平台这里记下每段是哪一路——否则同一航点几段录像在任务详情里分不清。
--
-- 老数据 / 老客户端不带这个字段：默认 `dogcam`（那时候板端只录默认源）。
ALTER TABLE task_recordings ADD COLUMN camera TEXT NOT NULL DEFAULT 'dogcam';
