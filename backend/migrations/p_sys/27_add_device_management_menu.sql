-- @migration-name: add_device_management_menu

-- 中台「设备管理」一级菜单（sort 2，排在系统管理之后、地图管理之前）。
-- 设备台账与用户分配是中台能力，业务平台只同步设备并读取分配结果。

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1210, '设备管理', null, null, '/dog/device/index', 'ri/robot-2-line', -1, 'dog/device', 2, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1220);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1220, '查询设备', null, 'dog_device_find', '', '', 1210, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);

INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1210);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1220);

-- 重新压缩现有菜单排序：设备管理插到地图管理之前。
UPDATE p_sys_menu
SET sort = 2,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1210;

UPDATE p_sys_menu
SET sort = 3,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1302;

UPDATE p_sys_menu
SET sort = 4,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1204;

UPDATE p_sys_menu
SET sort = 5,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1205;

UPDATE p_sys_menu
SET sort = 6,
    update_time = strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours')
WHERE id = 1207;
