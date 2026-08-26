-- Workspace data sidecar D21/D22 (prd-workspace-data-sidecar-v1) —
-- cross-workspace grants + optional bind role. Template: 0079/0081
-- mail_inbox_grants (unix-SECONDS timestamps, CHECK-constrained enums,
-- project_id TEXT and NOT a FK).
--
-- `sql_grants` — other workspaces get read|write on a database via
-- THEIR Postgres role (never the owner's superuser / k2_admin).
-- `can_manage` is orthogonal to level (mail 0081): the owner workspace
-- always manages; a grant with can_manage may grant/revoke others.
-- A workspace with neither the owner binding nor a grant row does not
-- see the database (no existence leak on agent paths).
CREATE TABLE IF NOT EXISTS sql_grants (
    database_id TEXT NOT NULL,
    project_id  TEXT NOT NULL,
    level       TEXT NOT NULL CHECK (level IN ('read','write')),
    can_manage  INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (database_id, project_id)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sql_grants_database
    ON sql_grants (database_id);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sql_grants_project
    ON sql_grants (project_id);
--> statement-breakpoint
-- D22: optional PG role the workspace assistant uses (default remains
-- ws_<id>_agent when NULL). Catalog only — spawn does not mint RLS.
ALTER TABLE sql_databases ADD COLUMN bind_role TEXT;
