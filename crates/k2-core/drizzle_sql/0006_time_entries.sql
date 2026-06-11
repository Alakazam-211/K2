CREATE TABLE IF NOT EXISTS time_entries (
  id TEXT PRIMARY KEY,
  project_id TEXT,
  start_time INTEGER NOT NULL,
  end_time INTEGER NOT NULL,
  duration_seconds INTEGER NOT NULL,
  memo TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE SET NULL
);
