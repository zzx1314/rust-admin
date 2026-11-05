-- @migration-name: add_map_file_fields

-- Map records are uploaded as `.zip` packages (same model as the legacy
-- mgmt_platform "模型管理 → 地图" flow): the list shows the file metadata and
-- the description instead of the park/JSON payload. `park_id` therefore
-- becomes optional, and the file columns are added.
--
-- SQLite cannot drop the NOT NULL constraint in place, so the table is
-- rebuilt and the rows copied over.

CREATE TABLE IF NOT EXISTS dog_map_new (
    id INTEGER PRIMARY KEY,
    map_name TEXT NOT NULL,
    map_type TEXT,
    map_data TEXT,
    park_id INTEGER,
    user_id INTEGER NOT NULL,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0,
    file_name TEXT,
    file_path TEXT,
    file_format TEXT,
    file_size INTEGER,
    remarks TEXT
);

INSERT INTO dog_map_new (
    id,
    map_name,
    map_type,
    map_data,
    park_id,
    user_id,
    create_time,
    update_time,
    is_deleted
)
SELECT
    id,
    map_name,
    map_type,
    map_data,
    park_id,
    user_id,
    create_time,
    update_time,
    is_deleted
FROM dog_map;

DROP TABLE dog_map;

ALTER TABLE dog_map_new RENAME TO dog_map;

CREATE INDEX IF NOT EXISTS idx_dog_map_park ON dog_map(park_id);
CREATE INDEX IF NOT EXISTS idx_dog_map_user ON dog_map(user_id);
