-- Remote Session Layer 0 — grants (Stage 2 mint/list/revoke) + events
-- (denials always recorded when drive is attempted while OFF / without grant).
-- Fail-closed: tables may be empty; the app_settings master switch is the wall.

CREATE TABLE IF NOT EXISTS remote_session_grants (
  id TEXT PRIMARY KEY NOT NULL,
  principal_kind TEXT NOT NULL,
  principal_ref TEXT NOT NULL,
  credential_hash TEXT,
  scope TEXT NOT NULL CHECK(scope IN ('shell')),
  issued_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  issued_by TEXT,
  revoked_at INTEGER,
  label TEXT
);

CREATE TABLE IF NOT EXISTS remote_session_events (
  id TEXT PRIMARY KEY NOT NULL,
  grant_id TEXT,
  principal_label TEXT NOT NULL,
  kind TEXT NOT NULL,
  code TEXT,
  payload TEXT,
  created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_remote_session_events_created
  ON remote_session_events (created_at DESC);

CREATE INDEX IF NOT EXISTS idx_remote_session_grants_active
  ON remote_session_grants (revoked_at, expires_at);
