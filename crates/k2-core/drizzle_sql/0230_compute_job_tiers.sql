-- 0230: compute job tiers (0.46.5, prd-compute-job-tiers-v1 §3.3 with
-- vs-live T1, T14, T16).
--
-- compute_jobs:
-- - `tier`: `ci` > `priority` > `normal` (the queue order; never stops a
--   running job). `batch` is allowed by the check for 0.46.6 (the windowed
--   Batch tier), never written by 0.46.5. Rows from before 0.46.5 are
--   `normal`, except `class = 'gate'` rows, which become `ci`.
-- - `tier_asked`: what the caller asked when the server lowered it (a guest
--   share where Priority doesn't count: `priority` asked, `normal` stored).
-- - `submitter`: who sent it (`<workspace>/<session short>[/<worktree or
--   --submitter>]`, or `owner` / `login:<user>`): display and the Priority
--   note only, never a security boundary. NULL on older rows.
-- - `supersede_key`: `--supersede KEY`; a newer job with the same key in the
--   same workspace cancels this one while it is still queued (`replaced`).
--
-- compute_node_grants:
-- - `allow_ci`: this workspace may send CI jobs to this node (set only by
--   the owner or an Admin login, never by an agent; T1, T12). Not granted to
--   anyone by this migration (Q11).
--
-- The per-submitter cap (`submitter_max_running`) is cut to 0.46.6 (T14):
-- no column here. `class` stays: older nodes order by it.
ALTER TABLE compute_jobs ADD COLUMN tier TEXT NOT NULL DEFAULT 'normal' CHECK (tier IN ('ci','priority','normal','batch'));
ALTER TABLE compute_jobs ADD COLUMN tier_asked TEXT CHECK (tier_asked IS NULL OR tier_asked IN ('ci','priority','normal','batch'));
ALTER TABLE compute_jobs ADD COLUMN submitter TEXT;
ALTER TABLE compute_jobs ADD COLUMN supersede_key TEXT;
CREATE INDEX IF NOT EXISTS compute_jobs_supersede ON compute_jobs (workspace_id, supersede_key) WHERE supersede_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS compute_jobs_tier_open ON compute_jobs (tier, state) WHERE tier != 'normal';
UPDATE compute_jobs SET tier = 'ci' WHERE class = 'gate';
ALTER TABLE compute_node_grants ADD COLUMN allow_ci INTEGER NOT NULL DEFAULT 0;
