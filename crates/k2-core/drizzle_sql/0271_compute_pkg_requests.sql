-- 0271: compute package requests (0.46.9, prd-compute-self-serve-environments-v1 §6, §13).
-- One row per node per filed request. A batch (one card) shares `batch_id`.
-- `origin` is 'own' (this server's agent asked for its own node), 'sent'
-- (this server asked another server's node; the answer comes back through
-- the node) or 'fwd' (another server asked for a node this server owns;
-- this server's people decide). 0269 and 0270 are other 0.46.9 branches';
-- renumber freely on a slip (and edit the migration-order tests).
CREATE TABLE IF NOT EXISTS compute_pkg_requests (
  id              TEXT PRIMARY KEY NOT NULL,
  batch_id        TEXT NOT NULL,
  origin          TEXT NOT NULL CHECK (origin IN ('own','sent','fwd')),
  node_id         TEXT NOT NULL,
  -- The node's own id for it ('npr_…'), once it knows one.
  node_request    TEXT,
  -- 'own', or the asking server's label (a 'fwd' row).
  server          TEXT NOT NULL DEFAULT 'own',
  workspace_id    TEXT,
  session_id      TEXT,
  job_id          TEXT,
  asked_by        TEXT NOT NULL DEFAULT '',
  why             TEXT,
  names_json      TEXT NOT NULL DEFAULT '[]',
  natives_json    TEXT NOT NULL DEFAULT '{}',
  dry_json        TEXT,
  host            TEXT,
  state           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (state IN ('pending','approved','installed','denied','failed','expired')),
  add_to_list     INTEGER NOT NULL DEFAULT 0,
  decided_by      TEXT,
  decided_at      INTEGER,
  note            TEXT,
  created_at      INTEGER NOT NULL,
  expires_at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS compute_pkg_requests_batch ON compute_pkg_requests (batch_id);
CREATE INDEX IF NOT EXISTS compute_pkg_requests_node_state ON compute_pkg_requests (node_id, state);
CREATE INDEX IF NOT EXISTS compute_pkg_requests_ws ON compute_pkg_requests (workspace_id, created_at);
