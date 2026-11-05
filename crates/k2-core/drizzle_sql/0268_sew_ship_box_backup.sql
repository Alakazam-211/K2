-- 0268: Sew's off-box copy through the box's own backup (0.46.9).
-- `k2 hostchat ship box-backup` records ship_kind = 'box-backup': the box
-- backup client (k2-backup) carries /var/lib/sew/backups off the box, and
-- K2 clears k2_backup_offbox_none only while it sees the evidence (the
-- daemon's sew::box_backup). 0166's CHECK allowed only 'none' and 'dir',
-- and SQLite can't alter a CHECK, so sew_server is copied into a new table
-- with the wider CHECK. Nothing references sew_server and it has no index
-- or trigger, so the rebuild cascades nowhere. Every column is kept as
-- 0166 made it (no later migration added one). 0265-0267 are taken by
-- other 0.46.9 branches; renumber freely on a slip (and edit the
-- migration-order tests), never below a number already live.
CREATE TABLE IF NOT EXISTS sew_server_0268 (
  id                     INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
  status                 TEXT NOT NULL CHECK (status IN
                           ('installing','running','degraded','stopped','disabled',
                            'error','upgrading','removed')),
  pinned_version         TEXT NOT NULL,
  installed_version      TEXT,
  previous_version       TEXT,
  install_id             TEXT,
  master_key_ref         TEXT,
  master_key_created_at  INTEGER,
  master_next_ref        TEXT,
  master_prev_ref        TEXT,
  recovery_shown_at      INTEGER,
  host                   TEXT,
  host_mode              TEXT CHECK (host_mode IN ('direct','connect')),
  app_url                TEXT,
  mirror_kind            TEXT NOT NULL DEFAULT 'none' CHECK (mirror_kind IN ('none','dir')),
  mirror_path            TEXT,
  ship_kind              TEXT NOT NULL DEFAULT 'none' CHECK (ship_kind IN ('none','dir','box-backup')),
  ship_path              TEXT,
  last_shipped_id        TEXT,
  last_shipped_at        INTEGER,
  last_ship_error        TEXT,
  enable_progress_json   TEXT,
  upgrade_progress_json  TEXT,
  rotate_progress_json   TEXT,
  restore_progress_json  TEXT,
  doctor_json            TEXT,
  doctor_at              INTEGER,
  last_error             TEXT,
  installed_at           INTEGER,
  updated_at             INTEGER NOT NULL
);
--> statement-breakpoint
INSERT INTO sew_server_0268 SELECT id, status, pinned_version, installed_version, previous_version, install_id,
  master_key_ref, master_key_created_at, master_next_ref, master_prev_ref, recovery_shown_at, host, host_mode, app_url,
  mirror_kind, mirror_path, ship_kind, ship_path, last_shipped_id, last_shipped_at, last_ship_error,
  enable_progress_json, upgrade_progress_json, rotate_progress_json, restore_progress_json, doctor_json, doctor_at,
  last_error, installed_at, updated_at FROM sew_server;
--> statement-breakpoint
DROP TABLE sew_server;
--> statement-breakpoint
ALTER TABLE sew_server_0268 RENAME TO sew_server;
