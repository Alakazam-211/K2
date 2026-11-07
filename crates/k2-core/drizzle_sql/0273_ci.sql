-- 0273: `k2 ci` (0.46.10, prd-ci-on-compute-for-clients-v1 C6a, vs-live
-- V32). The daemon owns CI: a run is daemon state, not a `k2 job`, and the
-- watcher is a daemon loop. Renumber freely on a slip (and edit the
-- migration-order tests). The store is `crate::ci`.
--
-- `ci_workspaces`: one row per workspace that set CI up. The watch (the
-- remote and branch patterns the daemon polls, how often, the tier, the
-- cap on runs in flight) and the release freeze flag (`k2 ci freeze`,
-- read by 0.46.11's rebalancer).
CREATE TABLE IF NOT EXISTS ci_workspaces (
  workspace_id   TEXT PRIMARY KEY NOT NULL,
  watch_on       INTEGER NOT NULL DEFAULT 0,
  remote         TEXT NOT NULL DEFAULT 'origin',
  branches_json  TEXT NOT NULL DEFAULT '[]',
  poll_secs      INTEGER NOT NULL DEFAULT 60,
  tier           TEXT NOT NULL DEFAULT 'ci' CHECK (tier IN ('ci','normal')),
  max_inflight   INTEGER NOT NULL DEFAULT 2,
  watch_by       TEXT NOT NULL DEFAULT '',
  watch_at       INTEGER NOT NULL DEFAULT 0,
  last_poll_at   INTEGER,
  last_error     TEXT,
  freeze_until   INTEGER,
  freeze_why     TEXT,
  freeze_by      TEXT,
  freeze_at      INTEGER
);

-- `ci_seen`: commits the watcher already took (or skipped), so a tip is
-- checked once even when two branches point at it.
CREATE TABLE IF NOT EXISTS ci_seen (
  workspace_id TEXT NOT NULL,
  sha          TEXT NOT NULL,
  ref          TEXT NOT NULL,
  seen_at      INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, sha)
);

-- `ci_runs`: one commit checked once. `statuses_json` is where its commit
-- statuses go (`{slug, context, tokenSecret}`; names only, never a token).
CREATE TABLE IF NOT EXISTS ci_runs (
  id            TEXT PRIMARY KEY NOT NULL,
  workspace_id  TEXT NOT NULL,
  sha           TEXT NOT NULL,
  ref           TEXT,
  trigger       TEXT NOT NULL CHECK (trigger IN ('watch','manual')),
  state         TEXT NOT NULL CHECK (state IN ('running','passed','failed','error','skipped')),
  detail        TEXT,
  created_by    TEXT NOT NULL DEFAULT '',
  tier          TEXT NOT NULL DEFAULT 'ci',
  statuses_json TEXT,
  created_at    INTEGER NOT NULL,
  ended_at      INTEGER
);
CREATE INDEX IF NOT EXISTS idx_ci_runs_ws ON ci_runs(workspace_id, created_at);
CREATE INDEX IF NOT EXISTS idx_ci_runs_state ON ci_runs(state);

-- `ci_checks`: one `[checks.<name>]` of a run, its compute job and the
-- commit status last posted for it.
CREATE TABLE IF NOT EXISTS ci_checks (
  run_id       TEXT NOT NULL,
  name         TEXT NOT NULL,
  job_id       TEXT,
  state        TEXT NOT NULL CHECK (state IN ('pending','running','success','failure','error')),
  description  TEXT NOT NULL DEFAULT '',
  posted       TEXT,
  post_tries   INTEGER NOT NULL DEFAULT 0,
  post_error   TEXT,
  PRIMARY KEY (run_id, name)
);
