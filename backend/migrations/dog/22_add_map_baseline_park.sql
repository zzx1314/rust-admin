-- @migration-name: add_map_baseline
-- @if-not-exists: column dog_park.map_id

-- 园区持有「当前生效基线」指针：map_id 指向 dog_map 中已发布的版本，
-- map_status 记录标定状态（未绑定 / 待标定 / 已标定 / 部分标定）。
-- 未绑定基线的园区只作为资产台账，不允许编排巡检路线。

ALTER TABLE dog_park ADD COLUMN map_id INTEGER;
ALTER TABLE dog_park ADD COLUMN map_status TEXT NOT NULL DEFAULT '未绑定';

CREATE INDEX IF NOT EXISTS idx_dog_park_map ON dog_park(map_id);
