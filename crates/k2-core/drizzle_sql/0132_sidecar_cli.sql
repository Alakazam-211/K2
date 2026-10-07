-- 0132: k2 sidecar v1 (prd-k2-sidecar-cli-v1 SC20 / SC38).
--
-- `projects.agents_can_manage_agents` — the "Allow hiring and managing
-- agents" switch (noun tiers NT11 ships the same column; whichever lands
-- first owns it, the other's ALTER is skipped as a duplicate column).
-- Default 0 (off). Backfill 1 for manager / coordinator / pod workspaces.
-- Written only by POST /cli/agent-access/set (Admin), never workspace/set.
ALTER TABLE projects ADD COLUMN agents_can_manage_agents INTEGER NOT NULL DEFAULT 0;
--> statement-breakpoint
UPDATE projects SET agents_can_manage_agents = 1
  WHERE agent_mode IN ('manager', 'coordinator', 'pod');
--> statement-breakpoint
-- Tab rows of CLI-made sidecars. NULL on every app-made row.
-- `created_by`   who ran `k2 sidecar new` (`owner`, `login:<name>`,
--                `shell:<ws>`, `agent:<ws>`).
-- `brief_path`   workspace-relative `.k2/sidecars/<slug>/BRIEF.md`.
-- `adopt_since`  unix seconds of the spawn that still waits for its
--                harness to write a conversation id (codex / hermes mint
--                their own ids). NULL once adopted or for premint harnesses.
-- The registration upsert names its columns, so these survive re-registers.
ALTER TABLE workspace_tab_sessions ADD COLUMN created_by TEXT;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN brief_path TEXT;
--> statement-breakpoint
ALTER TABLE workspace_tab_sessions ADD COLUMN adopt_since INTEGER;
