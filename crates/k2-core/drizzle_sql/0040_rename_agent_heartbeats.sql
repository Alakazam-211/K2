-- 0040: rename `agent_heartbeats` → `workspace_heartbeats`.
--
-- Cosmetic rename — agent_heartbeats has no `agent_name` column; the
-- multi-schedule PRD already removed it. Each row is per-(workspace,
-- schedule_name), already workspace-keyed in spirit. The rename
-- aligns naming with `workspace_sessions` (0039) so the post-0.37.0
-- schema speaks one consistent vocabulary.

ALTER TABLE agent_heartbeats RENAME TO workspace_heartbeats;

DROP INDEX IF EXISTS idx_agent_heartbeats_project_enabled;
CREATE INDEX idx_workspace_heartbeats_project_enabled
  ON workspace_heartbeats(project_id, enabled);
