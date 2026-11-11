-- 0295: CI test history and flaky detection (prd-compute-blacksmith-class-v1
-- §4.3, slice T1). Renumber freely on a slip (and edit the migration-order
-- tests). The store is `crate::ci_tests`; the writer is the CI engine's
-- reap (k2-daemon `ci_engine`), after it fetched a check job's JUnit.
--
-- Small on purpose: one row per NON-passing case per job, and one rolling
-- row per test per check. Passing cases only update the rolling row, so a
-- 10,000-test suite adds a few rows per job.

-- `ci_test_jobs`: what was recorded for one check job (idempotent: a job
-- is recorded once). `state`: `done` (cases parsed), `counts_only` (the
-- JUnit was over the 20 MB cap, or a file failed to fetch: the node's
-- counts are kept, no cases), `none` (the check names no JUnit, or the job
-- wrote none).
CREATE TABLE IF NOT EXISTS ci_test_jobs (
  job_id        TEXT PRIMARY KEY NOT NULL,
  workspace_id  TEXT NOT NULL,
  check_name    TEXT NOT NULL,
  run_id        TEXT,
  sha           TEXT,
  tree          TEXT,
  state         TEXT NOT NULL CHECK (state IN ('done','counts_only','none')),
  total         INTEGER NOT NULL DEFAULT 0,
  passed        INTEGER NOT NULL DEFAULT 0,
  failed        INTEGER NOT NULL DEFAULT 0,
  errors        INTEGER NOT NULL DEFAULT 0,
  skipped       INTEGER NOT NULL DEFAULT 0,
  -- Of the failures: known flaky, quarantined, never failed before.
  flaky         INTEGER NOT NULL DEFAULT 0,
  quarantined   INTEGER NOT NULL DEFAULT 0,
  new_failures  INTEGER NOT NULL DEFAULT 0,
  -- Cases nextest reported flaky (passed on a retry).
  retried       INTEGER NOT NULL DEFAULT 0,
  note          TEXT,
  at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ci_test_jobs_ws ON ci_test_jobs(workspace_id, check_name, at);

-- `ci_test_failures`: a failed, errored or flaky case of one job. `mark` is
-- what `--failures` shows: `new`, `flaky` or `quarantined` (a flaky row's
-- mark is `flaky`).
CREATE TABLE IF NOT EXISTS ci_test_failures (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  workspace_id  TEXT NOT NULL,
  check_name    TEXT NOT NULL,
  test_id       TEXT NOT NULL,
  classname     TEXT,
  name          TEXT NOT NULL,
  kind          TEXT NOT NULL CHECK (kind IN ('failure','error','flaky')),
  mark          TEXT NOT NULL CHECK (mark IN ('new','flaky','quarantined','known')),
  job_id        TEXT NOT NULL,
  run_id        TEXT,
  sha           TEXT,
  tree          TEXT,
  node_id       TEXT,
  attempt       INTEGER NOT NULL DEFAULT 1,
  message       TEXT,
  at            INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ci_test_failures_test ON ci_test_failures(workspace_id, check_name, test_id, at);
CREATE INDEX IF NOT EXISTS idx_ci_test_failures_tree ON ci_test_failures(workspace_id, check_name, tree);
CREATE INDEX IF NOT EXISTS idx_ci_test_failures_job ON ci_test_failures(job_id);

-- `ci_test_stats`: one rolling row per test per check. `ewma_ms` is the
-- run time's moving average (weight 1/5), `base_ms` its value 14 days ago
-- (for `--slow`'s "rose more than 50 %"). `last_pass_tree` lets a later
-- failure on the same tree count as flaky (rule 1, both directions).
CREATE TABLE IF NOT EXISTS ci_test_stats (
  workspace_id    TEXT NOT NULL,
  check_name      TEXT NOT NULL,
  test_id         TEXT NOT NULL,
  classname       TEXT,
  name            TEXT NOT NULL,
  runs            INTEGER NOT NULL DEFAULT 0,
  fails           INTEGER NOT NULL DEFAULT 0,
  flakes          INTEGER NOT NULL DEFAULT 0,
  ewma_ms         INTEGER,
  base_ms         INTEGER,
  base_at         INTEGER,
  first_seen      INTEGER NOT NULL,
  last_seen       INTEGER NOT NULL,
  last_fail_at    INTEGER,
  last_flake_at   INTEGER,
  last_flake_tree TEXT,
  last_pass_tree  TEXT,
  PRIMARY KEY (workspace_id, check_name, test_id)
);
CREATE INDEX IF NOT EXISTS idx_ci_test_stats_flaky ON ci_test_stats(workspace_id, last_flake_at);
