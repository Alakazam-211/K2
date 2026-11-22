-- 0328: App cron and point-in-time restore records
-- (prd-app-supabase-compat-v1 §11, lane E; the first release after 0.46.17).
--
-- `app_cron`: the daemon calls an App's own HTTP route on a 5-field cron
-- schedule (box time), headless, with `Authorization: Bearer
-- <CRON_SECRET>` (a vault secret the App also gets as env). `missed` says
-- what a slot missed while the daemon was down does: `skip` (default) or
-- `once` (one catch-up run if the slot is under 12 h old). `running_run`
-- is the overlap guard: while it names a run, the next slot is skipped and
-- logged as `skipped_overlap`. `failures` counts failures in a row; three
-- raise the owner's card.
--
-- `app_cron_runs`: one row per run (and per skipped slot): status `ok`,
-- `failed`, `timeout`, `skipped_overlap`, `skipped_missed`, `not_running`,
-- the HTTP status, the time and a short detail (never the response body
-- beyond a short excerpt, never the secret).
--
-- `pg_cluster_pitr`: per named cluster, the last base backup the daemon
-- took (daily for an archiving cluster), the last restore drill, and for a
-- cluster `restore-to` made: its source and the moment (epoch seconds).
--
-- `pg_db_switches`: each `k2 db cluster switch` of a database between a
-- cluster and its restore (and back): written in the same transaction as
-- the `sql_databases` row it moves.
CREATE TABLE IF NOT EXISTS app_cron (
    id TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    schedule TEXT NOT NULL,
    path TEXT NOT NULL,
    method TEXT NOT NULL DEFAULT 'GET' CHECK (method IN ('GET', 'POST')),
    missed TEXT NOT NULL DEFAULT 'skip' CHECK (missed IN ('skip', 'once')),
    timeout_secs INTEGER NOT NULL DEFAULT 300,
    enabled INTEGER NOT NULL DEFAULT 1,
    anchor_at INTEGER NOT NULL,
    running_run TEXT,
    failures INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    created_by TEXT,
    UNIQUE (app_id, path, schedule)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_cron_app ON app_cron (app_id);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_cron_runs (
    id TEXT PRIMARY KEY,
    cron_id TEXT NOT NULL,
    app_id TEXT NOT NULL,
    slot_at INTEGER,
    trigger TEXT NOT NULL CHECK (trigger IN ('schedule', 'catch_up', 'run_now')),
    status TEXT NOT NULL,
    http_status INTEGER,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    duration_ms INTEGER,
    detail TEXT
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_cron_runs_cron ON app_cron_runs (cron_id, started_at DESC);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS pg_cluster_pitr (
    major INTEGER NOT NULL,
    name TEXT NOT NULL,
    last_basebackup_at INTEGER,
    last_basebackup_ok INTEGER,
    last_basebackup_note TEXT,
    last_drill_at INTEGER,
    last_drill_ok INTEGER,
    last_drill_json TEXT,
    restored_from TEXT,
    restored_to INTEGER,
    PRIMARY KEY (major, name)
);
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS pg_db_switches (
    db_name TEXT NOT NULL,
    database_id TEXT NOT NULL,
    from_cluster TEXT NOT NULL,
    to_cluster TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('restore', 'rollback')),
    moment INTEGER NOT NULL,
    at INTEGER NOT NULL,
    by TEXT
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS pg_db_switches_db ON pg_db_switches (db_name, at DESC);
