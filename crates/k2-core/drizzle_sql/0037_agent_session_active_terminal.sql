-- 0037: agent_sessions active-terminal tracking — chat tab parity with heartbeats.
--
-- Mirrors 0036's `agent_heartbeats.active_terminal_id`: gives every
-- agent_sessions row a column that points at the daemon session_id of
-- the live PTY currently attached to it. Stamped synchronously when
-- the v2 spawn registers a session under a known agent_name; cleared
-- by the v2_session_map::unregister cleanup hook when the PTY exits.
--
-- Lets the workspace agent's pinned chat tab re-attach across Tauri
-- quit/reopen the same way heartbeat tabs do (0.36.11). Without this,
-- the renderer had to walk the in-memory v2_session_map on every mount
-- to decide whether to attach vs spawn fresh — fine while the daemon
-- is up but breaks down on daemon restart since the column survives
-- where the in-memory map doesn't (and the lookup-by-agent route can
-- lazy-clean stale entries authoritatively).

ALTER TABLE agent_sessions ADD COLUMN active_terminal_id TEXT;
