CREATE TABLE IF NOT EXISTS chat_session_names (
  provider TEXT NOT NULL,
  session_id TEXT NOT NULL,
  custom_name TEXT NOT NULL,
  updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
  PRIMARY KEY (provider, session_id)
);
