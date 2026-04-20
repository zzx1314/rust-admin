-- 用户初始化
INSERT INTO p_sys_user ( id, username, email, phone, password, org_id, lock_time, last_login_time, try_count, lock_flag, create_time, update_time, is_deleted, remarks, real_name, pass_update_time, card, is_show, enable, first_login, sex ) VALUES ( 1, 'sysadmin', null, null, '{MD5}afdd0b4ad2ec172c586e2150770fbf9e', 1, '2025-08-05 05:33:14', '2025-08-05 13:33:13', 0, 1, '2021-12-08 14:26:25', '2025-08-05 13:33:14', 0, null, '系统管理员', '2025-03-13 10:47:23', null, 1, 1, 1, '男' );

-- 角色初始化
INSERT INTO p_sys_role (id, name, code, create_time, update_time, is_deleted, remarks, description, is_edit, ds_type, ds_scope) VALUES (1, 'sysadm', '110', '2021-02-03 11:05:12', '2021-02-03 11:05:12', 0, '维护系统任务以及权限', '系统管理员', false, null, null);
INSERT INTO p_sys_role (id, name, code, create_time, update_time, is_deleted, remarks, description, is_edit, ds_type, ds_scope) VALUES (2, 'common', '111', '2025-07-16 20:25:50', '2021-02-03 11:05:12', 0, null, '普通人员', true, null, null);

-- 用户角色初始化
INSERT INTO p_sys_user_role (user_id, role_id ) VALUES ( 1, 1 );

-- 菜单初始化
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1000, '系统管理', null, null, '/system', 'ri/settings-3-line', -1, 'sys', 1, 0, 1, '2020-09-18 14:17:36', '2025-05-30 11:31:42', 0, null, false, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1002, '用户管理', null, '', '/system/sysUser/index', 'ri/admin-line', 1000, 'sysUser', 1, 0, 3, '2021-03-11 16:15:54', '2025-05-30 11:31:48', 0, null, true, '110', null, 1007);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1003, '组织管理', null, null, '/system/sysOrg/index', 'ri/git-branch-line', 1000, 'sysOrg', 2, 0, 3, '2023-02-25 07:37:23', '2024-11-01 15:20:58', 0, null, true, '110', null, 1008);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1004, '角色管理', null, null, '/system/sysRole/index', 'ri/admin-fill', 1000, 'sysRole', 3, 0, 3, '2023-02-26 10:41:36', '2024-11-01 15:21:47', 0, null, true, '110', null, 1009);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1005, '菜单管理', null, null, '/system/sysMenu/index', 'ep/menu', 1000, 'sysMenu', 4, 0, 3, '2023-02-26 10:43:24', '2024-11-01 15:22:36', 0, null, true, '110', null, 1010);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1006, '权限管理', null, null, '/system/sysAuth/index', 'ep/lollipop', 1000, 'sysAuth', 5, 0, 3, '2023-03-02 15:44:17', '2024-11-01 15:23:32', 0, null, true, '110', null, 1011);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1007, '查询用户', null, 'user_find', null, null, 1002, '', 1, 0, 2, '2023-03-08 17:53:29', '2025-05-30 11:31:16', 0, null, false, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1008, '查询组织', null, 'org_find', null, null, 1003, '', 1, 0, 2, '2023-03-08 17:55:18', '2024-11-01 15:21:10', 0, null, false, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1009, '查询角色', null, 'role_find', null, null, 1004, '', 1, 0, 2, '2023-03-08 17:56:00', '2024-11-01 15:21:59', 0, null, false, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1010, '查询菜单', null, 'menu_find', null, null, 1005, '', 1, 0, 2, '2023-03-08 17:56:47', '2024-11-01 15:22:45', 0, null, false, '110', true, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1011, '查询权限', null, 'auth_find', null, null, 1006, '', 1, 0, 2, '2023-03-08 17:57:32', '2024-11-01 15:23:39', 0, null, false, '110', true, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1012, '添加用户', null, 'user_add', null, null, 1002, '', 2, 0, 2, '2023-03-13 10:49:59', '2025-05-30 11:31:22', 0, null, false, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1021, '修改用户', null, 'user_update', '', '', 1002, '', 3, 0, 2, '2024-08-07 17:09:36', '2024-11-01 15:20:35', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1022, '删除用户', null, 'user_del', '', '', 1002, '', 4, 0, 2, '2024-08-07 17:10:48', '2024-11-01 15:20:41', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1023, '添加组织', null, 'org_add', '', '', 1003, '', 2, 0, 2, '2024-08-07 17:11:56', '2024-11-01 15:21:17', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1024, '修改组织', null, 'org_update', '', '', 1003, '', 3, 0, 2, '2024-08-07 17:12:32', '2024-11-01 15:21:25', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1025, '删除组织', null, 'org_del', '', '', 1003, '', 4, 0, 2, '2024-08-07 17:13:07', '2024-11-01 15:21:31', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1026, '添加角色', null, 'role_add', '', '', 1004, '', 2, 0, 2, '2024-08-07 17:13:38', '2024-11-01 15:22:06', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1027, '修改角色', null, 'role_update', '', '', 1004, '', 1, 0, 2, '2024-08-07 17:14:05', '2024-11-01 15:22:14', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1028, '删除角色', null, 'role_del', '', '', 1004, '', 4, 0, 2, '2024-08-07 17:14:43', '2024-11-01 15:22:23', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1030, '修改菜单', null, 'menu_update', '', '', 1005, '', 3, 0, 2, '2024-08-07 17:16:40', '2024-11-01 15:23:13', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1032, '添加权限', null, 'auth_add', '', '', 1006, '', 2, 0, 2, '2024-08-07 17:18:36', '2024-11-01 15:23:46', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1033, '删除权限', null, 'auth_del', '', '', 1006, '', 3, 0, 2, '2024-08-07 17:19:17', '2024-11-01 15:23:54', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1034, '修改权限', null, 'auth_update', '', '', 1006, '', 4, 0, 2, '2024-08-07 17:19:52', '2024-11-01 15:24:00', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1043, '安全配置', null, '', '/system/sysSeting/index', 'tdesign/lock-on', 1000, 'sysSeting', 6, 0, 3, '2024-08-23 11:49:29', '2024-08-23 11:49:29', 0, null, true, '110', null, 1044);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1044, '查询安全配置', null, 'sys_seting_find', '', '', 1043, '', 1, 0, 2, '2024-08-23 11:59:22', '2024-09-11 11:30:23', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1052, '保存安全配置', null, 'sys_seting_save', '', '', 1043, '', 1, 0, 2, '2024-09-11 11:26:21', '2024-09-11 11:30:37', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1062, '重置密码', null, 're_set_pass', '', '', 1002, '', 1, 0, 2, '2024-09-11 14:13:44', '2024-09-11 14:13:44', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1066, '启用禁用', null, 'user_stop_start', '', '', 1002, '', 1, 0, 2, '2024-09-23 17:52:11', '2024-09-11 14:13:44', 0, null, true, '110', null, null);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1101, '日志管理', null, null, '/system/sysLog/index', 'ri/blogger-line', 1000, 'sysLog', 8, 0, 3, '2025-03-19 15:15:00', '2024-09-11 14:13:44', 0, null, true, '110', null, 1102);
INSERT INTO p_sys_menu (id, name, code, permission, path_url, icon, parent_id, component, sort, keep_alive, type, create_time, update_time, is_deleted, remarks, leaf, role_code, disabled, find_auth_id) VALUES (1102, '查询日志', null, 'log_find', null, null, 1101, '', 1, 0, 2, '2025-03-19 15:17:42', '2024-09-11 14:13:44', 0, null, true, '110', null, null);

-- 角色菜单初始化
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1024);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1025);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1026);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1027);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1028);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1030);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1032);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1033);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1034);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1101);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1102);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1043);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1044);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1052);


INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1062);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1000);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1002);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1066);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1003);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1004);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1005);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1006);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1007);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1008);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1009);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1010);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1011);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1012);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1021);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1022);
INSERT INTO p_sys_role_menu (role_id, menu_id) VALUES (1, 1023);


-- 字典初始化
INSERT INTO p_sys_dict (id, type, dict_type, description, remarks, create_time, update_time, is_deleted, allow_deletion, is_show) VALUES (28, 'sys_security_policy', '系统类', '安全策略配置', '安全策略配置', '2020-09-01 16:17:18', null, 0, true, true);


-- 字典项初始化
INSERT INTO p_sys_dict_item (id, type, label, dict_id, value, sort, description, create_time, update_time, is_deleted, remarks, allow_deletion) VALUES (126, 'sysLoginMaxLockTime', '锁定时常', 28, '5', 1, '锁定时常', '2020-09-08 17:02:43', null, 0, '锁定时常', null);
INSERT INTO p_sys_dict_item (id, type, label, dict_id, value, sort, description, create_time, update_time, is_deleted, remarks, allow_deletion) VALUES (127, 'sysLoginMaxTryCount', '最大尝试次数', 28, '5', 2, '最大尝试次数', '2020-09-08 17:03:12', null, 0, '最大尝试次数', null);
INSERT INTO p_sys_dict_item (id, type, label, dict_id, value, sort, description, create_time, update_time, is_deleted, remarks, allow_deletion) VALUES (128, 'sysPassLength', '密码长度', 28, '13', 3, '密码长度', '2020-09-08 17:04:04', null, 0, '密码长度', null);
INSERT INTO p_sys_dict_item (id, type, label, dict_id, value, sort, description, create_time, update_time, is_deleted, remarks, allow_deletion) VALUES (129, 'sysPassChange', '密码更换周期', 28, '30', 15, '密码更换周期', '2020-09-08 17:04:28', null, 0, '密码更换周期', null);
INSERT INTO p_sys_dict_item (id, type, label, dict_id, value, sort, description, create_time, update_time, is_deleted, remarks, allow_deletion) VALUES (130, 'sysOvertime', '超时时间', 28, '900', 5, '超时时间', '2020-09-08 17:05:01', null, 0, '超时时间', null);
INSERT INTO p_sys_dict_item (id, type, label, dict_id, value, sort, description, create_time, update_time, is_deleted, remarks, allow_deletion) VALUES (131, 'sysPassShortLength', '密码最短长度', 28, '8', 10, '密码最短长度', '2020-10-14 11:05:58', '2020-11-26 10:16:08', 0, '密码最短长度', null);
INSERT INTO p_sys_dict_item (id, type, label, dict_id, value, sort, description, create_time, update_time, is_deleted, remarks, allow_deletion) VALUES (132, 'passCom', '密码复杂度', 28, '2', 7, '密码复杂度', '2020-10-14 11:06:37', null, 0, '密码复杂度', null);

--初始化组织
INSERT INTO p_sys_org (id, name, sort, parent_id, create_time, update_time, is_deleted, remarks, org_duty, desrc, type, parent_name) VALUES (1, '华郅集团', 1, 0, '2022-10-25 16:58:38', '2024-09-04 17:16:47', 0, '', '', '', 'top', '');
INSERT INTO p_sys_org (id, name, sort, parent_id, create_time, update_time, is_deleted, remarks, org_duty, desrc, type, parent_name) VALUES (2, '华郅北京公司', 3, 1, '2022-10-25 17:30:16', '2025-02-13 17:47:16', 0, '北京分公司', '', '', 'company', '华郅集团');
INSERT INTO p_sys_org (id, name, sort, parent_id, create_time, update_time, is_deleted, remarks, org_duty, desrc, type, parent_name) VALUES (3, '华郅西安公司', 3, 1, '2022-11-09 09:58:59', '2024-09-05 09:39:33', 0, '备注', '', '', 'company', '顶部门');
INSERT INTO p_sys_org (id, name, sort, parent_id, create_time, update_time, is_deleted, remarks, org_duty, desrc, type, parent_name) VALUES (18, '销售部-能源', 3, 3, '2025-01-13 16:31:17', '2024-09-05 09:39:33', 0, '专门负责华东和江南地区能源领域销售推广', null, null, 'common', '');
INSERT INTO p_sys_org (id, name, sort, parent_id, create_time, update_time, is_deleted, remarks, org_duty, desrc, type, parent_name) VALUES (19, '华郅成都公司', 1, 1, '2025-01-19 13:34:01', '2025-01-19 13:36:00', 0, '香港分公司 负责香港地区业务', null, null, 'company', '华郅集团');
INSERT INTO p_sys_org (id, name, sort, parent_id, create_time, update_time, is_deleted, remarks, org_duty, desrc, type, parent_name) VALUES (22, '技术支持部', 1, 19, '2025-01-19 13:35:37', '2025-01-19 13:41:21', 0, '', null, null, 'common', '华郅香港公司');
INSERT INTO p_sys_org (id, name, sort, parent_id, create_time, update_time, is_deleted, remarks, org_duty, desrc, type, parent_name) VALUES (23, '方案开发部', 1, 2, '2025-03-05 15:51:04', '2025-03-05 15:51:22', 0, '负责对外客户的方案开发', null, null, 'common', '华郅北京公司');
