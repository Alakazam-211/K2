-- P20: widen sql_databases.status CHECK to admit 'test' (disposable
-- `k2 db create --test` row). SQLite cannot ALTER a CHECK in place —
-- table rebuild, same pattern as 0082 / 0084.
--
-- One test row per workspace. Live (active) and test cannot share `name`.
-- Dropped tombstones stay out of both unique indexes (name reuse after
-- drop --yes / drop --test --yes). count_active stays status='active'.
CREATE TABLE IF NOT EXISTS sql_databases_new (
    id                   TEXT PRIMARY KEY NOT NULL,
    project_id           TEXT NOT NULL,
    name                 TEXT NOT NULL,
    client_id            TEXT,
    status               TEXT NOT NULL DEFAULT 'active'
                         CHECK (status IN ('active','dropped','test')),
    agent_secret_ref     TEXT,
    migrator_secret_ref  TEXT,
    created_at           INTEGER NOT NULL,
    dropped_at           INTEGER,
    bind_role            TEXT
);
--> statement-breakpoint
INSERT INTO sql_databases_new (
    id, project_id, name, client_id, status,
    agent_secret_ref, migrator_secret_ref, created_at, dropped_at, bind_role
)
SELECT
    id, project_id, name, client_id, status,
    agent_secret_ref, migrator_secret_ref, created_at, dropped_at, bind_role
FROM sql_databases;
--> statement-breakpoint
DROP TABLE sql_databases;
--> statement-breakpoint
ALTER TABLE sql_databases_new RENAME TO sql_databases;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_sql_databases_project_client
    ON sql_databases (project_id, client_id)
    WHERE client_id IS NOT NULL;
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_sql_databases_project_name_live
    ON sql_databases (project_id, name)
    WHERE status IN ('active','test');
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_sql_databases_project_test
    ON sql_databases (project_id)
    WHERE status = 'test';
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sql_databases_project_status
    ON sql_databases (project_id, status);
