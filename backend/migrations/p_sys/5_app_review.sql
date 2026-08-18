-- @migration-name: add_app_review
CREATE TABLE IF NOT EXISTS p_app_review (
    id INTEGER PRIMARY KEY,
    src_project TEXT NOT NULL,
    dest_project TEXT NOT NULL,
    repository_name TEXT NOT NULL,
    tag TEXT NOT NULL,
    digest TEXT,
    artifact_id INTEGER,
    status TEXT NOT NULL DEFAULT 'pending',
    reviewer_comment TEXT,
    created_by INTEGER,
    reviewer_id INTEGER,
    create_time TEXT NOT NULL,
    update_time TEXT,
    is_deleted INTEGER DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_app_review_status ON p_app_review(status);
CREATE INDEX IF NOT EXISTS idx_app_review_project ON p_app_review(src_project);
CREATE UNIQUE INDEX IF NOT EXISTS idx_app_review_pending_unique
    ON p_app_review(src_project, repository_name, tag)
    WHERE status = 'pending' AND is_deleted = 0;
