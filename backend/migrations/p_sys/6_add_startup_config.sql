-- @migration-name: add_startup_config
-- @if-not-exists: column p_app_review.startup_config
ALTER TABLE p_app_review ADD COLUMN startup_config TEXT;
