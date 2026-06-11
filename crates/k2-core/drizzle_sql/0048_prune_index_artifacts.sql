-- 0048: remove stale index artifacts.
--
-- Three classes of dead indexes:
--
-- (1) idx_feed_agent — created on (project_id, agent_name). After
--     migration 0041 renamed activity_feed.agent_name to .actor,
--     this is a true duplicate of idx_feed_actor (same shape, same
--     query coverage). SQLite would only use one of them; the other
--     is dead weight on every INSERT.
--
-- (2) idx_agent_heartbeats_project_enabled +
--     idx_agent_heartbeats_active — created on agent_heartbeats before
--     migration 0040 renamed it to workspace_heartbeats. The new
--     idx_workspace_heartbeats_project_enabled covers the same
--     (project_id, enabled) pattern. The old-named indexes are
--     duplicates after the rename.
--
-- (3) idx_heartbeat_fires_agent_time — created on
--     heartbeat_fires(project_id, agent_name, fired_at DESC). Verified
--     2026-05-23: zero queries filter or sort by agent_name. All three
--     SELECT sites in schema.rs (list_recent_with_project,
--     list_by_project, list_by_schedule_name) filter by project_id
--     and/or schedule_name only. The agent_name column is stored as
--     audit metadata but never used as a lookup key. Dead weight on
--     every scheduler-tick INSERT (high volume in production).
--
-- NOT dropped (audit re-verified): idx_feed_unread. It serves
-- get_unread_messages (schema.rs:2063) and mark_messages_read
-- (schema.rs:2101) for the unread-message badge UI. Real function.

DROP INDEX IF EXISTS idx_feed_agent;
DROP INDEX IF EXISTS idx_agent_heartbeats_project_enabled;
DROP INDEX IF EXISTS idx_agent_heartbeats_active;
DROP INDEX IF EXISTS idx_heartbeat_fires_agent_time;
