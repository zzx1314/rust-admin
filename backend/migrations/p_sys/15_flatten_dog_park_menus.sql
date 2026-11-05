-- @migration-name: flatten_dog_park_menus

-- 机器狗管理 / 园区管理 used to be sidebar directories (1200, 1300) whose
-- children were the actual pages. The pages are now promoted to first-level
-- menus, so the two directories are retired.
--
-- Resulting top-level layout (sort = sidebar rank):
--   1201 设备列表  sort 2
--   1210 设备绑定  sort 3（由 `remove_device_binding_menu` 下线）
--   1202 任务管理  sort 4
--   1203 路线管理  sort 5
--   1204 模型管理  sort 6
--   1205 媒体资源  sort 7
--   1206 指令管理  sort 8
--   1207 参数配置  sort 9
--   1208 服务管理  sort 10
--   1209 工作流    sort 11（由 `remove_workflow_menu` 下线）
--   1301 园区列表  sort 12
--   1302 地图管理  sort 13
--
-- Route paths (`/dog/device/index`, `/dogPark/park/index`, ...) and component
-- names are unchanged, so existing bookmarks and cached tabs keep working.
-- `UPDATE` by id keeps this safe to re-run.

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 2,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1201;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 3,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1210;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 4,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1202;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 5,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1203;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 6,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1204;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 7,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1205;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 8,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1206;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 9,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1207;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 10,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1208;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 11,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1209;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 12,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1301;

UPDATE p_sys_menu
SET parent_id = -1,
    sort = 13,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1302;

-- Retire the now-empty directories (1200 机器狗管理, 1300 园区管理).
UPDATE p_sys_menu
SET is_deleted = 1,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id IN (1200, 1300);

DELETE FROM p_sys_role_menu
WHERE menu_id IN (1200, 1300);
