-- @migration-name: remove_command_services_menus

-- 指令管理 / 服务管理不再由中台维护，菜单与按钮权限直接物理删除。
-- 保留：地图管理、模型管理、媒体资源、参数配置。

DELETE FROM p_sys_role_menu
WHERE menu_id IN (1206, 1208, 1216, 1218);

DELETE FROM p_sys_menu
WHERE id IN (1206, 1208, 1216, 1218);

-- 压缩保留的一级菜单排序。
UPDATE p_sys_menu
SET sort = 2,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1302;

UPDATE p_sys_menu
SET sort = 3,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1204;

UPDATE p_sys_menu
SET sort = 4,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1205;

UPDATE p_sys_menu
SET sort = 5,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1207;
