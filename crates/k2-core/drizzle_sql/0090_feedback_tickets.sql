-- Tickets rename track (UI/CLI: Tickets; wire/table names stay `feedback*`
-- for compatibility). Adds:
--   1. status value `planned` — sorted/decided, fix scheduled for a release
--   2. `feedback_assignees` — multi-assignee usernames snapshotted as text
--      so push still targets them if a connect-user is later removed
--
-- SQLite CHECK constraints are rebuilt with the table (can't ALTER CHECK).

CREATE TABLE IF NOT EXISTS feedback__0090 (
    id           TEXT PRIMARY KEY NOT NULL,
    project_id   TEXT NOT NULL,
    session_id   TEXT,
    session_kind TEXT,
    agent_name   TEXT NOT NULL,
    kind         TEXT NOT NULL DEFAULT 'question'
                 CHECK (kind IN ('question','approval','fyi')),
    title        TEXT NOT NULL,
    body         TEXT,
    options_json TEXT,
    priority     INTEGER NOT NULL DEFAULT 3
                 CHECK (priority BETWEEN 1 AND 5),
    status       TEXT NOT NULL DEFAULT 'waiting'
                 CHECK (status IN ('waiting','answered','resolved','dismissed','planned')),
    answer       TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    answered_at  INTEGER
);
--> statement-breakpoint
INSERT INTO feedback__0090 (
    id, project_id, session_id, session_kind, agent_name, kind, title, body,
    options_json, priority, status, answer, created_at, updated_at, answered_at
)
SELECT
    id, project_id, session_id, session_kind, agent_name, kind, title, body,
    options_json, priority, status, answer, created_at, updated_at, answered_at
FROM feedback;
--> statement-breakpoint
DROP TABLE feedback;
--> statement-breakpoint
ALTER TABLE feedback__0090 RENAME TO feedback;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_feedback_project_status_created
    ON feedback (project_id, status, created_at);
--> statement-breakpoint
-- Assignees: username is a *snapshot* string (not a FK to connect-users)
-- so removed users still appear and still match push_devices.username.
CREATE TABLE IF NOT EXISTS feedback_assignees (
    feedback_id TEXT NOT NULL REFERENCES feedback(id) ON DELETE CASCADE,
    username    TEXT NOT NULL,
    assigned_at INTEGER NOT NULL,
    PRIMARY KEY (feedback_id, username)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_feedback_assignees_username
    ON feedback_assignees (username);
