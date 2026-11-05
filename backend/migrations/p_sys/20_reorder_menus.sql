-- @migration-name: reorder_menus

-- Sidebar order requested by the operator:
--   园区列表 → after 系统管理 (above 地图管理)
--   任务管理 → after 路线管理
--
-- Final first-level order (sort = sidebar rank):
--   1000 系统管理 1
--   1301 园区列表 2
--   1302 地图管理 3
--   1201 设备列表 4
--   1203 路线管理 5
--   1202 任务管理 6
--   1204 模型管理 7
--   1205 媒体资源 8
--   1206 指令管理 9
--   1207 参数配置 10
--   1208 服务管理 11

UPDATE p_sys_menu
SET sort = 2,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1301;

UPDATE p_sys_menu
SET sort = 3,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1302;

UPDATE p_sys_menu
SET sort = 4,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1201;

UPDATE p_sys_menu
SET sort = 5,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1203;

UPDATE p_sys_menu
SET sort = 6,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1202;

UPDATE p_sys_menu
SET sort = 7,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1204;

UPDATE p_sys_menu
SET sort = 8,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1205;

UPDATE p_sys_menu
SET sort = 9,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1206;

UPDATE p_sys_menu
SET sort = 10,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1207;

UPDATE p_sys_menu
SET sort = 11,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1208;
