-- 0073: user-selectable heartbeat delivery session — provider identity.
-- `session_provider` records WHICH provider's session store the row's
-- `last_session_id` belongs to when the user pins the heartbeat to a
-- specific saved session (Settings → Heartbeats delivery drop-down /
-- `k2 heartbeat session <name> --set <id> --provider <p>`).
-- NULL (the backfill for every existing row) = the workspace default
-- agent, preserving today's behavior byte-identically. Additive.
ALTER TABLE workspace_heartbeats ADD COLUMN session_provider TEXT;
