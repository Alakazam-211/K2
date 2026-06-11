-- 0.37.8: clear ghost active_terminal_id pointers in workspace_heartbeats
-- left over from the pre-0.37.8 lane collapse.
--
-- Pre-0.37.8 the v2 spawn idempotency check returned the chat tab's
-- existing PTY for every heartbeat fire (canonical_key collision —
-- both the chat tab and heartbeat fires registered under bare
-- `<project_id>`). The heartbeat fire then stamped its own
-- `active_terminal_id` to that PTY id. Result: a row in
-- `workspace_heartbeats.active_terminal_id` that points at the chat
-- tab's PTY, not a heartbeat-owned PTY.
--
-- After this fix, heartbeat fires register under
-- `<project_id>:hb:<name>` and won't collide. Every existing row
-- where `workspace_heartbeats.active_terminal_id` matches the
-- workspace_sessions row's `active_terminal_id` for the same project
-- is a ghost pointer from the buggy era — clear it so the next fire
-- decides cleanly via the smart-launch cascade. The heartbeat's
-- `last_session_id` (claude UUID) stays — un-resolvable claude UUIDs
-- self-heal on the next fire via the JSONL existence check.
UPDATE workspace_heartbeats
SET active_terminal_id = NULL
WHERE active_terminal_id IS NOT NULL
  AND active_terminal_id IN (
    SELECT ws.active_terminal_id
    FROM workspace_sessions ws
    WHERE ws.project_id = workspace_heartbeats.project_id
      AND ws.active_terminal_id IS NOT NULL
  );
