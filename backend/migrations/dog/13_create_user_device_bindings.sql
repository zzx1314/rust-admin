-- @migration-name: create_user_device_bindings

-- Binds users to devices (many-to-many). A user may be bound to several
-- devices, and a device may be shared by several users. Regular users only
-- see devices they are bound to; administrators bypass the filter.
--
-- The unique index covers soft-deleted rows too, so re-binding restores the
-- existing row instead of inserting a duplicate.

CREATE TABLE IF NOT EXISTS dog_user_device (
    id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    device_id INTEGER NOT NULL,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_dog_user_device ON dog_user_device(user_id, device_id);
CREATE INDEX IF NOT EXISTS idx_dog_user_device_user ON dog_user_device(user_id);
CREATE INDEX IF NOT EXISTS idx_dog_user_device_device ON dog_user_device(device_id);
