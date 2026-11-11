-- 设备运行态数据来源标记
--
-- 设备客户端在心跳里带上两项，业务平台据此区分「真机数据」与「模拟数据」：
--   body_online = 1  本体链路在线，电量 / 温度 / 里程是本体 1315 上报的真实值
--   body_online = 0  本体没接上（模拟模式 / 本体掉线），这些数值不可信
-- run_mode 记录客户端当前工作模式：robot（接本体）/ sim（纯模拟）。
--
-- 历史数据默认 0：升级前的心跳没有这两项，宁可显示「—」也不要展示假数据。
ALTER TABLE devices ADD COLUMN body_online INTEGER NOT NULL DEFAULT 0;
ALTER TABLE devices ADD COLUMN run_mode TEXT;
