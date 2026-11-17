-- 0313: a compute job's `--env` values live beside its plan, not in it.
--
-- `compute_jobs.plan_json` kept every env value for good: callers pass
-- git bundles as base64 (8 KB values by the dozen) and sometimes secrets,
-- so a busy controller's plans reached 800 MB, read by every scan. The
-- plan now keeps each key with a marker,
-- `{"k2redacted":true,"bytes":N,"sha256":"…"}`, and the value lives here,
-- read by job id only: when the job is sent, moved or retried, and when
-- its receipt is checked (the plan digest is over the real values, as
-- before). No route or log gives these rows. They go 7 days after the job
-- ends, with its plan trim (`crate::compute::trim`). Store:
-- `crate::compute::store`.
--
-- No rows move here: plans written before keep their values until the
-- trimmer reaches them. Safe to run again.
CREATE TABLE IF NOT EXISTS compute_job_env (
  job_id TEXT NOT NULL,
  name   TEXT NOT NULL,
  value  TEXT NOT NULL,
  PRIMARY KEY (job_id, name)
) WITHOUT ROWID;
