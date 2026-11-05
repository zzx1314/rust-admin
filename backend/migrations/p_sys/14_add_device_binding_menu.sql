-- @migration-name: add_device_binding_menu

-- Sidebar entry for the user/device binding page (child of 机器狗管理 1200).
--   1210 设备绑定 ── 1220 查询绑定

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1210, '设备绑定', null, null, '/dog/binding/index', 'ri/links-fill', 1200, 'dog/binding', 10, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1220);

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1220, '查询绑定', null, 'dog_user_device_find', '', '', 1210, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);

INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1210);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1220);
