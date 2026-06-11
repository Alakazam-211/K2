-- 0038: rename `workspace_sessions` → `workspace_layouts`.
--
-- The original 0009 table stores per-(project, workspace) tab/pane
-- layout JSON for the K2SO UI's workspace tabs. The name has always
-- been misleading — it stores `layout_json`, not "sessions" in any
-- terminal/PTY/agent sense. 0.37.0 reuses the freed name for the
-- workspace-agent table created in 0039 (one row per project_id,
-- carrying the workspace's PTY session metadata).
--
-- This migration is a pure rename: data + foreign keys preserved,
-- index renamed alongside the table.

ALTER TABLE workspace_sessions RENAME TO workspace_layouts;

DROP INDEX IF EXISTS workspace_sessions_key;
CREATE UNIQUE INDEX workspace_layouts_key
  ON workspace_layouts(project_id, workspace_id);
