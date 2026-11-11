-- @migration-name: add_map_park

-- 地图归属园区（可选）。
--
-- 园区数据由 `business/` 独立业务平台维护，中台不建园区表、不做外键约束，
-- 只保存业务侧的园区编码与名称：
--   * 设备端建图回传时，业务平台会把设备绑定的园区随 `mapInfo` 一起带上；
--   * 业务平台从中台拉取地图时，可用 `parkCode` 反查该园区下的地图。
-- 历史地图没有园区信息，落为 NULL，不影响现有查询。
ALTER TABLE dog_map ADD COLUMN park_code TEXT;
ALTER TABLE dog_map ADD COLUMN park_name TEXT;

CREATE INDEX IF NOT EXISTS idx_dog_map_park_code ON dog_map(park_code);
