-- 0299: hiring and sidecars become two switches (Rosson, 0.46.12).
--
-- Until now one switch, `projects.agents_can_manage_agents` (0132, "Allow
-- hiring and managing agents"), was the agent's only door to its helper
-- agents. It splits:
--   - `agents_can_manage_agents` stays, and now means only "Let this agent
--     manage its sidecars" (open, stop, archive and interrupt its own
--     sidecars, and `k2 sessions open`). The column and its `agents_manage`
--     toggle keep their names, so anything that reads them keeps working.
--   - `agents_can_hire_agents` (new) is "Let this agent hire agents":
--     `k2 agent hire` from an agent (a new workspace, a template hire).
-- DEFAULT 0 (off) for new workspaces. A workspace whose old switch is on
-- gets both on, so nothing it relies on breaks; off stays off for both.
-- Written only by POST /cli/agent-access/set {toggle: "agents_hire"}
-- (Owner/Admin), never workspace/set. 0295–0298 are another arc's.
ALTER TABLE projects ADD COLUMN agents_can_hire_agents INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
UPDATE projects SET agents_can_hire_agents = 1 WHERE agents_can_manage_agents = 1;
