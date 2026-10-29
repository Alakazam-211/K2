-- 0255_app_updates — an App's update feed (prd-tauri-core-0467-v1 TC-7,
-- vs-live TA-14/TA-15; 0.46.7). New tables only: renumber freely on a
-- slip (and edit the migration-order tests). The store is
-- `crate::app_updates::store`.
--
-- - `app_update_keys`: the App's updater PUBLIC key (minisign, the Tauri
--   `plugins.updater.pubkey` form). Never a secret key.
-- - `app_update_releases`: one row per (App, version). `state` is
--   `uploading | live | failed | yanked`. Archives are never kept here:
--   they live on the download host; the box keeps a staging copy only
--   while an upload is pending.
-- - `app_update_artifacts`: one row per platform of a release: file name,
--   sha256, size, signature (the `.sig` text Tauri checks), the download
--   host's immutable URL once the upload is confirmed.
-- - `app_update_channels`: where each channel points now;
--   `app_update_channel_log`: every move (push, promote, rollback, pull),
--   so a pull can fall back and the dynamic answer knows which releases a
--   channel has carried.
-- - `app_update_checkins`: version checks per day, channel, platform and
--   version (counts only, never an address); `installs` counts distinct
--   anonymous install ids that day when the app sends one.
--   `app_update_install_days` holds a SHA-256 of (App, day, that id) for
--   the current and previous UTC day only (pruned), so a count is distinct.
CREATE TABLE IF NOT EXISTS app_update_keys (
    app_id      TEXT PRIMARY KEY NOT NULL,
    pubkey      TEXT NOT NULL,
    key_id      TEXT NOT NULL,
    set_by      TEXT NOT NULL,
    set_at      INTEGER NOT NULL
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_update_releases (
    id          TEXT PRIMARY KEY NOT NULL,
    app_id      TEXT NOT NULL,
    version     TEXT NOT NULL,
    channel     TEXT NOT NULL,
    notes       TEXT,
    pub_date    TEXT NOT NULL,
    created_by  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    state       TEXT NOT NULL CHECK (state IN ('uploading', 'live', 'failed', 'yanked')),
    error       TEXT,
    updated_at  INTEGER NOT NULL,
    UNIQUE (app_id, version)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_update_releases_app ON app_update_releases (app_id, created_at);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_update_artifacts (
    release_id    TEXT NOT NULL REFERENCES app_update_releases (id) ON DELETE CASCADE,
    platform      TEXT NOT NULL,
    file_name     TEXT NOT NULL,
    content_type  TEXT NOT NULL,
    sha256        TEXT NOT NULL,
    size          INTEGER NOT NULL,
    signature     TEXT NOT NULL,
    url           TEXT,
    upload_state  TEXT NOT NULL CHECK (upload_state IN ('pending', 'uploaded', 'failed')),
    error         TEXT,
    PRIMARY KEY (release_id, platform)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_update_channels (
    app_id      TEXT NOT NULL,
    channel     TEXT NOT NULL,
    release_id  TEXT NOT NULL REFERENCES app_update_releases (id) ON DELETE CASCADE,
    updated_by  TEXT NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (app_id, channel)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_update_channel_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    app_id      TEXT NOT NULL,
    channel     TEXT NOT NULL,
    release_id  TEXT,
    action      TEXT NOT NULL CHECK (action IN ('push', 'promote', 'rollback', 'pull')),
    by          TEXT NOT NULL,
    at          INTEGER NOT NULL
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_update_channel_log_app ON app_update_channel_log (app_id, channel, id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_update_checkins (
    app_id      TEXT NOT NULL,
    day         TEXT NOT NULL,
    channel     TEXT NOT NULL,
    platform    TEXT NOT NULL,
    version     TEXT NOT NULL,
    checks      INTEGER NOT NULL DEFAULT 0,
    installs    INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (app_id, day, channel, platform, version)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_update_install_days (
    app_id        TEXT NOT NULL,
    day           TEXT NOT NULL,
    install_hash  TEXT NOT NULL,
    PRIMARY KEY (app_id, day, install_hash)
);
