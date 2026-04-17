-- Migration: restructure users table to match SysUser schema
-- Drop old table and recreate with all fields

DROP TABLE IF EXISTS users;

CREATE TABLE users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    phone TEXT,
    email TEXT,
    real_name TEXT,
    password TEXT,
    org_id TEXT,
    lock_time TIMESTAMP,
    last_login_time TIMESTAMP,
    try_count INTEGER DEFAULT 0,
    lock_flag INTEGER DEFAULT 1,
    create_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    update_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT,
    pass_update_time TIMESTAMP,
    card TEXT,
    is_show INTEGER DEFAULT 1,
    enable INTEGER DEFAULT 1,
    first_login INTEGER DEFAULT 1,
    sex TEXT
);
