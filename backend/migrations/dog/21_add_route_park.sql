-- @migration-name: add_route_park
-- @if-not-exists: column dog_route_library.park_id

-- 路线归属园区：路线管理新增「所属园区」字段，与 dog_park 关联。
-- 历史路线保留 NULL，前端列表 / 新增 / 修改均可选择园区。

ALTER TABLE dog_route_library ADD COLUMN park_id INTEGER;

CREATE INDEX IF NOT EXISTS idx_dog_route_library_park ON dog_route_library(park_id);
