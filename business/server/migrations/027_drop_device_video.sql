-- 设备档案不再存视频地址（2026-11-18）
--
-- 设备管理 / 计划管理 / 实时视频三处画面统一走背包前置 `dogcam`
-- （`web/src/components/FrontCameraVideo.vue`），
-- 「设备客户端 ffmpeg 拉 RTSP → 平台中继」的旧链路已整体删除：
-- 服务端 `/api/devices/{id}/video/ws`、客户端 `src/video.rs`、`[video]` 配置段都去掉了，
-- 这两列不再有任何代码读写，直接删掉，避免留成「看着能配、其实没用」的死字段。
ALTER TABLE devices DROP COLUMN video_url;
ALTER TABLE devices DROP COLUMN video_type;
