-- 设备实时视频与远程接管
--
-- 视频：智航 Pro 协议没有视频接口，视频流由本体自带的服务（RTSP/HLS/WebRTC）提供，
-- 后台只保存地址与协议类型，前端按类型选择播放方式。
--
-- 接管：遥控直接驱动机器狗，必须先「接管」才能下发摇杆与动作指令。
-- teleop_holder 记录当前接管人，teleop_at 记录接管时间用于超时自动释放。
ALTER TABLE devices ADD COLUMN video_url TEXT;
ALTER TABLE devices ADD COLUMN video_type TEXT NOT NULL DEFAULT 'none';
ALTER TABLE devices ADD COLUMN teleop_holder TEXT;
ALTER TABLE devices ADD COLUMN teleop_at TEXT;

CREATE INDEX IF NOT EXISTS idx_devices_teleop ON devices(teleop_holder);
