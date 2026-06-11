-- Per-heartbeat session resumption + archive bucket + show-in-tabs flag.
--
-- Columns:
--
--   agent_heartbeats.last_session_id
--     Claude session id from the most recent successful spawn for THIS
--     heartbeat. Pre-0.36.0 every heartbeat resumed the agent's global
--     session (`agent_sessions.session_id`); the new column lets each
--     heartbeat keep its own dedicated chat that the user can audit
--     via the sidebar Heartbeats panel.
--     Spawns: written by `spawn_wake_headless` after PTY settles.
--     Reads:  `compose_wake_prompt_for_*` prefers this over the agent
--             session row when a heartbeat name is in scope.
--
--   agent_heartbeats.archived_at
--     RFC3339 timestamp when the user archived the heartbeat from
--     Settings. Archived rows are hidden from the Settings list but
--     still appear in the sidebar's collapsed Archived section so the
--     chat history remains readable. NULL = active.
--     Replaces the previous "Remove" delete behaviour: removing now
--     soft-archives instead of hard-deleting. Re-archive is a no-op
--     (the timestamp doesn't bump).
--
--   projects.show_heartbeat_sessions
--     0 (default) = silent autonomous run — heartbeats fire in the
--     daemon, the user audits them on demand by clicking the sidebar
--     entry. Matches the v2-headless vision.
--     1 = each scheduled fire opens a tab in the Tauri window in the
--     background. Tab persists until the user closes it.
--
-- See .k2so/prds/heartbeats-sidebar-audit.md Phase 2 for the full
-- contract this migration is part of.

ALTER TABLE agent_heartbeats ADD COLUMN last_session_id TEXT;
ALTER TABLE agent_heartbeats ADD COLUMN archived_at TEXT;
ALTER TABLE projects ADD COLUMN show_heartbeat_sessions INTEGER NOT NULL DEFAULT 0;

-- Active-rows index for the sidebar's primary query — joins to live
-- session telemetry and filters by project_id + archived_at IS NULL.
CREATE INDEX IF NOT EXISTS idx_agent_heartbeats_active
  ON agent_heartbeats(project_id, archived_at);
