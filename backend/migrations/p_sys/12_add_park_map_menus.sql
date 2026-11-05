-- @migration-name: add_park_map_menus

-- Sidebar entries for the park/map pages:
--   1300 园区管理 (directory)
--   ├── 1301 园区列表 ── 1311 查询园区
--   └── 1302 地图管理 ── 1312 查询地图

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1300, '园区管理', null, null, '/dogPark', 'ri/building-2-line', -1, '', 3, 0, 1, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 0, '110', null, null);

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1301, '园区列表', null, null, '/dogPark/park/index', 'ri/building-2-line', 1300, 'dog/park', 1, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1311);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1302, '地图管理', null, null, '/dogPark/map/index', 'ri/map-2-line', 1300, 'dog/map', 2, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1312);

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1311, '查询园区', null, 'dog_park_find', '', '', 1301, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1312, '查询地图', null, 'dog_map_find', '', '', 1302, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);

INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1300);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1301);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1302);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1311);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1312);
