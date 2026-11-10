-- @migration-name: remove_retired_dog_menus

-- 园区 / 设备 / 路线 / 任务（以及依赖任务的工作流、用户设备绑定）已移交
-- `business/` 独立服务，根后台不再提供这些菜单。这里直接物理删除菜单与
-- 权限行，不保留 is_deleted 软删除记录。
--
-- 保留：模型管理、媒体资源、指令管理、参数配置、服务管理、地图管理。

-- 1. 撤掉角色授权（含历史软删除菜单遗留的授权）。
DELETE FROM p_sys_role_menu
WHERE menu_id IN (
    1200, 1201, 1202, 1203, 1209, 1210, 1211, 1212, 1213, 1219, 1220,
    1300, 1301, 1311
);

DELETE FROM p_sys_role_menu
WHERE menu_id NOT IN (SELECT id FROM p_sys_menu);

-- 2. 物理删除菜单行（页面 + 查询权限）。
DELETE FROM p_sys_menu
WHERE id IN (
    1200, 1201, 1202, 1203, 1209, 1210, 1211, 1212, 1213, 1219, 1220,
    1300, 1301, 1311
);

-- 3. 压缩剩余一级菜单的排序，保持侧边栏紧凑。
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
WHERE id = 1206;

UPDATE p_sys_menu
SET sort = 6,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1207;

UPDATE p_sys_menu
SET sort = 7,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1208;
