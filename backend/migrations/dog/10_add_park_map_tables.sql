-- @migration-name: add_park_map_tables

-- Parks (园区) are owned by the user who creates them; maps (地图) belong to a
-- park. Both carry the owning `user_id` so list/detail queries can be scoped
-- without a join.

CREATE TABLE IF NOT EXISTS dog_park (
    id INTEGER PRIMARY KEY,
    park_name TEXT NOT NULL,
    park_description TEXT,
    park_address TEXT,
    user_id INTEGER NOT NULL,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS dog_map (
    id INTEGER PRIMARY KEY,
    map_name TEXT NOT NULL,
    map_type TEXT,
    map_data TEXT,
    park_id INTEGER NOT NULL,
    user_id INTEGER NOT NULL,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_dog_park_user ON dog_park(user_id);
CREATE INDEX IF NOT EXISTS idx_dog_map_park ON dog_map(park_id);
CREATE INDEX IF NOT EXISTS idx_dog_map_user ON dog_map(user_id);
