-- 本体工作模式：来自 1315 `kRobotStatus` 的 `mode`（协议里的 MachineStatus，
-- 网关以字符串下发，现场实测为 `remote`），由设备客户端心跳上报。
--
-- 同时废弃 `devices.signal`：智航协议里没有信号强度指标（1315 的 39 个字段逐一核对过，
-- 没有 rssi/csq/wifi/lte 之类），客户端已不再上报，业务平台也不再展示。
-- 历史列保留（SQLite 删列会重写表），新数据写默认值 0，不再对外暴露。
ALTER TABLE devices ADD COLUMN robot_mode TEXT;
