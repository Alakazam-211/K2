-- API session origin + per-workspace "hide sessions" (do not auto-surface
-- /v1 host-session tabs). `from_api` is the durable boolean for
-- workspace_tab_sessions rows spawned via POST /v1 host-sessions or
-- /v1 sandboxes (agent_name is `api-…`). Backfill existing api- rows.

ALTER TABLE workspace_tab_sessions ADD COLUMN from_api INTEGER NOT NULL DEFAULT 0;

UPDATE workspace_tab_sessions
   SET from_api = 1
 WHERE agent_name LIKE 'api-%';

-- 1 = do not auto-adopt API host-session / sandbox tabs onto the strip.
-- Sessions stay listed in Chat history → API. Default 0 (show).
ALTER TABLE projects ADD COLUMN hide_api_sessions INTEGER NOT NULL DEFAULT 0;
