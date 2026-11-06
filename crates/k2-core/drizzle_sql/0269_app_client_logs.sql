-- 0269: App client logs (0.46.9, app-client-logs-v1). A signed-in App
-- user's device sends a small, redacted log bundle (a panic message, the
-- last tracing lines, the app version and OS) with their own session pass
-- (`diagnostics.send`, POST /cli/skin/client-logs); the App's owner and
-- its agent read them with `k2 app logs <app> --client`. `lines` is a JSON
-- array of strings, already redacted by the daemon. Rows go after the
-- App's retention (14 days unless `k2 app set <app> --client-logs-days`),
-- with the person's account deletion, and past a per-App row cap. The
-- store is `crate::app_client_logs`.
CREATE TABLE IF NOT EXISTS app_client_logs (
  id TEXT PRIMARY KEY NOT NULL,
  app_id TEXT NOT NULL,
  principal_id TEXT NOT NULL,
  username TEXT NOT NULL,
  received_at INTEGER NOT NULL,
  client_at INTEGER,
  kind TEXT NOT NULL CHECK (kind IN ('crash', 'log')),
  app_version TEXT NOT NULL,
  os TEXT NOT NULL,
  device TEXT,
  message TEXT,
  lines TEXT NOT NULL,
  bytes INTEGER NOT NULL,
  redacted INTEGER NOT NULL DEFAULT 0,
  truncated INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_app_client_logs_app ON app_client_logs (app_id, received_at);
CREATE INDEX IF NOT EXISTS idx_app_client_logs_person ON app_client_logs (principal_id);
CREATE TABLE IF NOT EXISTS app_client_log_settings (
  app_id TEXT PRIMARY KEY NOT NULL,
  keep_days INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  updated_by TEXT
);
