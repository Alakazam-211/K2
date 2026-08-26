-- Workspace data sidecar (prd-workspace-data-sidecar-v1) — K2-side
-- catalog for a supervised Postgres engine. Template: 0075_mail.sql
-- (unix-SECONDS timestamps, CHECK-constrained enums, project_id TEXT
-- and NOT a FK).
--
-- Boundary: product SQLite (k2so.db / k2.db) stays catalog only. Skin
-- rows never land here. Live PGDATA is the distro cluster, never the
-- workspace. The daemon talks SQL on loopback / Unix socket; it never
-- links libpq server.
--
-- `sql_server` — the SINGLETON install record (id is CHECKed to 1).
-- "not-installed" is the ABSENCE of the row, so the status enum only
-- covers installed lifecycles. `installed_major` is the distro Postgres
-- major (14–16) recorded at enable. `listen` is localhost-only.
-- `enable_progress_json` is the resumable enable machine (mail 0076
-- shape, inlined so this is one migration).
CREATE TABLE IF NOT EXISTS sql_server (
    id                   INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    status               TEXT NOT NULL
                         CHECK (status IN ('installing','running','degraded',
                                           'stopped','disabled','error')),
    installed_major      INTEGER,
    listen               TEXT,
    enable_progress_json TEXT,
    last_error           TEXT,
    installed_at         INTEGER,
    updated_at           INTEGER NOT NULL
);
--> statement-breakpoint
-- `sql_databases` — one row per workspace database. `project_id` =
-- projects.id, NOT a FK (0064 idiom). `name` is the Postgres database
-- name (`ws_<project_id>` by default). `client_id` powers idempotent
-- minting (`k2 db create --id`). Cap accounting counts ACTIVE rows
-- (drop frees the slot). Secrets live in ~/.k2/db-secrets.json as
-- `dbsec_*` refs — never plaintext here.
CREATE TABLE IF NOT EXISTS sql_databases (
    id                   TEXT PRIMARY KEY NOT NULL,
    project_id           TEXT NOT NULL,
    name                 TEXT NOT NULL,
    client_id            TEXT,
    status               TEXT NOT NULL DEFAULT 'active'
                         CHECK (status IN ('active','dropped')),
    agent_secret_ref     TEXT,
    migrator_secret_ref  TEXT,
    created_at           INTEGER NOT NULL,
    dropped_at           INTEGER
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_sql_databases_project_client
    ON sql_databases (project_id, client_id)
    WHERE client_id IS NOT NULL;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_sql_databases_project_name_active
    ON sql_databases (project_id, name)
    WHERE status = 'active';
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sql_databases_project_status
    ON sql_databases (project_id, status);
--> statement-breakpoint
-- Per-workspace agent passport (D21): 'off' | 'read' | 'write'.
-- NULL (the backfill) = fail-closed 'off'. Agent create only if write.
ALTER TABLE projects ADD COLUMN db_agent_access TEXT;
--> statement-breakpoint
-- Per-workspace ACTIVE-DB cap (D9): non-negative int, 0 = unlimited,
-- NULL = inherit default 1. drop --yes frees a slot.
ALTER TABLE projects ADD COLUMN db_active_cap INTEGER;
--> statement-breakpoint
-- Fail-closed db door on API keys (D19/D20). Existing keys stay 0
-- (unlike 0086 host-sessions DEFAULT 1). Owner-token principals have
-- every capability at the route layer.
ALTER TABLE api_keys ADD COLUMN cap_db INTEGER NOT NULL DEFAULT 0;
