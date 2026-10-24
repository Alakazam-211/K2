-- 0217: CPU and memory trends for box usage (prd-app-resources-disk-guard-v1
-- §18 Slice B, research B1; 0.46.4). Reserved for Slice B since 0.46.3
-- (0214–0216 compute sharing, 0218 permission cards, 0219 Garden access).
--
-- Additive only: one new table, nothing existing changes. The daemon's
-- trends sampler (`apps/trends.rs`) is its only writer; the store is
-- `k2_core::resource_samples`.
--
-- * One row per (subject, resolution, time):
--   - `subject_kind` `box` (the whole box, `subject_id` = 'box'), `app` (an
--     App id) or `service` (a `--cmd` service, `<workspace id>/<name>`).
--   - `res` 60 (one sample a minute, kept 24 hours) or 3600 (the hour's
--     average and peak, rolled up from the minutes, kept 30 days). Bounded:
--     at most 1440 + 720 rows per subject; a removed App's rows age out.
-- * Unknown is NULL, never 0 (macOS has no box CPU split or available
--   RAM; an App engine without cgroups has no CPU or memory).
--   - `cpu_cores`: cores busy on average over the sample (2.0 = two whole
--     cores; the box row counts every core).
--   - `mem_bytes`: resident memory (the box: in use, total − available);
--     the hour's average. `mem_peak_bytes`: the hour's highest minute.
--   - `mem_budget_bytes`: the App's RAM budget at sample time (the
--     admission budget, `apps.memory_limit_bytes`; the seam for per-App
--     RAM budgets). Read only: nothing here enforces it.
--   - `mem_available_bytes`, `load5`: box rows only.
CREATE TABLE IF NOT EXISTS app_resource_samples (
  subject_kind        TEXT NOT NULL CHECK (subject_kind IN ('box', 'app', 'service')),
  subject_id          TEXT NOT NULL CHECK (length(subject_id) BETWEEN 1 AND 200),
  res                 INTEGER NOT NULL CHECK (res IN (60, 3600)),
  at                  INTEGER NOT NULL,
  cpu_cores           REAL,
  mem_bytes           INTEGER,
  mem_peak_bytes      INTEGER,
  mem_budget_bytes    INTEGER,
  mem_available_bytes INTEGER,
  load5               REAL,
  PRIMARY KEY (subject_kind, subject_id, res, at)
) WITHOUT ROWID;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS app_resource_samples_by_age ON app_resource_samples(res, at);
