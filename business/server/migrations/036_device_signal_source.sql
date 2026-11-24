-- 设备联网信号的来源：`wifi`（板端 nmcli 的 SIGNAL）/ `cellular`（4G 的 AT+CSQ 折算）。
--
-- 为什么需要它：`devices.signal`（0-100）在 2026-11-25 起由「设备客户端读背包板子」
-- 上报，板端是 **WiFi 优先、没连 WiFi 时退回 4G**（`GET /api/network`）。只看数值
-- 分不清「WiFi 良」还是「4G 良」，而现场恰恰常把平台流量钉在 4G 上，排查掉线时这是
-- 关键信息。NULL = 老客户端 / 没连网（页面显示 `—`）。
ALTER TABLE devices ADD COLUMN signal_source TEXT;
