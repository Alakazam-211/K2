-- 0253_site_visibility — `k2 publish` visibility modes
-- (prd-publish-visibility-and-dev-mode-v1 PV-1, vs-live PA-6, PA-13; 0.46.7).
--
-- One row per nested label a workspace publishes. A covered label with no row
-- is PRIVATE (fail closed): the store is `crate::site_visibility`, the gate is
-- k2-daemon `site_gate`. Reserved labels and App runtime labels are never
-- looked up here.
--
-- Nothing that is public today may stop working, so:
-- - every tunnelled `published_services` row gets a `public` row below;
-- - a label only the control plane knows is written `public` by the daemon
--   from the routing map, before that map routes anything (the persisted map
--   at boot, then the first map fetched): `grandfather` below says that pass
--   is still owed. A new database owes nothing: its labels all start private.
--
-- `site_sessions` / `site_open_codes` / `site_pin_limits` live here, not in
-- memory, so a restart neither signs everybody out nor resets a PIN lock
-- (PA-6). Only hashes of session ids and codes are stored, never the values,
-- and never a PIN. New tables only: renumber freely on a slip (and edit the
-- migration-order tests).
CREATE TABLE IF NOT EXISTS site_visibility (
    label          TEXT PRIMARY KEY NOT NULL,
    mode           TEXT NOT NULL DEFAULT 'private' CHECK (mode IN ('private', 'pin', 'public')),
    dev            INTEGER NOT NULL DEFAULT 0,
    pin_hash       TEXT,
    pin_generation INTEGER NOT NULL DEFAULT 0,
    pin_days       INTEGER NOT NULL DEFAULT 7,
    idle_secs      INTEGER,
    set_by         TEXT NOT NULL DEFAULT '',
    set_at         INTEGER NOT NULL DEFAULT (unixepoch()),
    public_reason  TEXT
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS site_sessions (
    id_hash        TEXT PRIMARY KEY NOT NULL,
    label          TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('open', 'pin')),
    user           TEXT NOT NULL,
    pin_generation INTEGER NOT NULL DEFAULT 0,
    notice_seen    INTEGER NOT NULL DEFAULT 0,
    created_at     INTEGER NOT NULL,
    expires_at     INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS site_sessions_label ON site_sessions (label);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS site_sessions_expires ON site_sessions (expires_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS site_open_codes (
    code_hash  TEXT PRIMARY KEY NOT NULL,
    label      TEXT NOT NULL,
    user       TEXT NOT NULL,
    path       TEXT NOT NULL,
    expires_at INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS site_open_codes_expires ON site_open_codes (expires_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS site_pin_limits (
    scope        TEXT PRIMARY KEY NOT NULL,
    fails        INTEGER NOT NULL DEFAULT 0,
    window_start INTEGER NOT NULL,
    locked_until INTEGER NOT NULL DEFAULT 0
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS site_visibility_meta (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
--> statement-breakpoint
INSERT OR IGNORE INTO site_visibility (label, mode, set_by, set_at, public_reason)
    SELECT lower(name), 'public', 'upgrade', unixepoch(), 'public before 0.46.7'
    FROM published_services
    WHERE expose = 'tunnel'
      AND lower(name) NOT IN ('skin', 'meet', 'chat', 'mail', 'voice', 'turn');
--> statement-breakpoint
INSERT OR IGNORE INTO site_visibility_meta (key, value)
    SELECT 'grandfather',
        CASE WHEN EXISTS (SELECT 1 FROM projects)
               OR EXISTS (SELECT 1 FROM published_services)
               OR EXISTS (SELECT 1 FROM _migrations WHERE applied_at < unixepoch() - 600)
             THEN 'pending' ELSE 'done' END;
