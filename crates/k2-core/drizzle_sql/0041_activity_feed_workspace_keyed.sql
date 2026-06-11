-- 0041: activity_feed columns from agent-keyed → workspace-keyed.
--
-- Schema-level rename + value preservation:
--   agent_name      → actor          (free-form: agent | user | heartbeat | cli | sms-bridge | external workspace path)
--   from_agent      → from_workspace
--   to_agent        → to_workspace
--
-- `actor` generalizes audit so cross-workspace events from an external
-- system (e.g., the SMS-bridge → workspace handoff that motivates the
-- 0.37.0 redesign) can land in the feed without a fake "agent name"
-- placeholder. Existing rows keep their values verbatim — the
-- migration is a pure column rename, no data transformation.
--
-- SQLite supports RENAME COLUMN since 3.25.0; the bundled rusqlite
-- ships a much newer version, so this is safe. The companion index
-- `idx_feed_agent` referenced the old name and is recreated under
-- the new `actor` name.

ALTER TABLE activity_feed RENAME COLUMN agent_name TO actor;
ALTER TABLE activity_feed RENAME COLUMN from_agent TO from_workspace;
ALTER TABLE activity_feed RENAME COLUMN to_agent TO to_workspace;

DROP INDEX IF EXISTS idx_feed_agent;
CREATE INDEX idx_feed_actor ON activity_feed(project_id, actor);
