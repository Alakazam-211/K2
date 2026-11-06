-- 0270: k2-node rollouts (prd-compute-update-continuity-v1 §5.10, §8; rev 3).
-- Reserved as 0224 (2026-10-25) and never used; 0224 is now below live
-- numbers, so it takes the next free number.
--
-- compute_node_rollouts: one push of this server's k2-node build to every
-- node it owns (never a tenant node). `state`: running | paused | stopped |
-- done. `batch_size` 0 = all at once (Rosson's Q3: push all at once, pause
-- on the first rollback). `trigger`: explicit (`k2 compute node update
-- --all`) | auto (the controller updated itself and `compute.nodes.follow`
-- is on). It survives the controller's own restarts.
--
-- compute_node_rollout_nodes: each node's place in it. `state`: pending |
-- offered | healthy | rolled_back | refused | failed | timed_out | skipped.
-- `code` says why for skipped / refused / failed (node_needs_manual_install,
-- up_to_date, auto_update_off, …).
CREATE TABLE IF NOT EXISTS compute_node_rollouts (
    id TEXT PRIMARY KEY,
    to_version TEXT NOT NULL,
    trigger TEXT NOT NULL CHECK (trigger IN ('explicit','auto')),
    state TEXT NOT NULL CHECK (state IN ('running','paused','stopped','done')),
    batch_size INTEGER NOT NULL DEFAULT 0,
    now_flag INTEGER NOT NULL DEFAULT 0,
    by TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    note TEXT
);
CREATE TABLE IF NOT EXISTS compute_node_rollout_nodes (
    rollout_id TEXT NOT NULL REFERENCES compute_node_rollouts(id) ON DELETE CASCADE,
    node_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending','offered','healthy','rolled_back','refused','failed','timed_out','skipped')),
    code TEXT,
    detail TEXT,
    at INTEGER NOT NULL,
    offered_at INTEGER,
    PRIMARY KEY (rollout_id, node_id)
);
CREATE INDEX IF NOT EXISTS compute_node_rollout_nodes_node ON compute_node_rollout_nodes (node_id);
