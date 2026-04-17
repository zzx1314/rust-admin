-- Migration: create orgs table and alter users.org_id to TEXT

CREATE TABLE IF NOT EXISTS orgs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    sort INTEGER DEFAULT 0,
    parent_id TEXT,
    parent_name TEXT,
    org_duty TEXT,
    desrc TEXT,
    type TEXT,
    is_out INTEGER DEFAULT 0,
    create_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    update_time TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    is_deleted INTEGER DEFAULT 0,
    remarks TEXT
);

-- Recreate users table with id as TEXT and org_id as TEXT (SQLite doesn't support ALTER COLUMN type)
CREATE TABLE users_new (
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

INSERT INTO users_new SELECT * FROM users;

DROP TABLE users;

ALTER TABLE users_new RENAME TO users;
