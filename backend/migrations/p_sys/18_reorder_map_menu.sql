-- @migration-name: reorder_map_menu

-- Sidebar order requested by the operator: 地图管理 sits right below
-- 系统管理 and above 设备列表.
--
-- Final first-level order (sort = sidebar rank):
--   1000 系统管理 1
--   1302 地图管理 2
--   1201 设备列表 3
--   1202 任务管理 4
--   1203 路线管理 5
--   1204 模型管理 6
--   1205 媒体资源 7
--   1206 指令管理 8
--   1207 参数配置 9
--   1208 服务管理 10
--   1301 园区列表 11

UPDATE p_sys_menu
SET sort = 2,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1302;

UPDATE p_sys_menu
SET sort = 3,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1201;

UPDATE p_sys_menu
SET sort = 11,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1301;
