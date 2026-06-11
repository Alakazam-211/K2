-- DB-tracked agent sessions replacing .lock/.last_session files.
-- owner='system' for scheduler-managed sessions, 'user' for interactive sessions.
CREATE TABLE IF NOT EXISTS agent_sessions (
  id TEXT PRIMARY KEY,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  agent_name TEXT NOT NULL,
  terminal_id TEXT,
  session_id TEXT,
  harness TEXT NOT NULL DEFAULT 'claude',
  owner TEXT NOT NULL DEFAULT 'system',
  status TEXT NOT NULL DEFAULT 'sleeping',
  status_message TEXT,
  last_activity_at INTEGER,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(project_id, agent_name)
);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_project ON agent_sessions(project_id);
