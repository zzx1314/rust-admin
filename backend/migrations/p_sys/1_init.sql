-- 用户表
CREATE TABLE IF NOT EXISTS p_sys_user (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL,
    email TEXT,
    phone TEXT,
    password TEXT,
    org_id TEXT,
    lock_time TEXT,
    last_login_time TEXT,
    try_count INTEGER DEFAULT 0,
    lock_flag INTEGER DEFAULT 1,
    create_time TEXT NOT NULL,
    update_time TEXT NOT NULL,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT,
    real_name TEXT,
    pass_update_time TEXT,
    card TEXT,
    is_show INTEGER DEFAULT 1,
    enable INTEGER DEFAULT 1,
    first_login INTEGER DEFAULT 1,
    sex TEXT
);

-- 角色表
CREATE TABLE IF NOT EXISTS p_sys_role (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    code TEXT,
    create_time TEXT,
    update_time TEXT,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT,
    description TEXT,
    is_edit INTEGER DEFAULT 1,
    ds_type INTEGER,
    ds_scope TEXT
);

-- 用户角色关联表
CREATE TABLE IF NOT EXISTS p_sys_user_role (
    user_id TEXT NOT NULL,
    role_id TEXT NOT NULL,
    PRIMARY KEY (user_id, role_id)
);

-- 菜单表
CREATE TABLE IF NOT EXISTS p_sys_menu (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    code TEXT,
    permission TEXT,
    path_url TEXT,
    icon TEXT,
    parent_id TEXT,
    component TEXT,
    sort INTEGER DEFAULT 0,
    keep_alive INTEGER DEFAULT 0,
    type INTEGER DEFAULT 0,
    create_time TEXT,
    update_time TEXT,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT,
    leaf INTEGER DEFAULT 0,
    role_code TEXT,
    disabled INTEGER DEFAULT 0,
    find_auth_id TEXT
);

-- 角色菜单关联表
CREATE TABLE IF NOT EXISTS p_sys_role_menu (
    role_id TEXT NOT NULL,
    menu_id TEXT NOT NULL,
    PRIMARY KEY (role_id, menu_id)
);

-- 字典表
CREATE TABLE IF NOT EXISTS p_sys_dict (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    dict_type TEXT,
    description TEXT,
    remarks TEXT,
    create_time TEXT,
    update_time TEXT,
    is_deleted INTEGER DEFAULT 0,
    allow_deletion INTEGER DEFAULT 1,
    is_show INTEGER DEFAULT 1
);

-- 字典项表
CREATE TABLE IF NOT EXISTS p_sys_dict_item (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    label TEXT,
    dict_id TEXT,
    value TEXT,
    sort INTEGER DEFAULT 0,
    description TEXT,
    create_time TEXT,
    update_time TEXT,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT,
    allow_deletion INTEGER DEFAULT 1
);

-- 组织表
CREATE TABLE IF NOT EXISTS p_sys_org (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    sort INTEGER DEFAULT 0,
    parent_id TEXT,
    create_time TEXT,
    update_time TEXT,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT,
    org_duty TEXT,
    desrc TEXT,
    type TEXT,
    parent_name TEXT
);
