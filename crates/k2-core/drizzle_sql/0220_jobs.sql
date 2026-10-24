-- 0220: daemon-run scheduled jobs, `k2 job` (prd-vault-gaps-v1 rev 2,
-- item 5; 0.46.4). Additive only: nothing reads these tables until someone
-- adds a job.
--
-- * `jobs` — one row per job: its workspace (`projects.id`), name (unique
--   per workspace), schedule (`every:<secs>` or `cron:<expr>`), argv (a JSON
--   array; no shell unless the argv says `sh -c`), working folder (relative
--   to the workspace folder, '' = the folder itself), timeout, the on/off
--   switch, who made it (an actor stamp: `owner`, `agent:<ws>/<sid>`, …) and
--   `anchor_at`, the schedule's reference point (the last slot looked at;
--   enable, a schedule edit and a resume re-anchor at now, so none of them
--   fires a catch-up).
-- * `job_runs` — the audit trail: one row per run, plus `skipped_overlap`
--   / `skipped_missed` / `skipped_busy` rows for slots that didn't run.
--   `outcome`: running | ok | failed | timeout | killed | interrupted |
--   spawn_failed | skipped_overlap | skipped_missed | skipped_busy.
--   `trigger`: schedule | catchup | manual. `bytes` is what the job wrote
--   (the log keeps at most 1 MiB of it). A daemon restart marks a `running`
--   row `interrupted`.
-- * `job_settings` — one row: `paused` = `k2 job pause --all` (people only).
CREATE TABLE IF NOT EXISTS jobs (
  id            TEXT PRIMARY KEY,
  workspace_id  TEXT NOT NULL CHECK (length(workspace_id) >= 1),
  name          TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
  schedule      TEXT NOT NULL,
  argv_json     TEXT NOT NULL,
  cwd           TEXT NOT NULL DEFAULT '',
  timeout_secs  INTEGER NOT NULL DEFAULT 600,
  enabled       INTEGER NOT NULL DEFAULT 1,
  created_by    TEXT NOT NULL,
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL,
  updated_by    TEXT NOT NULL DEFAULT '',
  anchor_at     TEXT NOT NULL,
  UNIQUE (workspace_id, name)
);

CREATE TABLE IF NOT EXISTS job_runs (
  id          TEXT PRIMARY KEY,
  job_id      TEXT NOT NULL,
  started_at  TEXT NOT NULL,
  ended_at    TEXT,
  outcome     TEXT NOT NULL,
  trigger     TEXT NOT NULL DEFAULT 'schedule',
  exit_code   INTEGER,
  log_path    TEXT,
  bytes       INTEGER NOT NULL DEFAULT 0,
  pid         INTEGER,
  started_by  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS job_runs_by_job ON job_runs (job_id, started_at);

CREATE TABLE IF NOT EXISTS job_settings (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  paused     INTEGER NOT NULL DEFAULT 0,
  paused_by  TEXT NOT NULL DEFAULT '',
  paused_at  TEXT
);
