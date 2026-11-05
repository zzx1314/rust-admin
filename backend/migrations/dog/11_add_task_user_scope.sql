-- @migration-name: add_task_user_scope
-- @if-not-exists: column dog_task.user_id

-- Tasks are linked to the user who created them. Existing rows keep NULL and
-- are therefore only visible to administrators (no owner filter applied).

ALTER TABLE dog_task ADD COLUMN user_id INTEGER;

CREATE INDEX IF NOT EXISTS idx_dog_task_user ON dog_task(user_id);
