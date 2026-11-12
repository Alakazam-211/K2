-- 0296: job stats series (prd-compute-blacksmith-class-v1 §4.5.1, slice O2).
-- Renumber freely on a slip (and edit the migration-order tests). 0295 and
-- 0297 are the same PRD's T1 and O1. The store is `crate::compute::job_stats`;
-- the writer is k2-daemon's `compute_ws::store_outputs` (a node offering
-- `job-stats` sends `Outputs.stats` with a job's end), the reader
-- `k2 compute stats`.
--
-- One row per job attempt (`generation`): the node's series, downsampled to
-- at most 720 points and 64 KB (`stats_json`, `proto::gens::JobStats`), kept
-- here so `compute_jobs.summary_json` stays small. Rows older than 30 days
-- go when a new one is written.
CREATE TABLE IF NOT EXISTS compute_job_stats (
  job_id        TEXT NOT NULL,
  generation    INTEGER NOT NULL,
  workspace_id  TEXT NOT NULL,
  node_id       TEXT NOT NULL,
  every_secs    INTEGER NOT NULL,
  points        INTEGER NOT NULL,
  stats_json    TEXT NOT NULL,
  created_at    INTEGER NOT NULL,
  PRIMARY KEY (job_id, generation)
);
CREATE INDEX IF NOT EXISTS idx_compute_job_stats_created ON compute_job_stats(created_at);
