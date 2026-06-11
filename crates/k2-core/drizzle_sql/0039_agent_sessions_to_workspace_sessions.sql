-- 0039: agent_sessions → workspace_sessions; one row per project_id.
--
-- Makes the product invariant ("a workspace IS its agent") load-
-- bearing at the schema level via UNIQUE(project_id). Existing
-- multi-row workspaces (legacy data drift from mode-swaps where one
-- workspace ended up with both a `manager` and a `k2so-agent` row)
-- collapse to a single row, with the lowest-rowid row winning;
-- anything dropped is preserved in workspace_sessions_legacy_archive
-- for audit/recovery.
--
-- The new table reuses the name freed by 0038 (the original
-- 0009-vintage `workspace_sessions` was renamed to `workspace_layouts`).
-- Carries every column shipped through 0.36.x: surfaced (0036),
-- active_terminal_id (0037), wakes_since_compact (0027). Drops only
-- agent_name — redundant with project_id post-collapse.

CREATE TABLE workspace_sessions_legacy_archive AS
  SELECT * FROM agent_sessions;

CREATE TABLE workspace_sessions (
  id                  TEXT PRIMARY KEY,
  project_id          TEXT NOT NULL UNIQUE REFERENCES projects(id) ON DELETE CASCADE,
  terminal_id         TEXT,
  active_terminal_id  TEXT,
  surfaced            INTEGER NOT NULL DEFAULT 0,
  session_id          TEXT,
  harness             TEXT NOT NULL DEFAULT 'claude',
  owner               TEXT NOT NULL DEFAULT 'system',
  status              TEXT NOT NULL DEFAULT 'sleeping',
  status_message      TEXT,
  last_activity_at    INTEGER,
  wakes_since_compact INTEGER NOT NULL DEFAULT 0,
  created_at          INTEGER NOT NULL DEFAULT (unixepoch())
);

INSERT INTO workspace_sessions
  (id, project_id, terminal_id, active_terminal_id, surfaced, session_id, harness, owner, status, status_message, last_activity_at, wakes_since_compact, created_at)
  SELECT id, project_id, terminal_id, active_terminal_id, surfaced, session_id, harness, owner, status, status_message, last_activity_at, wakes_since_compact, created_at
  FROM agent_sessions
  WHERE rowid IN (
    SELECT MIN(rowid) FROM agent_sessions GROUP BY project_id
  );

DROP TABLE agent_sessions;

CREATE INDEX idx_workspace_sessions_project ON workspace_sessions(project_id);
