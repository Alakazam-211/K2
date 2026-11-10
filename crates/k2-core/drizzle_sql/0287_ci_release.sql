-- 0287: `k2 ci release` and `k2 ci evidence` (0.46.11, prd-ci-on-compute-
-- for-clients-v1 §4.6, §4.7, slices C5 + C6b, and C8's `k2 app updates
-- push --evidence`). Columns and one table only. Renumber freely on a slip
-- (and edit the migration-order tests). The stores are `crate::ci` and
-- `crate::ci_release`.
--
-- A release run is a `ci_runs` row of kind 'release': the same reaper, the
-- same verdict, no commit statuses. Every older row is a check run.
ALTER TABLE ci_runs ADD COLUMN kind TEXT NOT NULL DEFAULT 'checks';
--> statement-breakpoint
-- The release version (`k2 ci release <version>`); NULL for check runs.
ALTER TABLE ci_runs ADD COLUMN version TEXT;
--> statement-breakpoint
-- The signer project (`[release] project`).
ALTER TABLE ci_runs ADD COLUMN project TEXT;
--> statement-breakpoint
-- The commit's tree (BUILD_TREE), read once at submit.
ALTER TABLE ci_runs ADD COLUMN tree TEXT;
--> statement-breakpoint
-- The client seat the set was placed on.
ALTER TABLE ci_runs ADD COLUMN seat_id TEXT;
--> statement-breakpoint
-- A release run's job: its receipt label (`L1`, `V1`, `L3`), the signer
-- policy's build name, `build` or `verify`, and the node it was placed on.
ALTER TABLE ci_checks ADD COLUMN label TEXT;
--> statement-breakpoint
ALTER TABLE ci_checks ADD COLUMN build TEXT;
--> statement-breakpoint
ALTER TABLE ci_checks ADD COLUMN role TEXT;
--> statement-breakpoint
ALTER TABLE ci_checks ADD COLUMN node_id TEXT;
--> statement-breakpoint
-- `k2 app updates push … --evidence <run>`: which release run built a
-- release's files. A record for people (`k2 app updates list`), never a
-- trust input: the signer's own checks are what made the `.sig` files.
CREATE TABLE IF NOT EXISTS app_update_evidence (
  release_id    TEXT PRIMARY KEY NOT NULL REFERENCES app_update_releases (id) ON DELETE CASCADE,
  run_id        TEXT NOT NULL,
  project       TEXT,
  sha           TEXT NOT NULL,
  summary_json  TEXT NOT NULL DEFAULT '{}',
  recorded_by   TEXT NOT NULL,
  recorded_at   INTEGER NOT NULL
);
