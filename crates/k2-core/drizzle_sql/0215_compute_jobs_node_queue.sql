-- 0215: jobs that wait in a shared node's queue (0.46.3,
-- prd-compute-shared-queue-v1 §6, §7, §11.2).
--
-- - `class`: `gate` > `interactive` > `batch`, set by the server from facts
--   (an attached run or `--wait` = interactive; `--detach`, big shard sets
--   and soaks = batch; `--gate` where the share allows it). Sent in
--   `Plan.class` only to nodes offering `queue`.
-- - `est_secs`: the server's run-time estimate (ETAs).
-- - `node_queued` = 1 while the node holds the job in its queue: the row
--   stays `assigned` (reason `node_queued`) and every view shows it as
--   queued with the node's position. A lost node's node-queued rows are
--   requeued with a new generation and no retry or health strike (V56).
-- - `node_pos`, `node_eta_secs`, `node_eta_p90_secs`, `node_blocked_by`,
--   `node_seen_at`: the last `StateUpdate.queue` (QueuePos).
-- - `withdraws`, `withdrawn_at`: the `--any` mover's withdraw limit — not
--   again within 5 min, at most twice in all (V71). A confirmed withdraw
--   (`cancelled/withdrawn`) requeues with a new generation, never finalizes
--   (V46).
ALTER TABLE compute_jobs ADD COLUMN class TEXT NOT NULL DEFAULT 'interactive' CHECK (class IN ('gate','interactive','batch'));
ALTER TABLE compute_jobs ADD COLUMN est_secs INTEGER;
ALTER TABLE compute_jobs ADD COLUMN node_queued INTEGER NOT NULL DEFAULT 0;
ALTER TABLE compute_jobs ADD COLUMN node_pos INTEGER;
ALTER TABLE compute_jobs ADD COLUMN node_eta_secs INTEGER;
ALTER TABLE compute_jobs ADD COLUMN node_eta_p90_secs INTEGER;
ALTER TABLE compute_jobs ADD COLUMN node_blocked_by TEXT;
ALTER TABLE compute_jobs ADD COLUMN node_seen_at INTEGER;
ALTER TABLE compute_jobs ADD COLUMN withdraws INTEGER NOT NULL DEFAULT 0;
ALTER TABLE compute_jobs ADD COLUMN withdrawn_at INTEGER;
