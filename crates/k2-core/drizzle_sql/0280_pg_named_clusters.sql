-- 0280: named Postgres clusters (prd-named-pg-clusters-v1, 0.46.11).
-- Beside each major's `<M>/main`, the root helper (protocol 15) makes a
-- cluster with a name of its own (`17/feedback`) for one database that
-- needs a cluster to itself. `sql_clusters` is already keyed (major, name)
-- with a unique port (0201); these columns say which workspace the cluster
-- is dedicated to and whether it archives WAL, and which cluster each
-- database lives in. Renumber freely on a slip (and edit the
-- migration-order tests). The reader is `k2-daemon` `sql::cluster`.
--
-- `sql_databases.cluster_name`: NULL = `<cluster_major>/main` (every row
-- before this column).
ALTER TABLE sql_databases ADD COLUMN cluster_name TEXT;
--> statement-breakpoint
-- `sql_clusters.owner_project`: a named cluster's one workspace (the only
-- one `k2 db create --cluster` may create in). NULL for `<M>/main`.
ALTER TABLE sql_clusters ADD COLUMN owner_project TEXT;
--> statement-breakpoint
-- `sql_clusters.wal_archive`: made with `--wal-archive` (the helper's
-- canned archive_command into /var/lib/postgresql/k2-wal/<M>-<name>).
ALTER TABLE sql_clusters ADD COLUMN wal_archive INTEGER NOT NULL DEFAULT 0;
