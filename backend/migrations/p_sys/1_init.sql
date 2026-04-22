-- 用户表
CREATE TABLE IF NOT EXISTS p_sys_user (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL,
    email TEXT,
    phone TEXT,
    password TEXT,
    org_id INTEGER,
    lock_time TEXT,
    last_login_time TEXT,
    try_count INTEGER DEFAULT 0,
    lock_flag INTEGER DEFAULT 1,
    create_time TEXT NOT NULL,
    update_time TEXT,
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
    id INTEGER PRIMARY KEY,
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
    user_id INTEGER NOT NULL,
    role_id INTEGER NOT NULL,
    PRIMARY KEY (user_id, role_id)
);

-- 菜单表
CREATE TABLE IF NOT EXISTS p_sys_menu (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    code TEXT,
    permission TEXT,
    path_url TEXT,
    icon TEXT,
    parent_id INTEGER,
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
    find_auth_id INTEGER
);

-- 角色菜单关联表
CREATE TABLE IF NOT EXISTS p_sys_role_menu (
    role_id INTEGER NOT NULL,
    menu_id INTEGER NOT NULL,
    PRIMARY KEY (role_id, menu_id)
);

-- 字典表
CREATE TABLE IF NOT EXISTS p_sys_dict (
    id INTEGER PRIMARY KEY,
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
    id INTEGER PRIMARY KEY,
    type TEXT NOT NULL,
    label TEXT,
    dict_id INTEGER,
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
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    sort INTEGER DEFAULT 0,
    parent_id INTEGER,
    create_time TEXT,
    update_time TEXT,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT,
    org_duty TEXT,
    desrc TEXT,
    type TEXT,
    parent_name TEXT
);

-- 日志表
CREATE TABLE IF NOT EXISTS p_sys_logrecord (
    id INTEGER PRIMARY KEY,
    tenant TEXT,
    type TEXT,
    sub_type TEXT,
    biz_no TEXT,
    operator TEXT,
    action TEXT,
    fail INTEGER DEFAULT 0,
    create_time TEXT,
    extra TEXT,
    code_variable TEXT,
    ip TEXT,
    is_deleted INTEGER DEFAULT 0
);
