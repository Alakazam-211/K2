-- 0281: the remote desktop tab (0.46.11, prd-remote-desktop-tab-v1 D1).
-- 0278/0279 are Reminders and mail notices, 0280 is reserved for named PG
-- clusters (wiki `Ops - Migration numbers`). Renumber freely on a slip (and
-- edit the migration-order tests). The store is `crate::desktop`.
--
-- `desktop_settings`: one row per desktop (`console` in v1). The switch is
-- here, NOT in AppSettings (S1): `/cli/settings/update` takes Admin
-- writes, and this switch is the Owner's. No row = off. `hide_apps_json`
-- is the password-app hide-list (bundle ids); NULL = K2's default list.
CREATE TABLE IF NOT EXISTS desktop_settings (
  desktop_id     TEXT PRIMARY KEY NOT NULL,
  enabled        INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
  hide_apps_json TEXT,
  updated_at     INTEGER NOT NULL DEFAULT 0,
  updated_by     TEXT NOT NULL DEFAULT ''
);
--> statement-breakpoint
-- `desktop_grants`: a Member login may watch with a grant (Owners and
-- Admins open without one). Keyed by the login's stable id
-- (`connect_users` `id`, vs-live R18), never its name, so a removed and
-- re-added name inherits nothing. `role_at_grant`: a role change (or the
-- login's removal or disabling) ends the grant. No TTL: a grant lasts
-- until revoked (Rosson 2026-11-10).
CREATE TABLE IF NOT EXISTS desktop_grants (
  desktop_id    TEXT NOT NULL,
  user_id       TEXT NOT NULL,
  username      TEXT NOT NULL,
  role_at_grant TEXT NOT NULL,
  mode          TEXT NOT NULL CHECK (mode IN ('view', 'control')),
  granted_by    TEXT NOT NULL,
  granted_at    INTEGER NOT NULL,
  PRIMARY KEY (desktop_id, user_id)
);
