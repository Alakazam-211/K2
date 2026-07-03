-- Feedback F1 (prd-agent-feedback-notifications §4.1) — agent→human asks.
--
-- `feedback` is the durable record of "an agent needs a person": a
-- question, an approval, or an fyi. It replaces the transient terminal
-- prompt with an item on the server's Feedback page that stays until a
-- human answers/resolves it. `feedback_comments` is the per-item
-- discussion thread (author = 'owner' | an agent name).
--
-- Shape notes:
-- - `project_id` = the asking WORKSPACE (projects.id). NOT a FK: project
--   rows can be removed independently (remove_workspace_db_only) and the
--   ask should survive as a readable record until resolved.
-- - `session_id`/`session_kind` (both nullable) = the asking session, so
--   the human can deep-link back into it (F3). kind ∈ canonical|sandbox;
--   NULL = ask filed outside any known session (must never fail the ask).
-- - `priority` 1 (urgent) … 5 (whenever), default 3.
-- - `status` pipeline (v1, agent-asks-human): waiting → answered →
--   resolved, + dismissed. Richer claim/review pipelines are later.
-- - `answer` = the accepted answer, denormalized off the thread so
--   `k2 feedback ask --wait` reads ONE row.
-- - Timestamps are unix SECONDS (house convention, cf. api_keys).
CREATE TABLE IF NOT EXISTS feedback (
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
                 CHECK (status IN ('waiting','answered','resolved','dismissed')),
    answer       TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    answered_at  INTEGER
);
--> statement-breakpoint
-- The per-item discussion thread. CASCADE: deleting an ask deletes its
-- thread (foreign_keys=ON is set at connection open — db/mod.rs).
CREATE TABLE IF NOT EXISTS feedback_comments (
    id          TEXT PRIMARY KEY NOT NULL,
    feedback_id TEXT NOT NULL REFERENCES feedback(id) ON DELETE CASCADE,
    author      TEXT NOT NULL,
    body        TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
--> statement-breakpoint
-- The list hot path: this workspace's open items, newest first.
CREATE INDEX IF NOT EXISTS idx_feedback_project_status_created
    ON feedback (project_id, status, created_at);
--> statement-breakpoint
-- The thread read path: one item's comments in chronological order.
CREATE INDEX IF NOT EXISTS idx_feedback_comments_feedback_created
    ON feedback_comments (feedback_id, created_at);
