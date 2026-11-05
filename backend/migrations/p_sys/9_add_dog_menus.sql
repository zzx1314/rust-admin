-- @migration-name: add_dog_menus

-- Menu entries for the robot-dog (dog) business module. Without them the new
-- `/views/dog/*` pages are unreachable from the sidebar even though the APIs
-- are registered.
--
-- Layout:
--   1200 机器狗管理 (directory)
--   ├── 1201 设备列表  ── 1211 查询设备
--   ├── 1202 任务管理  ── 1212 查询任务
--   ├── 1203 路线管理  ── 1213 查询路线
--   ├── 1204 模型管理  ── 1214 查询模型
--   ├── 1205 媒体资源  ── 1215 查询媒体
--   ├── 1206 指令管理  ── 1216 查询指令
--   ├── 1207 参数配置  ── 1217 查询配置
--   ├── 1208 服务管理  ── 1218 查询服务
--   └── 1209 工作流    ── 1219 查询工作流
--
-- `INSERT OR IGNORE` keeps this migration safe to re-run and avoids clashing
-- with menus already created through the admin UI.

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1200, '机器狗管理', null, null, '/dog', 'ri/robot-2-line', -1, '', 2, 0, 1, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 0, '110', null, null);

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1201, '设备列表', null, null, '/dog/device/index', 'ri/table-line', 1200, 'dog/device', 1, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1211);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1202, '任务管理', null, null, '/dog/task/index', 'ri/task-line', 1200, 'dog/task', 2, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1212);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1203, '路线管理', null, null, '/dog/route/index', 'ri/route-line', 1200, 'dog/route', 3, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1213);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1204, '模型管理', null, null, '/dog/model/index', 'ri/cpu-line', 1200, 'dog/model', 4, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1214);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1205, '媒体资源', null, null, '/dog/media/index', 'ri/image-line', 1200, 'dog/media', 5, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1215);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1206, '指令管理', null, null, '/dog/command/index', 'ri/terminal-window-line', 1200, 'dog/command', 6, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1216);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1207, '参数配置', null, null, '/dog/config/index', 'ri/settings-4-line', 1200, 'dog/config', 7, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1217);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1208, '服务管理', null, null, '/dog/services/index', 'ri/server-line', 1200, 'dog/services', 8, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1218);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1209, '工作流', null, null, '/dog/workflow/index', 'ri/flow-chart', 1200, 'dog/workflow', 9, 0, 3, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, 1219);

INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1211, '查询设备', null, 'dog_device_find', '', '', 1201, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1212, '查询任务', null, 'dog_task_find', '', '', 1202, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1213, '查询路线', null, 'dog_route_find', '', '', 1203, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1214, '查询模型', null, 'dog_model_find', '', '', 1204, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1215, '查询媒体', null, 'dog_media_find', '', '', 1205, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1216, '查询指令', null, 'dog_command_find', '', '', 1206, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1217, '查询配置', null, 'dog_config_find', '', '', 1207, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1218, '查询服务', null, 'dog_services_find', '', '', 1208, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);
INSERT OR IGNORE INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1219, '查询工作流', null, 'dog_workflow_find', '', '', 1209, '', 1, 0, 2, strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), strftime('%Y-%m-%d %H:%M:%S', 'now', '+8 hours'), 0, null, 1, '110', null, null);

-- Grant the new menus to the built-in super-admin role (role id 1), matching
-- how the other seeded menus are wired.
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1200);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1201);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1202);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1203);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1204);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1205);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1206);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1207);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1208);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1209);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1211);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1212);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1213);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1214);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1215);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1216);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1217);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1218);
INSERT OR IGNORE INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1219);
