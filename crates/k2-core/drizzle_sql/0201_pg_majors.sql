-- 0201: several Postgres majors per server + `k2 db upgrade` per database
-- (prd-pg17-and-db-upgrade-v1 rev 2 §4, §12; 0.46.3, Rosson 2026-10-20).
--
-- `sql_clusters` — one row per Postgres cluster K2 knows, keyed on
-- (major, name). Nothing derives a port from a major: every caller reads
-- the record through the daemon's resolver (`sql::cluster`). The box's
-- first cluster keeps 5432; further majors take the next free port in
-- K2's band 5432–5449.
--   role:   'k2' (K2 creates databases in it) | 'adopted' (set up outside
--           K2; moves need the owner's --allow-adopted) | 'foreign' (Sew's
--           own cluster and the like: recorded, never touched)
--   origin: 'baked' (provision installed it) | 'created' (`pg
--           ensure-cluster`) | 'adopted'
--   state:  what the last look saw ('online' | 'down' | 'unknown')
--   start_conf: Debian's start.conf ('auto' | 'manual' | 'disabled')
CREATE TABLE IF NOT EXISTS sql_clusters (
    major       INTEGER NOT NULL CHECK (major BETWEEN 10 AND 99),
    name        TEXT NOT NULL DEFAULT 'main',
    port        INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    data_dir    TEXT NOT NULL,
    unit        TEXT NOT NULL,
    start_conf  TEXT NOT NULL DEFAULT 'auto'
                CHECK (start_conf IN ('auto','manual','disabled')),
    state       TEXT NOT NULL DEFAULT 'unknown'
                CHECK (state IN ('online','down','unknown')),
    role        TEXT NOT NULL DEFAULT 'k2'
                CHECK (role IN ('k2','adopted','foreign')),
    origin      TEXT NOT NULL DEFAULT 'baked'
                CHECK (origin IN ('baked','created','adopted')),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (major, name)
);
--> statement-breakpoint
CREATE UNIQUE INDEX IF NOT EXISTS idx_sql_clusters_port ON sql_clusters (port);
--> statement-breakpoint
-- The cluster today's boxes already run: the one `enable` read
-- (`installed_major`), on 5432. Q4: 'baked' unless ops' inventory names
-- the box as adopted (the owner re-records it).
INSERT OR IGNORE INTO sql_clusters
    (major, name, port, data_dir, unit, start_conf, state, role, origin, created_at, updated_at)
SELECT installed_major, 'main', 5432,
       '/var/lib/postgresql/' || installed_major || '/main',
       'postgresql@' || installed_major || '-main.service',
       'auto', 'unknown', 'k2', 'baked',
       CAST(strftime('%s', 'now') AS INTEGER), CAST(strftime('%s', 'now') AS INTEGER)
FROM sql_server
WHERE id = 1 AND installed_major BETWEEN 10 AND 99;
--> statement-breakpoint
-- Each database records its cluster. Backfill: today every database lives
-- in the one cluster `enable` read. NULL = the 5432 cluster (old rows on
-- a box whose major was never read).
ALTER TABLE sql_databases ADD COLUMN cluster_major INTEGER;
--> statement-breakpoint
-- After a move: where it came from, when, and until when `--rollback`
-- may bring it back (14 days). `source_state`: 'none' (never moved) |
-- 'kept' (the source copy stays in the old cluster, ALLOW_CONNECTIONS
-- false) | 'forgotten' (`--forget-old` dropped it, owner only).
ALTER TABLE sql_databases ADD COLUMN moved_from_major INTEGER;
--> statement-breakpoint
ALTER TABLE sql_databases ADD COLUMN moved_at INTEGER;
--> statement-breakpoint
ALTER TABLE sql_databases ADD COLUMN rollback_until INTEGER;
--> statement-breakpoint
ALTER TABLE sql_databases ADD COLUMN source_state TEXT NOT NULL DEFAULT 'none'
    CHECK (source_state IN ('none','kept','forgotten'));
--> statement-breakpoint
UPDATE sql_databases
SET cluster_major = (SELECT installed_major FROM sql_server WHERE id = 1 AND installed_major BETWEEN 10 AND 99)
WHERE cluster_major IS NULL;
--> statement-breakpoint
-- `sql_moves` — one row per move (or rollback) of one database. The
-- machine runs one database at a time (hostmail-upgrade shape: busy flag,
-- `interrupted` at boot, a failure latch). `method`: 'copy' (dump/restore,
-- 0.46.3 default) | 'dual' (logical replication, 0.46.4) | 'cluster'
-- (pg_upgradecluster, 0.46.4, owner only). `kind`: 'move' | 'rollback'.
CREATE TABLE IF NOT EXISTS sql_moves (
    id            TEXT PRIMARY KEY NOT NULL,
    database_id   TEXT NOT NULL,
    db            TEXT NOT NULL,
    from_major    INTEGER NOT NULL,
    to_major      INTEGER NOT NULL,
    method        TEXT NOT NULL DEFAULT 'copy'
                  CHECK (method IN ('copy','dual','cluster')),
    kind          TEXT NOT NULL DEFAULT 'move'
                  CHECK (kind IN ('move','rollback')),
    state         TEXT NOT NULL DEFAULT 'queued'
                  CHECK (state IN ('queued','checking','copying','verifying','switching',
                                   'succeeded','aborted','rolled_back','failed','interrupted')),
    progress_json TEXT,
    actor         TEXT,
    started_at    INTEGER NOT NULL,
    finished_at   INTEGER,
    last_error    TEXT
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sql_moves_db ON sql_moves (database_id, started_at);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_sql_moves_state ON sql_moves (state);
--> statement-breakpoint
-- What `k2 db create` uses without --major (PRD §2 goal 2, Q8): 17 on new
-- boxes, and on a box that already runs Postgres the major of its 5432
-- cluster until the owner moves it (`k2 db default-major 17`). Keep
-- today's path until the new one is proven: an existing 16 box keeps
-- creating on 16. The backfill pins it to the cluster `enable` read; NULL
-- (a box with no Postgres yet) resolves at runtime to the 5432 cluster's
-- major, and to 17 only when there is none.
ALTER TABLE sql_server ADD COLUMN default_major INTEGER;
--> statement-breakpoint
UPDATE sql_server SET default_major = installed_major
WHERE id = 1 AND default_major IS NULL AND installed_major BETWEEN 10 AND 99;
--> statement-breakpoint
-- The per-agent "Allow this agent to manage Postgres versions" switch
-- (`db_upgrade`). Default 0. Written only by POST /cli/agent-access/set:
-- the owner raises, the owner or an Admin lowers, never an agent. With it
-- on, a local agent passport from the workspace may check, move and roll
-- back its own (and its Apps') databases, create on a chosen major and
-- tidy empty clusters; dropping the old copy stays owner-only.
ALTER TABLE projects ADD COLUMN agents_can_upgrade_postgres INTEGER NOT NULL DEFAULT 0;
