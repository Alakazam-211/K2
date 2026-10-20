-- 0197: per-agent App permissions (prd-agent-app-permissions-v1 rev 2,
-- slice A; 0.46.3, Rosson 2026-10-20).
--
-- Two switches per agent workspace, nested:
-- * switch 1 "Build and run Apps" keeps its column,
--   `projects.agents_can_manage_skin` (0115);
-- * switch 2 "…and manage them end to end" is the new
--   `projects.agents_can_manage_apps_full` (DEFAULT 0). It only counts while
--   switch 1 is on; turning switch 1 off turns it off.
-- Both are written only by POST /cli/agent-access/set (`apps_build`,
-- `apps_manage_full`): the owner raises, an Admin may only lower, an agent
-- never (Q2). New workspaces start with both off (Q3 = A).
--
-- `projects.app_cores` is the pre-approved cores list (JSON array of
-- "db" | "files" | "thread" | "mail" | "sew"; slice B reads it). Upgraded
-- rows that had the old switch on get the cores their Apps already use, so
-- nothing that runs today loses a cap (`apps.approved_caps_json` stays).
--
-- `agents_can_publish_open_apps` (0168) stays for reads; nothing new is
-- written to it (N1: the open path is a per-request person-present
-- confirm now).
--
-- `app_access_review`: the agents that had "Allow this agent to manage
-- Apps" on before this upgrade (Q4: they keep switch 1, switch 2 starts
-- off). The daemon tells the owner once (one review note) and stamps
-- `noted_at`. A fresh database lists nobody.
ALTER TABLE projects ADD COLUMN agents_can_manage_apps_full INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE projects ADD COLUMN app_cores TEXT NOT NULL DEFAULT '[]';
--> statement-breakpoint
CREATE TABLE IF NOT EXISTS app_access_review (
  project_id TEXT PRIMARY KEY,
  created_at INTEGER NOT NULL,
  noted_at INTEGER
);
--> statement-breakpoint
INSERT OR IGNORE INTO app_access_review (project_id, created_at)
  SELECT id, CAST(strftime('%s', 'now') AS INTEGER) FROM projects WHERE agents_can_manage_skin = 1;
--> statement-breakpoint
UPDATE projects SET app_cores = COALESCE((
  SELECT json_group_array(core) FROM (
    SELECT DISTINCT core FROM (
      SELECT CASE
        WHEN c.value IN ('files:read', 'files:write', 'room:members', 'wiki:read', 'tickets:read', 'tickets:post',
                         'activity:read', 'files:agentconfig') THEN 'files'
        WHEN c.value IN ('thread:read', 'thread:post', 'heartbeats:read', 'heartbeats:write', 'agent:identity',
                         'agent:handle') THEN 'thread'
        WHEN c.value IN ('store:read', 'store:write') THEN 'db'
      END AS core
      FROM apps a, json_each(CASE WHEN json_valid(a.approved_caps_json) THEN a.approved_caps_json ELSE '[]' END) c
      WHERE a.owner_workspace = projects.id
      UNION
      SELECT 'sew' AS core
      FROM app_box_services b JOIN apps a ON a.app_id = b.app_id
      WHERE a.owner_workspace = projects.id AND b.preset IN ('sew-api', 'sew-signal') AND b.status = 'allowed'
    )
    WHERE core IS NOT NULL
    ORDER BY core
  )
), '[]')
WHERE agents_can_manage_skin = 1;
