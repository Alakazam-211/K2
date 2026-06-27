-- GAP #3 — remote (cross-server) connections.
--
-- `workspace_relations` (migration 0023) models LOCAL, intra-daemon
-- connections (project_id → project_id, same server). It cannot express
-- a connection to an agent on a DIFFERENT daemon because the peer has no
-- `projects` row here.
--
-- This table holds the CROSS-DAEMON half: a source workspace on THIS
-- daemon connected to a remote `<agent>@<full-host>` (e.g.
-- `ai@rpm.k2.dev`). It is the gate for agent-initiated cross-server
-- sends — `federation::handle_send` refuses to dial a peer unless the
-- source workspace has a row here for that `<agent>@<host>`.
--
--   source_project_id = the LOCAL workspace initiating the connection.
--   remote_addr       = the canonical `<agent>@<host>` address.
--   host / agent      = remote_addr split on the LAST `@` (denormalized
--                       for cheap filtering / display).
--   peer_fingerprint  = the paired peer's fingerprint when known (the
--                       peer must already be Trusted via `k2 fed`
--                       pairing; NULL when not yet resolved).
--
-- UNIQUE(source_project_id, remote_addr) makes `connections add`
-- idempotent — re-adding the same remote for the same workspace is a
-- no-op, never a dup.
CREATE TABLE IF NOT EXISTS workspace_remote_connections (
  id TEXT PRIMARY KEY,
  source_project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  remote_addr TEXT NOT NULL,
  host TEXT NOT NULL,
  agent TEXT NOT NULL,
  peer_fingerprint TEXT,
  created_at INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE(source_project_id, remote_addr)
);
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_remote_conn_source ON workspace_remote_connections(source_project_id);
