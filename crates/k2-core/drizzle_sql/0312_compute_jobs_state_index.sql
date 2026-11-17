-- 0312: indexes so the compute controller's hot reads stop scanning
-- every job ever run.
--
-- `compute_jobs.state` comes after `plan_json` in each row, and plans run
-- to tens of KB (argv, env). With no index that starts with `state`, every
-- `WHERE state = …` read (every queued job, every active job: placement,
-- the sweeper, the queue views) walked each row's overflow pages to reach
-- it: the whole table, about 1.2 GB on a busy controller, under the one
-- database lock. Placement held that lock for minutes on a loaded Mac.
--
-- - `compute_jobs_state_created`: the open jobs by state, oldest first.
-- - `compute_jobs_created`: `jobs` with no filter, newest first.
-- - `compute_jobs_ws_cache`: `--cache NAME`'s home node (`cache_home`):
--   the expression must match the query's `json_extract` exactly.
--
-- Indexes only: no row or column changes. Safe to run again.
CREATE INDEX IF NOT EXISTS compute_jobs_state_created ON compute_jobs (state, created_at, id);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_jobs_created ON compute_jobs (created_at, id);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS compute_jobs_ws_cache ON compute_jobs (workspace_id, json_extract(plan_json, '$.cache'), started_at);
