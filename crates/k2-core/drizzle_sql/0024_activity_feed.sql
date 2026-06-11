-- Audit trail for all agent communications and lifecycle events.
-- Every k2so CLI call logs here for observability and debugging.
CREATE TABLE IF NOT EXISTS activity_feed (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  agent_name TEXT,
  event_type TEXT NOT NULL,
  from_agent TEXT,
  to_agent TEXT,
  to_project_id TEXT,
  summary TEXT,
  metadata TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_feed_project_time ON activity_feed(project_id, created_at);
CREATE INDEX IF NOT EXISTS idx_feed_agent ON activity_feed(project_id, agent_name);
