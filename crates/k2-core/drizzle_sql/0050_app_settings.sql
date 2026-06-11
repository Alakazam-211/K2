-- 0050_app_settings.sql
--
-- Create the long-missing `app_settings` table. The
-- `workspace/settings.rs` accessors (`get_agentic_enabled`,
-- `set_agentic_enabled`, `get_keep_daemon_on_quit`,
-- `set_keep_daemon_on_quit`) all read/write this table via
-- `INSERT OR REPLACE INTO app_settings (key, value) ...`, but no
-- prior migration ever created it. Live impact: `POST /cli/agentic`
-- returned HTTP 400 with "no such table: app_settings", and the
-- corresponding GETs silently defaulted via `.unwrap_or(default)`
-- so callers never saw the missing-table error.
--
-- Schema is the simplest key/value shape that matches the call
-- sites — `key` is the primary key, `value` is a free-form TEXT
-- (callers serialize bools as "0"/"1" strings).

CREATE TABLE IF NOT EXISTS app_settings (
  key TEXT PRIMARY KEY,
  value TEXT
);
