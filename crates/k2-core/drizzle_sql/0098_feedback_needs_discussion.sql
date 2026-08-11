-- Tickets: add status `needs_discussion` (human follow-up / discussion needed).
-- Rebuilds CHECK (SQLite cannot ALTER CHECK in place). Same shape as 0090.

CREATE TABLE IF NOT EXISTS feedback__0098 (
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
                 CHECK (status IN (
                   'waiting','answered','resolved','dismissed','planned','needs_discussion'
                 )),
    answer       TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    answered_at  INTEGER
);
--> statement-breakpoint
INSERT INTO feedback__0098 (
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
ALTER TABLE feedback__0098 RENAME TO feedback;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_feedback_project_status_created
    ON feedback (project_id, status, created_at);
