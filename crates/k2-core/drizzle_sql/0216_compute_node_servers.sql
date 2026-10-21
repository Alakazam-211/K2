-- 0216: the servers on a node this server owns (0.46.3,
-- prd-compute-shared-queue-v1 §9, §11.2).
--
-- Owner side only: a mirror of the node's `servers.list` (refreshed every
-- 5 min and after every `servers.*` change), for `k2 compute node servers`
-- and audit. The node is the truth; nothing here is ever sent back to it.
--
-- - `slot`: the server's number on that node (0 = the owner).
-- - `state`: `pending` (approved, waiting for its SAS), `active`, `paused`,
--   `expired` (past its end date), `revoked`, or `refused` (approved but its
--   attach is refused, e.g. `server_too_old`).
-- - `share_json`: its limits (`ShareLimits`); `usage_json`: today's usage.
-- - `fp`: its full key fingerprint once enrolled (the code's 16-hex prefix
--   before that).
CREATE TABLE IF NOT EXISTS compute_node_servers (
  node_id TEXT NOT NULL REFERENCES compute_nodes(id) ON DELETE CASCADE,
  slot INTEGER NOT NULL CHECK (slot BETWEEN 0 AND 7),
  label TEXT NOT NULL,
  fp TEXT,
  state TEXT NOT NULL CHECK (state IN ('pending','active','paused','expired','revoked','refused')),
  trust TEXT NOT NULL CHECK (trust IN ('own','team')),
  share_json TEXT NOT NULL,
  usage_json TEXT,
  expires_at INTEGER,
  added_at INTEGER,
  added_by TEXT,
  seen_at INTEGER NOT NULL,
  PRIMARY KEY (node_id, slot)
);
