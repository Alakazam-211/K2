-- 0294: an App's database connections (prd-app-database-integrations-v1
-- §4.7, slice S2; 0.46.11, Rosson 2026-11-10: "make sure Apps can actually
-- use the DBs").
--
-- One row per App and secret name (`as_name`, DATABASE_URL by default) and
-- environment: which box database `k2 app db connect` connected it to, the
-- App's own roles there (never a password: those live only in the vault,
-- in the App's URL secrets), the vault secrets it granted to the App, and
-- the last live check from inside the App. Written by `k2-daemon`
-- `apps::db_connect`; the database door (`apps::db_door`) admits an App's
-- own roles only on a database it has a row for. 0288–0293 are other
-- lanes'; renumber freely on a slip (and edit the migration-order tests).
CREATE TABLE IF NOT EXISTS app_db_connections (
    app_id TEXT NOT NULL,
    -- the App's env name for the URL (DATABASE_URL, or `--as`)
    as_name TEXT NOT NULL DEFAULT 'DATABASE_URL',
    -- 'production' | 'test' (S6; only production is made today)
    env TEXT NOT NULL DEFAULT 'production' CHECK (env IN ('production', 'test')),
    -- 'box' (a K2 database on this box); 'outside' is S4's
    kind TEXT NOT NULL DEFAULT 'box' CHECK (kind IN ('box', 'outside')),
    -- sql_databases.id and its Postgres name
    database_id TEXT,
    database_name TEXT NOT NULL,
    -- the workspace that owns the database (the App's own, or a grantor's)
    database_project TEXT NOT NULL,
    -- the cluster's label (17/main, 17/feedback) and major when connected
    cluster TEXT NOT NULL DEFAULT '',
    major INTEGER NOT NULL DEFAULT 0,
    -- the NOLOGIN group role and the current login role (<group>__g<n>)
    group_role TEXT NOT NULL,
    login_role TEXT NOT NULL,
    generation INTEGER NOT NULL DEFAULT 1,
    -- the role the group holds ({db}_agent, or the workspace's grantee)
    inherit_from TEXT NOT NULL,
    -- 1 when DATABASE_MIGRATE_URL (the migrator's) was granted too
    migrate_url INTEGER NOT NULL DEFAULT 0,
    -- JSON array of the vault secret names granted to the App (names only)
    secrets_json TEXT NOT NULL DEFAULT '[]',
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    rotated_at INTEGER,
    -- the last live check: when, and its JSON result (ok, version,
    -- latencyMs, step, error; never a URL)
    checked_at INTEGER,
    check_json TEXT,
    PRIMARY KEY (app_id, as_name, env)
);

CREATE INDEX IF NOT EXISTS idx_app_db_connections_db ON app_db_connections(database_name);
