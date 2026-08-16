-- Per-workspace compose-bar send history (Up/Down). Cap is enforced
-- in the insert transaction (newest 50 per project_id).

CREATE TABLE IF NOT EXISTS workspace_compose_send_history (
    id         TEXT PRIMARY KEY NOT NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    body       TEXT NOT NULL,
    author     TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_compose_send_history_project_created
    ON workspace_compose_send_history (project_id, created_at);
