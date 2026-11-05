-- @migration-name: create_dog_tables

CREATE TABLE IF NOT EXISTS dog_device (
    id INTEGER PRIMARY KEY,
    device_id TEXT NOT NULL,
    device_type TEXT,
    status TEXT NOT NULL DEFAULT 'offline',
    device_ip TEXT,
    model TEXT,
    speed TEXT,
    height TEXT,
    battery TEXT,
    course TEXT,
    location TEXT,
    comm_time TEXT,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_task (
    id INTEGER PRIMARY KEY,
    task_name TEXT NOT NULL,
    task_description TEXT,
    task_status TEXT NOT NULL DEFAULT 'pending',
    workflow_id INTEGER,
    workflow_uuid TEXT,
    device_id INTEGER,
    route_id INTEGER,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_media (
    id INTEGER PRIMARY KEY,
    media_name TEXT NOT NULL,
    media_type TEXT,
    media_path TEXT,
    media_size INTEGER,
    remarks TEXT,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_model (
    id INTEGER PRIMARY KEY,
    model_name TEXT NOT NULL,
    model_type TEXT,
    file_name TEXT,
    file_path TEXT,
    file_format TEXT,
    file_size INTEGER,
    remarks TEXT,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_config (
    id INTEGER PRIMARY KEY,
    config_name TEXT NOT NULL,
    config_value TEXT,
    description TEXT,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_command (
    id INTEGER PRIMARY KEY,
    command_name TEXT NOT NULL,
    command_type TEXT,
    command_params TEXT,
    device_id TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    return_value TEXT,
    task_id INTEGER,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_route_library (
    id INTEGER PRIMARY KEY,
    route_name TEXT NOT NULL,
    route_type TEXT,
    route_status TEXT,
    route_description TEXT,
    model_id INTEGER NOT NULL,
    route_data TEXT,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_workflow (
    id INTEGER PRIMARY KEY,
    uuid TEXT NOT NULL,
    nodes TEXT,
    edges TEXT,
    task_id INTEGER,
    command_json_string TEXT,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_services (
    id INTEGER PRIMARY KEY,
    type TEXT,
    params TEXT,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);
