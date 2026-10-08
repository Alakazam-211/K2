-- A7 (0.45.1): a remote connection belongs to a remote WORKSPACE, not to a
-- spelling of its name.
--
-- `workspace_remote_connections` rows were keyed only on the typed
-- `agent::host`. A peer workspace has several names (handle, aliases,
-- display name), so a connection saved under one of them refused a send
-- addressed by another. These columns bind a row to the peer's
-- `projects.id` (from the peer's signed roster) plus the names that roster
-- showed for it, so every name of that workspace uses the same connection.
--
--   remote_workspace_id  = the peer's workspace UUID; NULL until a roster
--                          fetch binds the row (on send, add, roster read
--                          or when the peer is next seen online).
--   remote_handle        = the peer workspace's handle at bind time.
--   remote_display_name  = its display name at bind time (display only).
--
-- Safe to re-run under another number: the runner skips "duplicate
-- column" and the index uses IF NOT EXISTS.
ALTER TABLE workspace_remote_connections ADD COLUMN remote_workspace_id TEXT;
--> statement-breakpoint
ALTER TABLE workspace_remote_connections ADD COLUMN remote_handle TEXT;
--> statement-breakpoint
ALTER TABLE workspace_remote_connections ADD COLUMN remote_display_name TEXT;
--> statement-breakpoint
CREATE INDEX IF NOT EXISTS idx_remote_conn_remote_ws ON workspace_remote_connections(source_project_id, remote_workspace_id);
