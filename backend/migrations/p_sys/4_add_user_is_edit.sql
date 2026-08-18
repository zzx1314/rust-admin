-- @migration-name: add_user_is_edit
-- @if-not-exists: column p_sys_user.is_edit
-- Add is_edit column to p_sys_user table for existing databases
ALTER TABLE p_sys_user ADD COLUMN is_edit INTEGER DEFAULT 1;

-- Protect sysadmin user (id=1) from being edited/deleted
UPDATE p_sys_user SET is_edit = 0 WHERE id = 1;
